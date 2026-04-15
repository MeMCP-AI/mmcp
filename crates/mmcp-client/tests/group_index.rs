//! Integration tests for `GroupIndex`.
//!
//! These exercise the on-disk walk that turns every bare
//! repository under `~/.mmcp/repos/` into a typed index entry,
//! verifying the manifest round-trip and the skip-on-error
//! behaviour that keeps one broken repo from breaking the whole
//! scan.

use std::path::PathBuf;
use std::sync::Arc;

use mmcp_client::state::GroupIndex;
use mmcp_core::id::GroupId;
use mmcp_core::manifest::{GroupManifest, MANIFEST_FILENAME};
use mmcp_git::{CommitSpec, GitBackend, GroupRef, NativeBackend};
use tempfile::TempDir;
use uuid::Uuid;

fn make_backend() -> (Arc<NativeBackend>, PathBuf, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let backend = Arc::new(NativeBackend::new(&root).expect("backend"));
    (backend, root, tmp)
}

async fn seed_group(
    backend: &NativeBackend,
    slug: &str,
    owner: Uuid,
) -> (GroupManifest, Uuid) {
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    let handle = backend
        .create_group_repo(&GroupRef::new(uuid, slug))
        .await
        .expect("create repo");
    let manifest = GroupManifest::new_user_owned(group_id, slug, owner);
    let rendered = manifest.to_toml().expect("render manifest");
    backend
        .write_commit(
            &handle,
            CommitSpec {
                branch: "main".to_string(),
                author_name: "test".into(),
                author_email: "test@example.com".into(),
                message: "seed manifest".into(),
                files: vec![(MANIFEST_FILENAME.to_string(), Some(rendered.into_bytes()))],
            },
        )
        .await
        .expect("write commit");
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
async fn repo_without_manifest_is_skipped() {
    let (backend, root, _tmp) = make_backend();
    // Create a bare repo via the backend but never commit a manifest.
    let group_id = GroupId::new();
    let uuid = *group_id.as_uuid();
    backend
        .create_group_repo(&GroupRef::new(uuid, "orphan"))
        .await
        .expect("create repo");

    let index = GroupIndex::build(root, backend).await.expect("build");
    // The bare repo has no main branch yet, so the manifest read
    // fails and the scanner logs + skips it. The index stays empty.
    assert!(index.list().await.is_empty());
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
