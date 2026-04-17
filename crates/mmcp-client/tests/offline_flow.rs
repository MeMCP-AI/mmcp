//! Integration test: every client primitive works without a remote.
//!
//! Documents and protects the "remoteless standalone" client mode —
//! the operator has neither an mmcp-server nor any git remote
//! configured, and every read/write path against memories must
//! succeed against a local bare repo only. If any of these paths
//! ever starts requiring a remote (e.g. a hidden `/sync/*` call on
//! import, or a diagnose step that tries to reach a sync server),
//! this test will fail and surface the regression.

use std::path::PathBuf;

use mmcp_core::config::{GroupsConfig, LanguagesConfig, ProjectConfig};
use mmcp_core::id::{GroupId, ProjectUuid};
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::MemoryKind;
use mmcp_git::{GitBackend, NativeBackend};
use mmcp_store::diagnostics as health;
use mmcp_store::memory::{self as import, SynthFrontmatter};
use mmcp_store::testing::ScratchHome;
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn import_list_read_health_and_diagnose_run_without_any_remote() {
    let scratch = ScratchHome::new().await.expect("scratch home");

    // The project config explicitly has no [sync] block; this is the
    // canonical shape for a user who has never run `mmcp link`.
    let project_cfg = ProjectConfig {
        project_uuid: ProjectUuid::new(),
        project_slug: None,
        sync: None,
        groups: GroupsConfig::default(),
        languages: LanguagesConfig::default(),
    };
    assert!(
        project_cfg.sync.is_none(),
        "remoteless test must start from a sync-less config"
    );

    // Seed a group locally. No remote URL is involved anywhere.
    let seeded = scratch.seed_group("team-rust").await.expect("seed group");
    let entry = scratch
        .groups()
        .get(&seeded.group_id)
        .await
        .expect("seeded group resolvable");

    // Import a memory — writes stay on local disk via the native backend.
    let result = import::import_memory(
        scratch.backend(),
        &entry.handle,
        "offline-rule",
        "This is the body of an offline-authored memory.\n",
        Some(SynthFrontmatter {
            name: "Offline rule".into(),
            description: "Written with no network".into(),
            kind: MemoryKind::Rule,
        }),
        scratch.author(),
        false,
    )
    .await
    .expect("import succeeds offline");
    assert_eq!(result.slug, "offline-rule");
    assert_eq!(result.commit_id.len(), 40, "real git commit id");

    // Surface checks: manifest + memory must parse without any I/O
    // beyond the local bare repo.
    let surface = health::health_check_all(scratch.backend(), scratch.groups()).await;
    assert_eq!(surface.len(), 1, "exactly one group should be reported");
    let group_surface = &surface[0];
    assert_eq!(group_surface.memory_count, 1, "memory should be visible");
    assert!(
        group_surface.manifest_ok,
        "manifest should parse offline; issues: {:?}",
        group_surface.issues
    );
    assert!(
        group_surface.issues.iter().all(|i| i.severity != "error"),
        "surface check must not raise errors for a valid offline group; got {:?}",
        group_surface.issues
    );

    // Deep checks: same guarantee, plus the project-level issue
    // reporter is allowed to warn about missing sync config but
    // must not fail fast or raise a hard error.
    let diag = health::diagnose_all(scratch.backend(), scratch.groups()).await;
    assert_eq!(diag.groups.len(), 1, "diagnose matches health");
    let has_error = diag
        .groups
        .iter()
        .flat_map(|r| &r.issues)
        .chain(diag.project_issues.iter())
        .any(|i| i.severity == "error");
    assert!(
        !has_error,
        "offline diagnose should produce warnings at most, not errors. project={:?}, group={:?}",
        diag.project_issues, diag.groups[0].issues
    );
}

#[tokio::test]
async fn native_backend_is_usable_with_only_a_local_repo_root() {
    // Smoke test: the whole storage layer must boot and round-trip
    // a file using only filesystem APIs — no DNS, no sockets.
    let tmp = TempDir::new().expect("tempdir");
    let root: PathBuf = tmp.path().to_path_buf();
    let backend = NativeBackend::new(&root).expect("backend without net");
    let manifest = GroupManifest::new_user_owned(GroupId::new(), "solo", Uuid::now_v7());
    let handle = backend
        .create_group_repo(&manifest)
        .await
        .expect("offline create");
    let loaded = backend.read_manifest(&handle).await.expect("offline read");
    assert_eq!(loaded, manifest);
}
