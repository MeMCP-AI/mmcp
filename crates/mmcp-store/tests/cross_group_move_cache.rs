#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Exercises the cache's cross-group-move trigger hook end-to-end:
//! [`mmcp_store::cache::init_from_home`] followed by a real
//! [`mmcp_store::move_memory_across_groups`] call must leave exactly
//! one row for the moved memory's id in the active cache pool,
//! against the target group, never the source.
//!
//! Its own integration-test binary, same rationale as
//! `cache_write_trigger.rs`: the hook reads a process-global
//! `OnceLock`.

use mmcp_core::memory::MemoryFile;
use mmcp_core::memory::{FrontmatterFormat, MemoryFrontmatter, MemoryKind};
use mmcp_store::cache;
use mmcp_store::memory::import_memory;
use mmcp_store::testing::ScratchHome;
use mmcp_store::{CrossGroupMoveOptions, move_memory_across_groups};

#[tokio::test]
async fn move_leaves_exactly_one_cache_row_in_the_target_group() {
    let scratch = ScratchHome::new().await.expect("scratch home");
    cache::init_from_home(scratch.home())
        .await
        .expect("init_from_home");

    let source = scratch
        .seed_group("cache-move-source")
        .await
        .expect("seed source");
    let target = scratch
        .seed_group("cache-move-target")
        .await
        .expect("seed target");
    let source_entry = scratch
        .groups()
        .get(&source.group_id)
        .await
        .expect("source entry");
    let target_entry = scratch
        .groups()
        .get(&target.group_id)
        .await
        .expect("target entry");

    let file = MemoryFile {
        frontmatter: MemoryFrontmatter::new(
            "cache-tracked",
            "moves across groups",
            MemoryKind::Scratch,
        ),
        body: "tracked by the cache\n".to_string(),
        format: FrontmatterFormat::TomlPlus,
    };
    let seeded = import_memory(
        scratch.backend(),
        &source_entry.handle,
        "cache-tracked",
        &file.to_string().expect("render"),
        None,
        scratch.author(),
        false,
    )
    .await
    .expect("seed source memory");

    let pool = cache::active_pool().expect("active pool was initialised above");
    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM indexed_memory WHERE id = ?")
        .bind(seeded.id.to_string())
        .fetch_one(&pool)
        .await
        .expect("count rows before move");
    assert_eq!(
        before.0, 1,
        "the write-trigger hook must have indexed the source row"
    );

    move_memory_across_groups(
        scratch.backend(),
        &source_entry,
        &target_entry,
        None,
        Some(seeded.id),
        scratch.author(),
        CrossGroupMoveOptions::default(),
    )
    .await
    .expect("move across groups");

    let rows: Vec<(String,)> = sqlx::query_as("SELECT group_id FROM indexed_memory WHERE id = ?")
        .bind(seeded.id.to_string())
        .fetch_all(&pool)
        .await
        .expect("count rows after move");
    assert_eq!(
        rows.len(),
        1,
        "exactly one group must hold the memory's cache row after the move"
    );
    assert_eq!(rows[0].0, target.group_id.as_uuid().to_string());
}
