//! Seed a test group with two memories in a TEMPORARY directory.
//!
//! This example is for development testing ONLY. It never touches
//! the real ~/.mmcp/repos/ directory.
//!
//! Usage: cargo run --package mmcp-git --example seed_test_group

use mmcp_core::id::GroupId;
use mmcp_core::manifest::GroupManifest;
use mmcp_git::{GitBackend, NativeBackend};
use mmcp_git::types::CommitSpec;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let backend = NativeBackend::new(tmp.path()).expect("init repos root");

    let group_id = GroupId::new();
    let owner = Uuid::now_v7();
    let manifest = GroupManifest::new_user_owned(group_id, "test-offline", owner);
    let handle = backend.create_group_repo(&manifest).await.expect("create group");
    println!("Created group: {} (slug: test-offline)", group_id.as_uuid());
    println!("Temp dir: {}", tmp.path().display());

    let mem1_content = r#"+++
name = "always-use-result"
description = "Enforce Result return types for all fallible functions"
kind = "rule"
mandatory = true
tags = ["rust", "error-handling"]
+++

Always return `Result` from fallible functions. Never use `.unwrap()` in
library code - reserve it for tests and examples.
"#;
    backend.write_commit(&handle, CommitSpec {
        branch: "main".to_string(),
        author_name: "mmcp-seed".to_string(),
        author_email: "seed@mmcp.invalid".to_string(),
        message: "add always-use-result memory".to_string(),
        files: vec![
            ("memories/always-use-result.md".to_string(), Some(mem1_content.as_bytes().to_vec())),
        ],
    }).await.expect("write memory 1");
    println!("Wrote memory: always-use-result");

    let mem2_content = r#"+++
name = "offline-test-note"
description = "Seed memory for verifying offline MCP tool reads"
kind = "reference"
mandatory = false
tags = ["test", "offline"]
+++

This memory was created by the seed script for offline testing.
It verifies that the mmcp MCP tools can read real content from git.
"#;
    backend.write_commit(&handle, CommitSpec {
        branch: "main".to_string(),
        author_name: "mmcp-seed".to_string(),
        author_email: "seed@mmcp.invalid".to_string(),
        message: "add offline-test-note memory".to_string(),
        files: vec![
            ("memories/offline-test-note.md".to_string(), Some(mem2_content.as_bytes().to_vec())),
        ],
    }).await.expect("write memory 2");
    println!("Wrote memory: offline-test-note");

    println!("\nDone. Temp dir will be cleaned up on exit.");
    println!("To inspect: run before this process exits, or use --nocapture.");
}
