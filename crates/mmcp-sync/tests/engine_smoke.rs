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
use std::sync::Arc;

use mmcp_core::id::{GroupId, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle};
use mmcp_sync::{GroupHandleResolver, ManifestResponse, RemoteGroup, SyncClient, SyncEngine};
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// Build a real native backend rooted in a tempdir plus one seeded
/// group so the tests can exercise the content plane alongside
/// the control plane.
async fn seeded_backend() -> (Arc<NativeBackend>, MapResolver, Uuid, TempDir) {
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine
        .push(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("push ok");

    assert_eq!(report.pushed.len(), 1);
    assert_eq!(report.pushed[0].group_id, group_uuid);
    assert!(!report.pushed[0].content_transferred);
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine
        .push(
            mmcp_sync::SyncFilter::Group(group_uuid),
            &resolver,
            &resolver,
        )
        .await
        .expect("push ok");

    assert_eq!(report.pushed.len(), 1);
    assert_eq!(report.pushed[0].group_id, group_uuid);
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine
        .push(mmcp_sync::SyncFilter::Group(ghost), &resolver, &resolver)
        .await
        .expect("push ok");

    assert!(report.pushed.is_empty());
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine
        .fetch(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("fetch ok");
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].slug, "team-rust");
    assert_eq!(report.groups[0].remote_head, "aaa");
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
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
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine
        .sync(mmcp_sync::SyncFilter::All, &resolver, &resolver)
        .await
        .expect("sync ok");
    assert_eq!(report.pulled.updated.len(), 1);
    assert_eq!(report.pushed.pushed.len(), 1);
    assert_eq!(report.pushed.pushed[0].group_id, group_uuid);
}
