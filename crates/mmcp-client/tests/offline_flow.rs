#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: every client primitive works without a remote.
//!
//! Documents and protects the "remoteless standalone" client mode:
//! the operator has neither an mmcp-server nor any git remote
//! configured, and every read/write path against memories must
//! succeed against a local bare repo only. If any of these paths
//! ever starts requiring a remote (e.g. a hidden `/sync/*` call on
//! import, or a diagnose step that tries to reach a sync server),
//! this test will fail and surface the regression.

use std::path::PathBuf;

use mmcp_core::config::{ProjectConfig, SubscriptionsConfig};
use mmcp_core::id::{GroupId, ProjectUuid, UserId};
use mmcp_core::manifest::GroupManifest;
use mmcp_core::memory::MemoryKind;
use mmcp_git::{GitBackend, NativeBackend, Rev};
use mmcp_store::diagnostics as health;
use mmcp_store::import_adoc::convert_adoc_to_markdown;
use mmcp_store::memory::{self as import, SynthFrontmatter, resolve_memory};
use mmcp_store::testing::ScratchHome;
use tempfile::TempDir;

#[tokio::test]
async fn import_list_read_health_and_diagnose_run_without_any_remote() {
    let scratch = ScratchHome::new().await.expect("scratch home");

    // The project config explicitly has no [sync] block; this is the
    // canonical shape for a user who has never run `mmcp link`.
    let project_cfg = ProjectConfig {
        project_uuid: ProjectUuid::new(),
        project_slug: None,
        sync: Default::default(),
        project_remote_only: false,
        subscriptions: SubscriptionsConfig::default(),
    };
    assert!(
        project_cfg.sync.is_empty(),
        "remoteless test must start from a sync-less config"
    );

    // Seed a group locally. No remote URL is involved anywhere.
    let seeded = scratch.seed_group("team-rust").await.expect("seed group");
    let entry = scratch
        .groups()
        .get(&seeded.group_id)
        .await
        .expect("seeded group resolvable");

    // Import a memory: writes stay on local disk via the native backend.
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
        "manifest should parse offline; findings: {:?}",
        group_surface.findings
    );
    assert!(
        group_surface.findings.iter().all(|i| i.severity != "error"),
        "surface check must not raise errors for a valid offline group; got {:?}",
        group_surface.findings
    );

    // Deep checks: same guarantee, plus the project-level finding
    // reporter is allowed to warn about missing sync config but
    // must not fail fast or raise a hard error.
    let diag = health::diagnose_all(scratch.backend(), scratch.groups()).await;
    assert_eq!(diag.groups.len(), 1, "diagnose matches health");
    let has_error = diag
        .groups
        .iter()
        .flat_map(|r| &r.findings)
        .chain(diag.project_findings.iter())
        .any(|i| i.severity == "error");
    assert!(
        !has_error,
        "offline diagnose should produce warnings at most, not errors. project={:?}, group={:?}",
        diag.project_findings, diag.groups[0].findings
    );
}

#[tokio::test]
async fn adoc_source_round_trips_through_import_as_markdown_memory() {
    // Bridges the adoc source side with the markdown storage side:
    // an operator drops an `.adoc` file into the import path, the
    // pipeline renders it to CommonMark via `acdc`, and `import_memory`
    // lands a regular `.md` memory we can read back and inspect. The
    // stored content is the converted markdown - downstream tooling
    // (search, section editing, diagnostics) never has to know the
    // source was AsciiDoc.
    let scratch = ScratchHome::new().await.expect("scratch home");
    let seeded = scratch.seed_group("team-rust").await.expect("seed group");
    let entry = scratch
        .groups()
        .get(&seeded.group_id)
        .await
        .expect("seeded group resolvable");

    let adoc_source = "= Coding Rules\n\nKeep commits atomic.\n";
    let markdown = convert_adoc_to_markdown(adoc_source).expect("convert adoc");
    assert!(
        markdown.contains("Coding Rules"),
        "heading text must survive the conversion; got:\n{markdown}"
    );

    let result = import::import_memory(
        scratch.backend(),
        &entry.handle,
        "coding-rules",
        &markdown,
        Some(SynthFrontmatter {
            name: "Coding Rules".into(),
            description: "From an adoc source".into(),
            kind: MemoryKind::Rule,
        }),
        scratch.author(),
        false,
    )
    .await
    .expect("import succeeds against adoc-derived markdown");
    assert_eq!(result.slug, "coding-rules");

    // The stored memory must round trip through the native backend,
    // carrying the converted markdown body verbatim plus the synthetic
    // frontmatter fence the importer stamps on.
    let resolved = resolve_memory(scratch.backend(), &entry.handle, Some(&result.slug), None)
        .await
        .expect("resolve imported memory");
    let raw = scratch
        .backend()
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
        .expect("read back stored memory");
    let text = std::str::from_utf8(&raw).expect("utf8 memory");
    assert!(
        text.contains("kind = \"rule\""),
        "frontmatter kind must land in the committed file; got:\n{text}"
    );
    assert!(
        text.contains("Coding Rules"),
        "converted adoc heading must survive into storage; got:\n{text}"
    );
    assert!(
        text.contains("Keep commits atomic."),
        "converted adoc paragraph must survive into storage; got:\n{text}"
    );
}

#[tokio::test]
async fn native_backend_is_usable_with_only_a_local_repo_root() {
    // Smoke test: the whole storage layer must boot and round-trip
    // a file using only filesystem APIs: no DNS, no sockets.
    let tmp = TempDir::new().expect("tempdir");
    let root: PathBuf = tmp.path().to_path_buf();
    let backend = NativeBackend::new(&root).expect("backend without net");
    let manifest = GroupManifest::new_user_owned(GroupId::new(), "solo", UserId::new());
    let handle = backend
        .create_group_repo(&manifest)
        .await
        .expect("offline create");
    let loaded = backend.read_manifest(&handle).await.expect("offline read");
    assert_eq!(loaded, manifest);
}
