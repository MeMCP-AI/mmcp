#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real CLI/Git regression for moving a configured project to an empty MMCP home.
use assert_cmd::Command;
use mmcp_core::config::{ProjectConfig, Remote, RemoteAuth};
use mmcp_core::id::ProjectUuid;
use mmcp_core::memory::MemoryKind;
use mmcp_git::{GitBackend, Rev};
use mmcp_store::MmcpHome;
use mmcp_store::memory::{SynthFrontmatter, import_memory, resolve_memory};
use mmcp_store::testing::ScratchHome;

#[tokio::test]
async fn fresh_device_pull_by_uuid_slug_scope_and_all_imports_memory_and_repeats() {
    let source = ScratchHome::new().await.unwrap();
    let seeded = source.seed_group("portable-project").await.unwrap();
    let entry = source.groups().get(&seeded.group_id).await.unwrap();
    import_memory(
        source.backend(),
        &entry.handle,
        "portable-rule",
        "Keep the portable memory.\n",
        Some(SynthFrontmatter {
            name: "Portable rule".into(),
            description: "Portability regression".into(),
            kind: MemoryKind::Rule,
        }),
        source.author(),
        false,
    )
    .await
    .unwrap();
    let remote = source.backend().repo_path(*seeded.group_id.as_uuid());
    for (case, (selector, declared_slug)) in [
        (
            vec!["--group".to_owned(), seeded.group_id.to_string()],
            false,
        ),
        (
            vec!["--group".to_owned(), "portable-project".to_owned()],
            true,
        ),
        (vec!["--scope".to_owned(), "project".to_owned()], false),
        (vec!["--all".to_owned()], false),
    ]
    .into_iter()
    .enumerate()
    {
        let project = tempfile::tempdir().unwrap();
        let home = MmcpHome::from_root(project.path().join("fresh-home"));
        let mut config = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(*seeded.group_id.as_uuid()),
            project_slug: Some("portable-project".into()),
            sync: Default::default(),
            project_remote_only: false,
            subscriptions: Default::default(),
        };
        config.sync.remotes.push(Remote::DirectGit {
            name: "portable".into(),
            url: remote.to_string_lossy().into_owned(),
            auth: RemoteAuth::None,
            group: declared_slug.then(|| "portable-project".into()),
            default: true,
            include_in_push_all: true,
        });
        mmcp_store::config::save(project.path(), &config).unwrap();
        let updated_slug = format!("portable-update-{case}");
        for attempt in 0..3 {
            if attempt == 1 {
                import_memory(
                    source.backend(),
                    &entry.handle,
                    &updated_slug,
                    "An update after first clone.\n",
                    Some(SynthFrontmatter {
                        name: "Later update".into(),
                        description: "Fast-forward regression".into(),
                        kind: MemoryKind::Rule,
                    }),
                    source.author(),
                    false,
                )
                .await
                .unwrap();
            }
            Command::cargo_bin("mmcp")
                .unwrap()
                .arg("pull")
                .args(&selector)
                .current_dir(project.path())
                .env("MMCP_HOME", project.path().join("fresh-home"))
                .assert()
                .success();
        }
        let (backend, groups) = home.init_backend().await.unwrap();
        let imported = groups
            .get(&seeded.group_id)
            .await
            .expect("cloned group indexed");
        let memory = resolve_memory(&backend, &imported.handle, Some("portable-rule"), None)
            .await
            .unwrap();
        let contents = backend
            .read_file(&imported.handle, &memory.path, &Rev::head())
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&contents).contains("Keep the portable memory."));
        resolve_memory(&backend, &imported.handle, Some(&updated_slug), None)
            .await
            .expect("second pull must fast-forward a new remote memory");
        assert!(
            backend
                .repo_path(*seeded.group_id.as_uuid())
                .join("HEAD")
                .exists(),
            "mirror must be bare"
        );
    }
}

