#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Exercises the cache's process-global write-trigger hook
//! end-to-end: [`mmcp_store::cache::init_from_home`] followed by a
//! real memory write through [`mmcp_store::memory::write_file_at_path`]
//! must land a row in the active cache pool without any explicit
//! rebuild call.
//!
//! Deliberately its own integration-test binary (cargo compiles
//! every file under `tests/` as a separate process) rather than a
//! `#[cfg(test)]` module inside `src/cache/`: the hook reads a
//! process-global `OnceLock`, so exercising it must not share a
//! process with any other test that might also call
//! `init_from_home` against a different scratch home.

use mmcp_core::memory::MemoryFile;
use mmcp_store::cache;
use mmcp_store::memory::{WriteFileOptions, write_file_at_path};
use mmcp_store::testing::ScratchHome;

#[tokio::test]
async fn write_file_at_path_indexes_through_the_global_hook() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    cache::init_from_home(scratch.home())
        .await
        .expect("init_from_home");

    let seeded = scratch
        .seed_group("write-trigger-test")
        .await
        .expect("seed group");
    let entry = scratch
        .groups()
        .get(&seeded.group_id)
        .await
        .expect("seeded group present");

    let source = "+++\nname = \"otter-notes\"\ndescription = \"about otters\"\nkind = \"scratch\"\n+++\n\notters are semiaquatic mammals\n";
    let file = MemoryFile::parse(source).expect("parse sample memory");
    let rendered = file.to_string().expect("render memory file");
    let id = mmcp_core::id::MemoryId::new();
    let path = mmcp_core::conventions::memory_path("otter-notes", id);

    write_file_at_path(
        scratch.backend(),
        &entry.handle,
        &path,
        &rendered,
        scratch.author(),
        WriteFileOptions::default(),
    )
    .await
    .expect("write sample memory");

    // No explicit rebuild, no explicit upsert call: the write
    // itself must have driven the row into the active pool via the
    // `notify_write` hook wired into `write_file_at_path`.
    let pool = cache::active_pool().expect("active pool was initialised above");
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory WHERE slug = ?")
        .bind("otter-notes")
        .fetch_one(&pool)
        .await
        .expect("count rows");
    assert_eq!(row.0, 1);
}
