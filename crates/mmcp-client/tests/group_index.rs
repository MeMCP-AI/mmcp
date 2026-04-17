//! Integration tests for `GroupIndex`.
//!
//! These exercise the on-disk walk that turns every bare
//! repository under `~/.mmcp/repos/` into a typed index entry,
//! verifying the manifest round-trip and the skip-on-error
//! behaviour that keeps one broken repo from breaking the whole
//! scan.

use std::path::PathBuf;
use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend};
use mmcp_store::groups::GroupIndex;
use tempfile::TempDir;
use uuid::Uuid;

fn make_backend() -> (Arc<NativeBackend>, PathBuf, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let backend = Arc::new(NativeBackend::new(&root).expect("backend"));
    (backend, root, tmp)
}

async fn seed_group(backend: &NativeBackend, slug: &str, owner: Uuid) -> (GroupManifest, Uuid) {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let manifest = GroupManifest::new_user_owned(group_id, slug, owner);
    backend
        .create_group_repo(&manifest)
        .await
        .expect("create repo");
    (manifest, uuid)
}

#[tokio::test]
async fn empty_repos_root_yields_empty_index() {
    let (backend, root, _tmp) = make_backend();
    let index = GroupIndex::build(root, backend).await.expect("build");
    assert!(index.list().await.is_empty());
}

#[tokio::test]
async fn repo_with_manifest_is_indexed() {
    let (backend, root, _tmp) = make_backend();
    let owner = Uuid::now_v7();
    let (manifest, group_uuid) = seed_group(&backend, "team-rust", owner).await;

    let index = GroupIndex::build(root, backend).await.expect("build");
    let entries = index.list().await;
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    assert_eq!(entry.manifest, manifest);
    assert_eq!(entry.manifest.slug, "team-rust");
    assert_eq!(entry.handle.group_id, group_uuid);

    let lookup = index.get(&GroupId::from_uuid(group_uuid)).await;
    assert!(lookup.is_some(), "entry should be retrievable by id");
}

#[tokio::test]
async fn multiple_repos_are_all_indexed() {
    let (backend, root, _tmp) = make_backend();
    let owner = Uuid::now_v7();
    let (_, id_a) = seed_group(&backend, "alpha", owner).await;
    let (_, id_b) = seed_group(&backend, "beta", owner).await;
    let (_, id_c) = seed_group(&backend, "gamma", owner).await;

    let index = GroupIndex::build(root, backend).await.expect("build");
    let entries = index.list().await;
    assert_eq!(entries.len(), 3);

    for uuid in [id_a, id_b, id_c] {
        assert!(
            index.get(&GroupId::from_uuid(uuid)).await.is_some(),
            "missing entry for {uuid}"
        );
    }
}

#[tokio::test]
async fn broken_repo_directory_is_skipped() {
    let (_backend, root, _tmp) = make_backend();
    // Drop a directory named like a bare repo but containing no
    // actual git state. The scanner should log the open failure
    // and skip it rather than erroring the whole index.
    let broken_uuid = Uuid::now_v7();
    let broken_dir = root.join(format!("{broken_uuid}.git"));
    std::fs::create_dir_all(&broken_dir).expect("mkdir broken repo");

    // Also seed a healthy neighbour so we can prove the scan
    // continues past the broken repo.
    let (_, healthy_uuid) = seed_group(&_backend, "healthy", Uuid::now_v7()).await;

    let index = GroupIndex::build(root, _backend).await.expect("build");
    let entries = index.list().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].handle.group_id, healthy_uuid,
        "only the healthy repo should survive the scan"
    );
}

#[tokio::test]
async fn refresh_picks_up_newly_created_group() {
    let (backend, root, _tmp) = make_backend();
    let index = GroupIndex::build(root.clone(), backend.clone())
        .await
        .expect("build");
    assert!(index.list().await.is_empty());

    let owner = Uuid::now_v7();
    let (_, uuid) = seed_group(&backend, "team-rust", owner).await;

    // Manual refresh: the watcher spawn is tested elsewhere; here
    // we only care that `refresh` itself picks up the new repo.
    index.refresh().await.expect("refresh");
    assert_eq!(index.list().await.len(), 1);
    assert!(index.get(&GroupId::from_uuid(uuid)).await.is_some());
}
