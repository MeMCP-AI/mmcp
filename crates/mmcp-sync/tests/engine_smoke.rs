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

use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::BumpIntent;
use mmcp_git::{GitBackend, NativeBackend, RepoHandle};
use mmcp_sync::{
    ConflictBody, GroupHandleResolver, ManifestResponse, PendingEdit, PendingQueue, PushRequest,
    PushResponse, RemoteGroup, SyncClient, SyncEngine, SyncError,
};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;
use wiremock::matchers::{method, path, path_regex};
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
    let owner = Uuid::now_v7();
    let group_id = GroupId::new();
    let manifest = GroupManifest::new_user_owned(group_id, "team-rust", owner);
    let handle = backend.create_group_repo(&manifest).await.expect("create");
    let mut resolver = MapResolver::default();
    resolver.insert(*group_id.as_uuid(), handle);
    (backend, resolver, *group_id.as_uuid(), tmp)
}

#[tokio::test]
async fn push_drains_the_queue_and_records_versions() {
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    // Two pending edits queued up.
    let queue = PendingQueue::new();
    let edit_a = PendingEdit::new(group_uuid, group_uuid, "aaa", BumpIntent::Patch, "a");
    let edit_b = PendingEdit::new(group_uuid, group_uuid, "bbb", BumpIntent::Minor, "b");
    queue.enqueue(edit_a.clone());
    queue.enqueue(edit_b.clone());

    // Server assigns versions in order: 0.1.1 then 0.2.0.
    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(PushResponse {
            group_id: group_uuid,
            memory_id: group_uuid,
            assigned_version: "0.1.1".to_string(),
            tag: "v0.1.1".to_string(),
        }))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(PushResponse {
            group_id: group_uuid,
            memory_id: group_uuid,
            assigned_version: "0.2.0".to_string(),
            tag: "v0.2.0".to_string(),
        }))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine.push(&queue, mmcp_sync::SyncFilter::All, &resolver, &resolver).await.expect("push ok");

    assert_eq!(report.drained.len(), 2);
    assert_eq!(report.drained[0].response.assigned_version, "0.1.1");
    assert_eq!(report.drained[1].response.assigned_version, "0.2.0");
    assert!(!report.drained[0].content_transferred);
    assert!(queue.is_empty());
}

#[tokio::test]
async fn push_re_enqueues_the_failing_edit_on_transport_error() {
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let queue = PendingQueue::new();
    let edit_a = PendingEdit::new(group_uuid, group_uuid, "aaa", BumpIntent::Patch, "a");
    let edit_b = PendingEdit::new(group_uuid, group_uuid, "bbb", BumpIntent::Patch, "b");
    queue.enqueue(edit_a.clone());
    queue.enqueue(edit_b.clone());

    // First push succeeds, second one 500s.
    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(PushResponse {
            group_id: group_uuid,
            memory_id: group_uuid,
            assigned_version: "0.1.1".to_string(),
            tag: "v0.1.1".to_string(),
        }))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream blew up"))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let err = engine.push(&queue, mmcp_sync::SyncFilter::All, &resolver, &resolver).await.unwrap_err();
    match err {
        SyncError::Remote { status, .. } => assert_eq!(status, 500),
        other => panic!("expected Remote, got {other:?}"),
    }
    // First edit drained, second edit re-queued so the caller can
    // retry without losing it.
    assert_eq!(queue.len(), 1);
    let remaining = queue.snapshot();
    assert_eq!(remaining[0].commit, "bbb");
}

#[tokio::test]
async fn push_conflict_surfaces_structured_error() {
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let queue = PendingQueue::new();
    let edit = PendingEdit::new(group_uuid, group_uuid, "local-sha", BumpIntent::Minor, "ship it");
    queue.enqueue(edit.clone());

    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .respond_with(ResponseTemplate::new(409).set_body_json(ConflictBody {
            memory_id: group_uuid,
            local_commit: "local-sha".to_string(),
            remote_commit: "remote-sha".to_string(),
        }))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let err = engine.push(&queue, mmcp_sync::SyncFilter::All, &resolver, &resolver).await.unwrap_err();
    match err {
        SyncError::Conflict {
            memory,
            local_commit,
            remote_commit,
        } => {
            assert_eq!(memory, group_uuid);
            assert_eq!(local_commit, "local-sha");
            assert_eq!(remote_commit, "remote-sha");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    // Edit stays enqueued so the caller can resolve and retry.
    assert_eq!(queue.len(), 1);
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
    let report = engine.pull(mmcp_sync::SyncFilter::All, &resolver, &resolver).await.expect("pull ok");
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
    Mock::given(method("POST"))
        .and(path_regex(r"^/sync/push$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(PushResponse {
            group_id: group_uuid,
            memory_id: group_uuid,
            assigned_version: "0.1.1".to_string(),
            tag: "v0.1.1".to_string(),
        }))
        .mount(&server)
        .await;

    let queue = PendingQueue::new();
    queue.enqueue(PendingEdit::new(
        group_uuid,
        group_uuid,
        "local-sha",
        BumpIntent::Patch,
        "ship it",
    ));

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine.sync(&queue, mmcp_sync::SyncFilter::All, &resolver, &resolver).await.expect("sync ok");
    assert_eq!(report.pulled.updated.len(), 1);
    assert_eq!(report.pushed.drained.len(), 1);
    assert!(queue.is_empty());
}

#[tokio::test]
async fn push_request_shape_is_recognisable_on_the_wire() {
    // Exhaustively asserts the JSON body shape by using wiremock's
    // `body_json` matcher, giving the plan's phase 6 handlers a
    // contract to implement against.
    let server = MockServer::start().await;
    let (backend, resolver, group_uuid, _tmp) = seeded_backend().await;

    let queue = PendingQueue::new();
    let edit = PendingEdit::new(group_uuid, group_uuid, "local-sha", BumpIntent::Major, "big change");
    queue.enqueue(edit.clone());

    Mock::given(method("POST"))
        .and(path("/sync/push"))
        .and(wiremock::matchers::body_json(&PushRequest {
            group_id: group_uuid,
            memory_id: group_uuid,
            commit: "local-sha".to_string(),
            bump: BumpIntent::Major,
            message: Some("big change".to_string()),
        }))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "group_id": group_uuid,
            "memory_id": group_uuid,
            "assigned_version": "1.0.0",
            "tag": "v1.0.0"
        })))
        .mount(&server)
        .await;

    let client = SyncClient::new(server.uri()).expect("client");
    let engine = SyncEngine::new(backend as Arc<dyn GitBackend>, client);
    let report = engine.push(&queue, mmcp_sync::SyncFilter::All, &resolver, &resolver).await.expect("push ok");
    assert_eq!(report.drained[0].response.assigned_version, "1.0.0");
}