#[tokio::test]
async fn invalid_pull_selectors_do_not_import_configured_group() {
    let source = ScratchHome::new().await.unwrap();
    let seeded = source.seed_group("portable-project").await.unwrap();
    let project = tempfile::tempdir().unwrap();
    let home = project.path().join("fresh-home");
    let mut config = ProjectConfig {
        project_uuid: ProjectUuid::from_uuid(*seeded.group_id.as_uuid()),
        project_slug: None,
        sync: Default::default(),
        project_remote_only: false,
        subscriptions: Default::default(),
    };
    config.sync.remotes.push(Remote::DirectGit {
        name: "portable".into(),
        url: source
            .backend()
            .repo_path(*seeded.group_id.as_uuid())
            .to_string_lossy()
            .into_owned(),
        auth: RemoteAuth::None,
        group: None,
        default: true,
        include_in_push_all: true,
    });
    mmcp_store::config::save(project.path(), &config).unwrap();
    for args in [
        vec!["pull"],
        vec!["pull", "--all", "--group", "portable-project"],
        vec!["pull", "--group", "unrelated"],
        vec!["fetch", "--all"],
        vec!["push", "--all"],
    ] {
        Command::cargo_bin("mmcp")
            .unwrap()
            .args(args)
            .current_dir(project.path())
            .env("MMCP_HOME", &home)
            .assert()
            .failure();
        let (_, groups) = MmcpHome::from_root(home.clone())
            .init_backend()
            .await
            .unwrap();
        assert!(groups.get(&seeded.group_id).await.is_none());
    }
}

#[tokio::test]
async fn fresh_device_sync_preserves_all_and_named_push_remotes() {
    for (push_args, expected_pushed) in [
        (vec!["--all-remotes"], 2),
        (vec!["--remote", "secondary"], 1),
    ] {
        let source = ScratchHome::new().await.unwrap();
        let seeded = source.seed_group("portable-sync").await.unwrap();
        let primary_path = source.backend().repo_path(*seeded.group_id.as_uuid());
        let secondary = tempfile::tempdir().unwrap();
        let secondary_path = secondary.path().join("secondary.git");
        let entry = source.groups().get(&seeded.group_id).await.unwrap();
        import_memory(
            source.backend(),
            &entry.handle,
            "new-remote-rule",
            "Send this rule to both configured remotes.\n",
            Some(SynthFrontmatter {
                name: "New rule".into(),
                description: "Remote retention regression".into(),
                kind: MemoryKind::Rule,
            }),
            source.author(),
            false,
        )
        .await
        .unwrap();
        // Equal remote heads isolate bootstrap and push selection from fetch conflicts.
        source
            .backend()
            .clone_bare_to(
                &primary_path.to_string_lossy(),
                &secondary_path,
                &mmcp_git::Credentials::None,
            )
            .await
            .unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut config = ProjectConfig {
            project_uuid: ProjectUuid::from_uuid(*seeded.group_id.as_uuid()),
            project_slug: None,
            sync: Default::default(),
            project_remote_only: false,
            subscriptions: Default::default(),
        };
        for (name, path, default) in [
            ("primary", &primary_path, true),
            ("secondary", &secondary_path, false),
        ] {
            config.sync.remotes.push(Remote::DirectGit {
                name: name.into(),
                url: path.to_string_lossy().into_owned(),
                auth: RemoteAuth::None,
                group: None,
                default,
                include_in_push_all: true,
            });
        }
        mmcp_store::config::save(project.path(), &config).unwrap();
        Command::cargo_bin("mmcp")
            .unwrap()
            .args(["sync", "--group", &seeded.group_id.to_string()])
            .args(push_args)
            .current_dir(project.path())
            .env("MMCP_HOME", project.path().join("fresh-home"))
            .assert()
            .success()
            .stdout(predicates::str::contains(format!(
                "pushed {expected_pushed} groups"
            )));
    }
}
