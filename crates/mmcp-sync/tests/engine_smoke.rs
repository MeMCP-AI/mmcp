#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for `SyncEngine` against a wiremock-fronted
//! `mmcp-server` and a real in-process native git backend.
//!
//! Every test stands up a dedicated wiremock server, mounts the
//! `/sync/*` routes it needs, and drives the engine exactly as a
//! production caller would. The git content plane runs on a
//! tempdir-backed native backend; the network git push/fetch path
//! remains `Unsupported` until the native HTTP transport lands,
//! and the engine's reports explicitly record that.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle};
use mmcp_sync::{
    BoundRemote, GroupHandleResolver, ManifestResponse, PushScope, RemoteGroup, RemoteTransport,
    SyncClient, SyncEngine,
};
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Thread-safe byte sink shared between a `tracing_subscriber::fmt`
/// layer and the test that reads it back. `MakeWriter` is
/// implemented on the handle itself so `.clone()` on each write call
/// keeps sharing the same underlying buffer.
#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl CapturedLog {
    fn contains(&self, needle: &str) -> bool {
        let bytes = self.0.lock().expect("log buffer lock");
        String::from_utf8_lossy(&bytes).contains(needle)
    }
}

impl io::Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Tiny resolver built from an explicit map. Matches the shape
/// the real client will expose through its `GroupIndex`.
#[derive(Default, Clone)]
struct MapResolver {
    entries: HashMap<Uuid, RepoHandle>,
}

impl MapResolver {
    fn insert(&mut self, id: Uuid, handle: RepoHandle) {
        self.entries.insert(id, handle);
    }
}

impl GroupHandleResolver for MapResolver {
    fn resolve(&self, group_id: Uuid) -> Option<RepoHandle> {
        self.entries.get(&group_id).cloned()
    }

    fn iter_group_ids(&self) -> Vec<Uuid> {
        self.entries.keys().copied().collect()
    }
}

impl mmcp_sync::ScopeIndex for MapResolver {
    fn scope_of(&self, _group_id: Uuid) -> Option<mmcp_core::manifest::GroupScope> {
        // The engine's filter dispatch only consults the scope
        // index for `SyncFilter::Scope`; the existing tests drive
        // the engine with `SyncFilter::All`, where `scope_of` is
        // never called, so a trivial None impl suffices.
        None
    }
}

/// Insertion-ordered resolver, unlike [`MapResolver`] whose
/// `HashMap`-backed `iter_group_ids` has unspecified iteration
/// order. The partial-success test below needs a deterministic list order.
#[derive(Default, Clone)]
struct OrderedResolver {
    entries: Vec<(Uuid, RepoHandle)>,
}

impl OrderedResolver {
    fn insert(&mut self, id: Uuid, handle: RepoHandle) {
        self.entries.push((id, handle));
    }
}

impl GroupHandleResolver for OrderedResolver {
    fn resolve(&self, group_id: Uuid) -> Option<RepoHandle> {
        self.entries
            .iter()
            .find(|(id, _)| *id == group_id)
            .map(|(_, handle)| handle.clone())
    }

    fn iter_group_ids(&self) -> Vec<Uuid> {
        self.entries.iter().map(|(id, _)| *id).collect()
    }
}

impl mmcp_sync::ScopeIndex for OrderedResolver {
    fn scope_of(&self, _group_id: Uuid) -> Option<mmcp_core::manifest::GroupScope> {
        None
    }
}

/// Wrap a single `SyncClient` as the engine's sole bound remote:
/// named `"primary"`, marked default, included in `PushScope::All`.
/// Every test below drove a single-`SyncClient` engine before the
/// multi-remote restructuring; this helper keeps that shape while
/// adapting to `SyncEngine::new`'s new `Vec<BoundRemote>` signature.
fn bound(client: SyncClient) -> Vec<BoundRemote> {
    vec![BoundRemote {
        name: "primary".to_string(),
        default: true,
        include_in_push_all: true,
        transport: RemoteTransport::MmcpServer(client),
    }]
}

