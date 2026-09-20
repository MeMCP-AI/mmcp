#![allow(clippy::unwrap_used, clippy::expect_used)]
//! First pull adopts only a validated default remote into a fresh local mirror.

use std::sync::Arc;

use mmcp_core::config::{Remote, RemoteAuth};
use mmcp_core::id::GroupId;
use mmcp_core::manifest::{GroupScope, MANIFEST_FILENAME};
use mmcp_git::{CommitSpec, GitBackend, RepoHandle, Rev};
use mmcp_store::sync::{EffectiveRemotes, PullSelector, RemoteLevel, ResolvedRemote, prepare_pull};
use mmcp_store::testing::{ScratchHome, SeededGroup};

fn remote(home: &ScratchHome, group: &SeededGroup, reference: String) -> ResolvedRemote {
    ResolvedRemote {
        remote: Remote::DirectGit {
            name: "primary".into(),
            url: format!(
                "file://{}",
                home.backend()
                    .repo_path(*group.group_id.as_uuid())
                    .to_string_lossy()
                    .replace('\\', "/")
            ),
            auth: RemoteAuth::None,
            group: Some(reference.clone()),
            default: true,
            include_in_push_all: true,
        },
        level: RemoteLevel::Project,
        direct_git_group: Some(reference),
    }
}

fn effective(remote: ResolvedRemote) -> EffectiveRemotes {
    EffectiveRemotes {
        remotes: vec![remote],
        default_index: Some(0),
    }
}

fn assert_empty(home: &ScratchHome) {
    assert_eq!(
        std::fs::read_dir(home.repos_root()).unwrap().count(),
        0,
        "no repository or staging directory should remain"
    );
}

async fn commit(home: &ScratchHome, group: &SeededGroup, path: &str, contents: &str) -> String {
    let handle = RepoHandle::new(
        *group.group_id.as_uuid(),
        home.backend()
            .repo_path(*group.group_id.as_uuid())
            .to_string_lossy()
            .into_owned(),
    );
    home.backend()
        .write_commit(
            &handle,
            CommitSpec::mmcp_commit(
                "remote update",
                vec![(path.into(), Some(contents.as_bytes().to_vec()))],
                "test",
                "test@example.invalid",
            ),
        )
        .await
        .unwrap()
}

async fn pull(home: &ScratchHome, remotes: &EffectiveRemotes, selector: PullSelector) {
    let (engine, resolver, filter) = prepare_pull(
        Arc::clone(home.backend()),
        home.groups().clone(),
        remotes,
        selector,
    )
    .await
    .unwrap();
    let report = engine.pull(filter, &resolver, &resolver).await.unwrap();
    assert!(
        report.failed.is_empty(),
        "pull failures: {:?}",
        report.failed
    );
    assert!(report.manifest_failures.is_empty());
}

#[tokio::test]
async fn default_adoption_and_repeated_pull_preserve_remote_history() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("portable").await.unwrap();
    let first = commit(&source, &group, "memories/note.md", "first").await;
    let target = ScratchHome::new().await.unwrap();
    let remotes = effective(remote(&source, &group, group.group_id.to_string()));
    // The slug selector must resolve through the UUID-bound remote manifest.
    pull(&target, &remotes, PullSelector::Group("portable".into())).await;
    let entry = target.groups().get(&group.group_id).await.unwrap();
    assert_eq!(entry.manifest, group.manifest);
    assert!(
        target
            .backend()
            .repo_path(*group.group_id.as_uuid())
            .join("HEAD")
            .is_file()
    );
    assert!(
        !target
            .backend()
            .repo_path(*group.group_id.as_uuid())
            .join(".git")
            .exists()
    );
    let second = commit(&source, &group, "memories/note.md", "second").await;
    pull(&target, &remotes, PullSelector::All).await;
    pull(&target, &remotes, PullSelector::All).await;
    let bytes = target
        .backend()
        .read_file(&entry.handle, "memories/note.md", &Rev::Head)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"second");
    let history = target
        .backend()
        .walk_history(&entry.handle, "memories/note.md", None)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|c| c.id == first));
    assert!(history.iter().any(|c| c.id == second));
    assert_eq!(std::fs::read_dir(target.repos_root()).unwrap().count(), 1);
}

#[tokio::test]
async fn mismatched_uuid_and_slug_are_rejected_without_publication() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("actual").await.unwrap();
    for reference in [GroupId::new().to_string(), "wrong-slug".into()] {
        let target = ScratchHome::new().await.unwrap();
        let remotes = effective(remote(&source, &group, reference));
        let result = prepare_pull(
            Arc::clone(target.backend()),
            target.groups().clone(),
            &remotes,
            PullSelector::All,
        )
        .await;
        assert!(result.is_err());
        assert!(target.groups().get(&group.group_id).await.is_none());
        assert_empty(&target);
    }
}

#[tokio::test]
async fn invalid_manifest_is_rejected_without_publication() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("invalid").await.unwrap();
    commit(&source, &group, MANIFEST_FILENAME, "not = [valid TOML").await;
    let target = ScratchHome::new().await.unwrap();
    let remotes = effective(remote(&source, &group, group.group_id.to_string()));
    assert!(
        prepare_pull(
            Arc::clone(target.backend()),
            target.groups().clone(),
            &remotes,
            PullSelector::All
        )
        .await
        .is_err()
    );
    assert_empty(&target);
}

