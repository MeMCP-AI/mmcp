//! Exercises the cache's pull-trigger hook,
//! [`mmcp_store::cache::notify_pull`], end-to-end.
//!
//! Its own integration-test binary for the same reason
//! `cache_write_trigger.rs` is: the hook reads a process-global
//! `OnceLock`, so it must not share a process with another test
//! that also calls `init_from_home`.

use mmcp_core::memory::MemoryFile;
use mmcp_git::{CommitSpec, GitBackend};
use mmcp_store::cache;
use mmcp_store::testing::ScratchHome;

#[tokio::test]
async fn notify_pull_reindexes_exactly_the_named_groups() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    cache::init_from_home(scratch.home())
        .await
        .expect("init_from_home");

    let seeded = scratch
        .seed_group("pull-trigger-test")
        .await
        .expect("seed group");
    let entry = scratch
        .groups()
        .get(&seeded.group_id)
        .await
        .expect("seeded group present");

    // First build so the index is marked "built" -- `rebuild_groups`
    // (and therefore `notify_pull`) is a documented no-op against an
    // index that has never completed its first build.
    let pool = cache::active_pool().expect("active pool");
    cache::rebuild_full(&pool, scratch.backend(), scratch.groups())
        .await
        .expect("initial rebuild_full");

    // Simulate a server pull landing new content: write a memory
    // directly through the git backend, bypassing
    // `write_file_at_path` entirely, so the write-trigger hook never
    // fires and the only way this row can appear is the pull-trigger
    // hook under test.
    let source = "+++\nname = \"pulled-memory\"\ndescription = \"landed via a simulated pull\"\nkind = \"scratch\"\n+++\n\ncontent that arrived from the remote\n";
    let file = MemoryFile::parse(source).expect("parse");
    let rendered = file.to_string().expect("render");
    let id = mmcp_core::id::MemoryId::new();
    let path = mmcp_core::conventions::memory_path("pulled-memory", id);
    scratch
        .backend()
        .write_commit(
            &entry.handle,
            CommitSpec::mmcp_commit(
                "simulate a pulled commit".to_string(),
                vec![(path, Some(rendered.into_bytes()))],
                &scratch.author().name,
                &scratch.author().email,
            ),
        )
        .await
        .expect("simulate pulled commit");

    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory WHERE slug = ?")
        .bind("pulled-memory")
        .fetch_one(&pool)
        .await
        .expect("count before");
    assert_eq!(before.0, 0, "not indexed until the pull-trigger runs");

    cache::notify_pull(
        scratch.backend(),
        scratch.groups(),
        &[*seeded.group_id.as_uuid()],
    )
    .await;

    let after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory WHERE slug = ?")
        .bind("pulled-memory")
        .fetch_one(&pool)
        .await
        .expect("count after");
    assert_eq!(after.0, 1, "pull-trigger must index the simulated pull");
}