/// Seeds tracing's process-global per-callsite interest cache as
/// permanently "always interested", once per test-binary process.
///
/// `tracing` caches whether anyone cares about a given callsite the
/// FIRST time it fires, and that cache is process-wide, not
/// thread-local. Under cargo's parallel test runner, whichever test
/// happens to hit `push_one_group`'s warn! callsite first, with no
/// subscriber installed, would otherwise cache `Interest::never` and
/// silently starve every later test's thread-local capturing
/// subscriber. Installing a maximally-permissive global default
/// before any test's first `push`/`fetch`/`pull` call keeps the
/// cache "always interested" for the rest of the process; each
/// test's own `tracing::subscriber::set_default` then reliably
/// receives every event on its own thread.
fn ensure_tracing_interest_cache_stays_open() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(io::sink)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

/// Build a real native backend rooted in a tempdir plus one seeded
/// group so the tests can exercise the content plane alongside
/// the control plane.
async fn seeded_backend() -> (Arc<NativeBackend>, MapResolver, Uuid, TempDir) {
    ensure_tracing_interest_cache_stays_open();
    let tmp = TempDir::new().expect("tempdir");
    let backend = Arc::new(NativeBackend::new(tmp.path()).expect("backend"));
    let owner = UserId::new();
    let group_id = GroupId::new();
    let manifest = GroupManifest::new_user_owned(group_id, "team-rust", owner);
    let handle = backend.create_group_repo(&manifest).await.expect("create");
    let mut resolver = MapResolver::default();
    resolver.insert(*group_id.as_uuid(), handle);
    (backend, resolver, *group_id.as_uuid(), tmp)
}

#[tokio::test]
async fn push_reports_each_in_scope_group_with_transport_status() {
    // Git-symmetric write path: no per-edit queue, no control
    // plane round trip. `push` walks local groups and runs
    // `git push origin main` on each one. With a wiremock URL
    // the native backend can't actually ship bytes, so the
    // report records `content_transferred: false` without raising.
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .push(
            mmcp_sync::SyncFilter::All,
            PushScope::Default,
            &resolver,
            &resolver,
        )
        .await
        .expect("push ok");

    assert_eq!(report.by_remote.len(), 1);
    assert_eq!(report.by_remote[0].pushed.len(), 1);
    assert_eq!(report.by_remote[0].pushed[0].group_id, group_uuid);
    assert!(!report.by_remote[0].pushed[0].content_transferred);
}

/// A `GitError::Transport` collapses into `content_transferred: false`.
/// It also emits a warn-level log line naming the affected group instead of failing silently.
/// The same wiremock-fronted server used above cannot serve git smart HTTP,
/// so it drives the native backend into the exact `Transport` arm.
/// Installs a scoped `tracing_subscriber` writing into an in-memory buffer rather than relying on a global subscriber.
/// This test never races other tests' log output as a result.
#[tokio::test]
async fn push_transport_failure_logs_a_warning_instead_of_staying_silent() {
    let captured = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();

    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;
    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));

    // `set_default`'s guard is thread-local, not closure-scoped, so
    // it stays alive across the `.await` below. `#[tokio::test]`
    // defaults to a single-threaded (`current_thread`) runtime, so
    // the task resumes on the same OS thread after the git
    // subprocess's `spawn_blocking` call completes, keeping the
    // whole call graph under this subscriber. `seeded_backend`
    // already called `ensure_tracing_interest_cache_stays_open`, so
    // the callsite below is guaranteed to actually reach this
    // thread-local subscriber instead of a stale process-global
    // `Interest::never` verdict.
    let _guard = tracing::subscriber::set_default(subscriber);
    let report = engine
        .push(
            mmcp_sync::SyncFilter::All,
            PushScope::Default,
            &resolver,
            &resolver,
        )
        .await
        .expect("push ok");
    drop(_guard);

    assert!(!report.by_remote[0].pushed[0].content_transferred);
    assert!(
        captured.contains("push content-plane transport failed"),
        "the transport failure swallow must emit a warn-level log line"
    );
    assert!(
        captured.contains(&group_uuid.to_string()),
        "the log line must name the affected group"
    );
}