#[tokio::test]
async fn scope_mismatch_leaves_no_repository_and_preserves_unresolved_group_error() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("project-only").await.unwrap();
    assert_eq!(group.manifest.scope, GroupScope::Project);
    let target = ScratchHome::new().await.unwrap();
    let remotes = effective(remote(&source, &group, group.group_id.to_string()));
    let result = prepare_pull(
        Arc::clone(target.backend()),
        target.groups().clone(),
        &remotes,
        PullSelector::Scope(GroupScope::Global),
    )
    .await;
    assert!(matches!(
        result,
        Err(mmcp_store::error::StoreError::DirectGitGroupNotFound { .. })
    ));
    assert_empty(&target);
}

#[tokio::test]
async fn corrupt_existing_destination_is_never_replaced() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("protected-destination").await.unwrap();
    let target = ScratchHome::new().await.unwrap();
    let destination = target.backend().repo_path(*group.group_id.as_uuid());
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("sentinel"), b"keep me").unwrap();
    let remotes = effective(remote(&source, &group, group.group_id.to_string()));
    assert!(
        prepare_pull(
            Arc::clone(target.backend()),
            target.groups().clone(),
            &remotes,
            PullSelector::All
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(destination.join("sentinel")).unwrap(),
        b"keep me"
    );
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(target.repos_root()).unwrap().count(), 1);
}

#[tokio::test]
async fn failed_transport_cleans_staging_and_retry_succeeds() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("retry").await.unwrap();
    let target = ScratchHome::new().await.unwrap();
    let good_remote = remote(&source, &group, group.group_id.to_string());
    let mut bad_remote = good_remote.clone();
    if let Remote::DirectGit { url, .. } = &mut bad_remote.remote {
        url.push_str("-missing");
    }
    assert!(
        prepare_pull(
            Arc::clone(target.backend()),
            target.groups().clone(),
            &effective(bad_remote),
            PullSelector::All
        )
        .await
        .is_err()
    );
    assert_empty(&target);
    pull(&target, &effective(good_remote), PullSelector::All).await;
    assert!(target.groups().get(&group.group_id).await.is_some());
    assert_eq!(std::fs::read_dir(target.repos_root()).unwrap().count(), 1);
}

#[tokio::test]
async fn nondefault_remote_is_not_adopted() {
    let source = ScratchHome::new().await.unwrap();
    let primary = source.seed_group("primary").await.unwrap();
    let secondary = source.seed_group("secondary").await.unwrap();
    let target = ScratchHome::new().await.unwrap();
    let mut secondary_remote = remote(&source, &secondary, secondary.group_id.to_string());
    if let Remote::DirectGit {
        name, default, url, ..
    } = &mut secondary_remote.remote
    {
        *name = "secondary".into();
        *default = false;
        // Unresolved non-default groups retain the original engine validation error.
        url.push_str("-missing");
    }
    let remotes = EffectiveRemotes {
        remotes: vec![
            remote(&source, &primary, primary.group_id.to_string()),
            secondary_remote,
        ],
        default_index: Some(0),
    };
    let result = prepare_pull(
        Arc::clone(target.backend()),
        target.groups().clone(),
        &remotes,
        PullSelector::All,
    )
    .await;
    assert!(matches!(
        result,
        Err(mmcp_store::error::StoreError::DirectGitGroupNotFound { .. })
    ));
    assert!(target.groups().get(&primary.group_id).await.is_some());
    assert!(target.groups().get(&secondary.group_id).await.is_none());
    assert!(
        !target
            .backend()
            .repo_path(*secondary.group_id.as_uuid())
            .exists()
    );
    assert_eq!(std::fs::read_dir(target.repos_root()).unwrap().count(), 1);
}

#[tokio::test]
async fn default_and_nondefault_for_same_group_both_remain_engine_targets() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("shared-target").await.unwrap();
    let target = ScratchHome::new().await.unwrap();
    let primary = remote(&source, &group, group.group_id.to_string());
    let mut secondary = primary.clone();
    if let Remote::DirectGit { name, default, .. } = &mut secondary.remote {
        *name = "secondary".into();
        *default = false;
    }
    let remotes = EffectiveRemotes {
        remotes: vec![primary, secondary],
        default_index: Some(0),
    };
    let (engine, resolver, filter) = prepare_pull(
        Arc::clone(target.backend()),
        target.groups().clone(),
        &remotes,
        PullSelector::All,
    )
    .await
    .unwrap();
    assert_eq!(engine.remotes().len(), 2);
    assert_eq!(engine.remotes()[0].name, "primary");
    assert!(engine.remotes()[0].default);
    assert_eq!(engine.remotes()[1].name, "secondary");
    assert!(!engine.remotes()[1].default);
    let report = engine.pull(filter, &resolver, &resolver).await.unwrap();
    assert!(report.failed.is_empty());
    assert!(report.manifest_failures.is_empty());
    assert_eq!(std::fs::read_dir(target.repos_root()).unwrap().count(), 1);
}

#[tokio::test]
async fn tag_named_main_cannot_replace_the_required_main_branch() {
    let source = ScratchHome::new().await.unwrap();
    let group = source.seed_group("tag-only").await.unwrap();
    let path = source.backend().repo_path(*group.group_id.as_uuid());
    for args in [
        ["tag", "main", "refs/heads/main"],
        ["update-ref", "-d", "refs/heads/main"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let target = ScratchHome::new().await.unwrap();
    let remotes = effective(remote(&source, &group, group.group_id.to_string()));
    let result = prepare_pull(
        Arc::clone(target.backend()),
        target.groups().clone(),
        &remotes,
        PullSelector::All,
    )
    .await;
    assert!(result.is_err());
    assert_empty(&target);
}
