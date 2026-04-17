//! Integration tests for `GroupIndex`.
//!
//! These exercise the on-disk walk that turns every bare
//! repository under `~/.mmcp/repos/` into a typed index entry,
//! verifying the manifest round-trip and the skip-on-error
//! behaviour that keeps one broken repo from breaking the whole
//! scan.

use mmcp_store::groups::GroupIndex;
use mmcp_store::testing::ScratchHome;
use uuid::Uuid;

#[tokio::test]
async fn empty_repos_root_yields_empty_index() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    assert!(scratch.groups().list().await.is_empty());
}

#[tokio::test]
async fn repo_with_manifest_is_indexed() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    let seeded = scratch.seed_group("team-rust").await.expect("seed group");

    let entries = scratch.groups().list().await;
    assert_eq!(entries.len(), 1);

    let entry = &entries[0];
    assert_eq!(entry.manifest, seeded.manifest);
    assert_eq!(entry.manifest.slug, "team-rust");
    assert_eq!(entry.handle.group_id, *seeded.group_id.as_uuid());

    let lookup = scratch.groups().get(&seeded.group_id).await;
    assert!(lookup.is_some(), "entry should be retrievable by id");
}

#[tokio::test]
async fn multiple_repos_are_all_indexed() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    let a = scratch.seed_group("alpha").await.expect("seed alpha");
    let b = scratch.seed_group("beta").await.expect("seed beta");
    let c = scratch.seed_group("gamma").await.expect("seed gamma");

    let entries = scratch.groups().list().await;
    assert_eq!(entries.len(), 3);

    for seeded in [&a, &b, &c] {
        assert!(
            scratch.groups().get(&seeded.group_id).await.is_some(),
            "missing entry for {}",
            seeded.group_id.as_uuid()
        );
    }
}

#[tokio::test]
async fn broken_repo_directory_is_skipped() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    // Drop a directory named like a bare repo but containing no
    // actual git state. The scanner should log the open failure
    // and skip it rather than erroring the whole index.
    let broken_uuid = Uuid::now_v7();
    let broken_dir = scratch.repos_root().join(format!("{broken_uuid}.git"));
    std::fs::create_dir_all(&broken_dir).expect("mkdir broken repo");

    // Also seed a healthy neighbour so we can prove the scan
    // continues past the broken repo.
    let healthy = scratch.seed_group("healthy").await.expect("seed healthy");

    // Rebuild the index against the same repos root to exercise the
    // first-time walk over the manually-dropped broken directory
    // (the fixture's index has already indexed the healthy repo via
    // `refresh`, so a plain refresh wouldn't rescan the broken one).
    let index = GroupIndex::build(scratch.repos_root(), scratch.backend().clone())
        .await
        .expect("build");
    let entries = index.list().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].handle.group_id,
        *healthy.group_id.as_uuid(),
        "only the healthy repo should survive the scan"
    );
}

#[tokio::test]
async fn refresh_picks_up_newly_created_group() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    assert!(scratch.groups().list().await.is_empty());

    let seeded = scratch.seed_group("team-rust").await.expect("seed");

    // `seed_group` already called `refresh` internally, so the new
    // entry is expected to be observable without further work.
    assert_eq!(scratch.groups().list().await.len(), 1);
    assert!(scratch.groups().get(&seeded.group_id).await.is_some());
}