#[tokio::test]
async fn push_group_filter_restricts_to_matching_group() {
    // Seed a second group in the same backend and confirm that
    // `SyncFilter::Group(target)` only pushes that one.
    let server = MockServer::start().await;
    let (backend, mut resolver, group_uuid, _tmp) = seeded_backend().await;

    let other_owner = UserId::new();
    let other_id = GroupId::new();
    let other_manifest = GroupManifest::new_user_owned(other_id, "team-python", other_owner);
    let other_handle = backend
        .create_group_repo(&other_manifest)
        .await
        .expect("seed second group");
    resolver.insert(*other_id.as_uuid(), other_handle);

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .push(
            mmcp_sync::SyncFilter::Group(group_uuid),
            PushScope::Default,
            &resolver,
            &resolver,
        )
        .await
        .expect("push ok");

    assert_eq!(report.by_remote[0].pushed.len(), 1);
    assert_eq!(report.by_remote[0].pushed[0].group_id, group_uuid);
}

#[tokio::test]
async fn push_unknown_group_is_a_silent_noop() {
    // Filter targets a uuid the resolver has never heard of. The
    // engine skips it without erroring - symmetric with how
    // `pull` reports unknown groups under `new_groups` rather
    // than raising.
    let server = MockServer::start().await;
    let (backend, resolver, _group_uuid, _tmp) = seeded_backend().await;

    let ghost = Uuid::now_v7();
    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .push(
            mmcp_sync::SyncFilter::Group(ghost),
            PushScope::Default,
            &resolver,
            &resolver,
        )
        .await
        .expect("push ok");

    assert!(report.by_remote[0].pushed.is_empty());
}

#[tokio::test]
async fn fetch_reports_each_in_scope_group_and_lists_new_ones() {
    // Git-symmetric read path: the manifest lists one known and
    // one new group. Fetch walks both, reports the indexed one
    // under `groups` with its advertised remote head, and the
    // unknown one under `new_groups` (never auto-cloned).
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let new_group = Uuid::now_v7();
    let manifest = ManifestResponse {
        groups: vec![
            RemoteGroup {
                group_id: group_uuid,
                slug: "team-rust".to_string(),
                head_commit: "aaa".to_string(),
            },
            RemoteGroup {
                group_id: new_group,
                slug: "team-python".to_string(),
                head_commit: "bbb".to_string(),
            },
        ],
    };
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&manifest))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .fetch(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("fetch ok");
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].slug.as_deref(), Some("team-rust"));
    assert_eq!(report.groups[0].remote_head.as_deref(), Some("aaa"));
    assert_eq!(report.groups[0].remote_name, "primary");
    assert_eq!(report.new_groups.len(), 1);
    assert_eq!(report.new_groups[0].slug, "team-python");
}

#[tokio::test]
async fn pull_reports_updated_and_new_groups() {
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    // The server advertises our known group plus a new one the
    // client has never seen.
    let new_group = Uuid::now_v7();
    let manifest = ManifestResponse {
        groups: vec![
            RemoteGroup {
                group_id: group_uuid,
                slug: "team-rust".to_string(),
                head_commit: "aaa".to_string(),
            },
            RemoteGroup {
                group_id: new_group,
                slug: "team-python".to_string(),
                head_commit: "bbb".to_string(),
            },
        ],
    };
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&manifest))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .pull(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("pull ok");
    assert_eq!(report.updated.len(), 1);
    assert_eq!(report.updated[0].slug, "team-rust");
    assert_eq!(report.new_groups.len(), 1);
    assert_eq!(report.new_groups[0].slug, "team-python");
}

#[tokio::test]
async fn sync_runs_pull_then_push() {
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let manifest = ManifestResponse {
        groups: vec![RemoteGroup {
            group_id: group_uuid,
            slug: "team-rust".to_string(),
            head_commit: "aaa".to_string(),
        }],
    };
    Mock::given(method("GET"))
        .and(path("/sync/manifest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&manifest))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .sync(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("sync ok");
    assert_eq!(report.pulled.updated.len(), 1);
    assert_eq!(report.pushed.total_pushed(), 1);
    assert_eq!(report.pushed.by_remote[0].pushed[0].group_id, group_uuid);
}

#[tokio::test]
async fn push_survives_an_earlier_groups_failure_and_attributes_it_correctly() {
    // `push`'s result-aggregation keeps every already-collected success after an
    // earlier-in-list-order group fails. This seeds an EARLIER-in-list-order group whose local
    // repo directory is deleted out from under it (a genuine `GitError::RepoNotFound`, never
    // `Unsupported`/`Transport`, so it actually reaches the `Err` arm) and a LATER-in-list-order
    // group that pushes normally, then asserts both survive into the report: the later group's
    // success is recorded, and the failure is attributed to the earlier group specifically,
    // rather than the whole call collapsing to a single top-level error.
    let server = MockServer::start().await;
    let tmp = TempDir::new().expect("tempdir");
    let backend = Arc::new(NativeBackend::new(tmp.path()).expect("backend"));

    let broken_owner = UserId::new();
    let broken_group_id = GroupId::new();
    let broken_manifest =
        GroupManifest::new_user_owned(broken_group_id, "team-broken", broken_owner);
    let broken_handle = backend
        .create_group_repo(&broken_manifest)
        .await
        .expect("create broken group repo");
    // Delete the repo directory out from under its own handle: the
    // native backend's `open_repo` reports `GitError::RepoNotFound`
    // for a vanished path (never `Unsupported`), so `push_one_group`
    // returns a genuine `Err` for this group.
    std::fs::remove_dir_all(&broken_handle.locator).expect("delete broken repo directory");

    let good_owner = UserId::new();
    let good_group_id = GroupId::new();
    let good_manifest = GroupManifest::new_user_owned(good_group_id, "team-good", good_owner);
    let good_handle = backend
        .create_group_repo(&good_manifest)
        .await
        .expect("create good group repo");

    // Insertion order fixes list order: the broken group is index 0
    // (earlier), the good group is index 1 (later).
    let mut resolver = OrderedResolver::default();
    resolver.insert(*broken_group_id.as_uuid(), broken_handle);
    resolver.insert(*good_group_id.as_uuid(), good_handle);

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, bound(client));
    let report = engine
        .push(
            mmcp_sync::SyncFilter::All,
            PushScope::Default,
            &resolver,
            &resolver,
        )
        .await
        .expect("push must still return Ok with partial success, not a bare top-level Err");
    let outcome = &report.by_remote[0];

    assert_eq!(
        outcome.pushed.len(),
        1,
        "the later, healthy group's push must still be recorded despite the earlier group's \
         failure: {:?}",
        outcome.pushed
    );
    assert_eq!(outcome.pushed[0].group_id, *good_group_id.as_uuid());

    assert_eq!(
        outcome.failed.len(),
        1,
        "the earlier group's failure must be attributed, not silently dropped: {:?}",
        outcome
            .failed
            .iter()
            .map(|f| f.group_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(outcome.failed[0].group_id, broken_group_id);
    assert!(
        matches!(
            outcome.failed[0].error,
            mmcp_sync::SyncError::Git(mmcp_git::GitError::RepoNotFound(_))
        ),
        "expected a RepoNotFound git error for the deleted repo, got {:?}",
        outcome.failed[0].error
    );
}
