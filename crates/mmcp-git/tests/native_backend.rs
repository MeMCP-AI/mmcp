//! Integration tests for the native git backend.

use mmcp_core::id::GroupId;
use mmcp_core::manifest::{GroupManifest, MANIFEST_FILENAME};
use mmcp_git::{CommitSpec, FastForwardOutcome, GitBackend, NativeBackend, Rev};
use tempfile::TempDir;
use uuid::Uuid;

fn backend_in_tempdir() -> (NativeBackend, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let backend = NativeBackend::new(tmp.path()).expect("backend");
    (backend, tmp)
}

fn sample_manifest() -> GroupManifest {
    GroupManifest::new_user_owned(GroupId::new(), "g", Uuid::now_v7())
}

fn sample_commit(author: &str, branch: &str, file: &str, contents: &str) -> CommitSpec {
    CommitSpec {
        branch: branch.to_string(),
        author_name: author.to_string(),
        author_email: format!("{author}@example.com"),
        message: format!("add {file}"),
        files: vec![(file.to_string(), Some(contents.as_bytes().to_vec()))],
    }
}

#[tokio::test]
async fn create_group_repo_is_idempotent() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();

    let handle_a = backend.create_group_repo(&manifest).await.unwrap();
    let handle_b = backend.create_group_repo(&manifest).await.unwrap();
    assert_eq!(handle_a, handle_b);
}

#[tokio::test]
async fn create_group_repo_writes_initial_manifest() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let handle = backend.create_group_repo(&manifest).await.unwrap();

    // The repo should carry a readable manifest on HEAD.
    let loaded = backend.read_manifest(&handle).await.unwrap();
    assert_eq!(loaded, manifest);

    // And the raw file should be at the conventional path.
    let bytes = backend
        .read_file(&handle, MANIFEST_FILENAME, &Rev::Branch("main".into()))
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("schema_version"));
}

#[tokio::test]
async fn init_bare_pins_head_to_main() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let _handle = backend.create_group_repo(&manifest).await.unwrap();

    // HEAD is pinned to `main` regardless of the host init.defaultBranch.
    let head =
        std::fs::read_to_string(backend.repo_path(*manifest.group_id.as_uuid()).join("HEAD"))
            .unwrap();
    assert_eq!(head.trim(), "ref: refs/heads/main");
}

#[tokio::test]
async fn head_read_falls_back_when_default_branch_mismatches() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let handle = backend.create_group_repo(&manifest).await.unwrap();

    // Simulate a repo whose HEAD points at an unborn `master` (created
    // with init.defaultBranch=master) while mmcp committed to `main`.
    std::fs::write(
        backend.repo_path(*manifest.group_id.as_uuid()).join("HEAD"),
        b"ref: refs/heads/master\n",
    )
    .unwrap();

    // Rev::Head must still resolve via the `main` fallback.
    let loaded = backend.read_manifest(&handle).await.unwrap();
    assert_eq!(loaded.group_id, manifest.group_id);
}

#[tokio::test]
async fn write_manifest_overwrites_existing_manifest() {
    let (backend, _tmp) = backend_in_tempdir();
    let original = sample_manifest();
    let handle = backend.create_group_repo(&original).await.unwrap();

    let mut renamed = original.clone();
    renamed.slug = "renamed".to_string();
    backend.write_manifest(&handle, &renamed).await.unwrap();

    let loaded = backend.read_manifest(&handle).await.unwrap();
    assert_eq!(loaded.slug, "renamed");
    assert_eq!(loaded.group_id, original.group_id);
}

#[tokio::test]
async fn write_commit_and_read_file_round_trip() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    let commit_id = backend
        .write_commit(
            &repo,
            sample_commit(
                "alice",
                "main",
                "memories/hello.md",
                "+++\nname = \"hello\"\ndescription = \"h\"\nkind = \"rule\"\n+++\n# Hello\n",
            ),
        )
        .await
        .unwrap();
    assert_eq!(commit_id.len(), 40);

    let bytes = backend
        .read_file(&repo, "memories/hello.md", &Rev::Branch("main".into()))
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("# Hello"));
}

#[tokio::test]
async fn write_commit_twice_advances_branch() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    let first = backend
        .write_commit(&repo, sample_commit("alice", "main", "a.md", "first"))
        .await
        .unwrap();
    let second = backend
        .write_commit(&repo, sample_commit("alice", "main", "b.md", "second"))
        .await
        .unwrap();
    assert_ne!(first, second);

    let b = backend
        .read_file(&repo, "b.md", &Rev::Branch("main".into()))
        .await
        .unwrap();
    assert_eq!(&b[..], b"second");
}

#[tokio::test]
async fn tag_points_at_commit() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    let commit_id = backend
        .write_commit(&repo, sample_commit("alice", "main", "x.md", "x"))
        .await
        .unwrap();

    backend.tag(&repo, "v0.1.0", &commit_id).await.unwrap();

    let via_tag = backend
        .read_file(&repo, "x.md", &Rev::Tag("v0.1.0".into()))
        .await
        .unwrap();
    let via_commit = backend
        .read_file(&repo, "x.md", &Rev::Commit(commit_id))
        .await
        .unwrap();
    assert_eq!(via_tag, via_commit);
}

#[tokio::test]
async fn read_missing_file_returns_path_not_found() {
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    backend
        .write_commit(&repo, sample_commit("alice", "main", "exists.md", "yes"))
        .await
        .unwrap();

    let err = backend
        .read_file(&repo, "missing.md", &Rev::Branch("main".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, mmcp_git::GitError::PathNotFound(_)));
}

/// End-to-end round trip: push from one `NativeBackend` to a bare
/// `file://` remote, then fetch from the remote into a second
/// `NativeBackend`, and check that the commit id moved across.
///
/// This is the "any git forge" smoke test. The `file://` transport
/// uses the same smart-protocol code path as HTTPS/SSH against stock
/// git hosts, so a passing round-trip here means the outbound side
/// of the workspace holds up against any git-compatible endpoint,
/// not just mmcp-server.
#[tokio::test]
async fn push_and_fetch_round_trip_through_file_url() {
    // Source backend: create a group repo and commit some content.
    let (backend_src, _tmp_src) = backend_in_tempdir();
    let manifest = sample_manifest();
    let handle_src = backend_src.create_group_repo(&manifest).await.unwrap();
    let commit_id = backend_src
        .write_commit(
            &handle_src,
            sample_commit("alice", "main", "memories/a.md", "hello"),
        )
        .await
        .unwrap();

    // Bare remote acting as a neutral git host. `gix::init_bare`
    // creates the same shape any forge would expose, so the
    // refspecs and transport below are exercised end-to-end.
    let remote_tmp = TempDir::new().expect("remote tempdir");
    let remote_path = remote_tmp.path().join("bare.git");
    gix::init_bare(&remote_path).expect("init bare remote");
    let remote_url = format!(
        "file://{}",
        remote_path.to_string_lossy().replace('\\', "/")
    );

    // Destination backend: a fresh mirror with its own group repo
    // so fetch writes into a distinct filesystem tree.
    let (backend_dst, _tmp_dst) = backend_in_tempdir();
    let handle_dst = backend_dst.create_group_repo(&manifest).await.unwrap();

    let refs = vec![mmcp_git::RefSpec::new(
        "refs/heads/main",
        "refs/heads/main",
    )
    .forced()];
    let creds = mmcp_git::Credentials::None;

    // Push source → remote. Forced to overwrite the manifest commit
    // the destination tempdir's bootstrap planted on its own `main`.
    backend_src
        .push(&handle_src, &remote_url, &refs, &creds)
        .await
        .expect("push to file:// remote");

    // Fetch remote → destination. Force-update the local main so
    // the remote's tip replaces the destination's bootstrap commit.
    let fetch_refs = vec![
        mmcp_git::RefSpec::new("refs/heads/main", "refs/heads/main").forced(),
    ];
    backend_dst
        .fetch(&handle_dst, &remote_url, &fetch_refs, &creds)
        .await
        .expect("fetch from file:// remote");

    // The destination repo should now contain the source commit.
    let bytes = backend_dst
        .read_file(&handle_dst, "memories/a.md", &Rev::Branch("main".into()))
        .await
        .expect("read file fetched from remote");
    assert_eq!(bytes.as_ref(), b"hello");

    // Spot-check the commit id matches what we pushed.
    let history = backend_dst
        .walk_history(&handle_dst, "memories/a.md")
        .await
        .expect("walk history");
    assert!(
        history.iter().any(|c| c.id == commit_id),
        "fetched history missing commit {commit_id}; saw: {:?}",
        history.iter().map(|c| &c.id).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn remote_operations_fail_fast_without_a_remote() {
    // `NativeBackend::fetch` and `push` shell out to the user's git
    // binary. When the remote URL points at nothing, the subprocess
    // exits non-zero and the backend returns a `Gix` error carrying
    // git's stderr. A full content-plane round-trip is covered by
    // the integration suite in phase 9.
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    let creds = mmcp_git::Credentials::None;
    let err = backend
        .fetch(&repo, "http://127.0.0.1:1/no-such.git", &[], &creds)
        .await
        .unwrap_err();
    assert!(matches!(err, mmcp_git::GitError::Transport { .. }));
    let err = backend
        .push(&repo, "http://127.0.0.1:1/no-such.git", &[], &creds)
        .await
        .unwrap_err();
    assert!(matches!(err, mmcp_git::GitError::Transport { .. }));
}

#[tokio::test]
async fn fast_forward_advances_local_ref_to_target_commit() {
    // Seed a repo with an initial manifest commit (creates main),
    // then commit a second blob to a synthetic `refs/remotes/origin/main`
    // tracking ref and run fast_forward to advance main. The
    // outcome should be `Advanced { from: Some(parent), to: child }`.
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    // Parent commit: the manifest commit that `create_group_repo`
    // already wrote. Read its id off `main`.
    let parent_bytes = backend
        .read_file(&repo, MANIFEST_FILENAME, &Rev::Branch("main".into()))
        .await
        .unwrap();
    assert!(!parent_bytes.is_empty());

    // Child commit on main: adds a file.
    let child_id = backend
        .write_commit(
            &repo,
            sample_commit("alice", "main", "hello.md", "hi"),
        )
        .await
        .unwrap();

    // Simulate what fetch-to-tracking-ref would produce: point a
    // new ref `refs/remotes/origin/main` at child_id, then rewind
    // local `refs/heads/main` to the parent so the two refs have
    // something to differ on.
    let tmp_path = _tmp.path().join(format!("{}.git", manifest.group_id));
    let gix_repo = gix::open(&tmp_path).unwrap();
    let child_obj = gix::ObjectId::from_hex(child_id.as_bytes()).unwrap();

    // Create the tracking ref at child.
    gix_repo
        .reference(
            "refs/remotes/origin/main",
            child_obj,
            gix::refs::transaction::PreviousValue::MustNotExist,
            "seed tracking ref",
        )
        .unwrap();

    // Walk back one commit to find the parent so we can rewind local.
    let parent_id = {
        let commit = gix_repo.find_object(child_obj).unwrap().into_commit();
        let decoded = commit.decode().unwrap();
        decoded.parents().next().expect("child has a parent")
    };
    let mut local_main = gix_repo.find_reference("refs/heads/main").unwrap();
    local_main
        .set_target_id(parent_id, "rewind for FF test")
        .unwrap();

    let outcome = backend
        .fast_forward(&repo, "refs/heads/main", "refs/remotes/origin/main")
        .await
        .unwrap();
    match outcome {
        FastForwardOutcome::Advanced { from, to } => {
            assert_eq!(from, Some(parent_id.to_string()));
            assert_eq!(to, child_id);
        }
        other => panic!("expected Advanced, got {other:?}"),
    }
}

#[tokio::test]
async fn fast_forward_on_equal_refs_reports_already_at() {
    // When local and tracking ref already point at the same commit,
    // fast_forward returns `AlreadyAt` and leaves the ref alone.
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    let tmp_path = _tmp.path().join(format!("{}.git", manifest.group_id));
    let gix_repo = gix::open(&tmp_path).unwrap();
    let main_id = gix_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .id()
        .detach();
    gix_repo
        .reference(
            "refs/remotes/origin/main",
            main_id,
            gix::refs::transaction::PreviousValue::MustNotExist,
            "seed tracking ref",
        )
        .unwrap();

    let outcome = backend
        .fast_forward(&repo, "refs/heads/main", "refs/remotes/origin/main")
        .await
        .unwrap();
    match outcome {
        FastForwardOutcome::AlreadyAt { commit } => {
            assert_eq!(commit, main_id.to_string());
        }
        other => panic!("expected AlreadyAt, got {other:?}"),
    }
}

#[tokio::test]
async fn fast_forward_reports_not_fast_forward_on_divergence() {
    // Seed two sibling commits off the same parent (one on main,
    // one on a fake tracking ref). Fast-forwarding main to the
    // tracking ref should report NotFastForward since neither is
    // an ancestor of the other.
    let (backend, _tmp) = backend_in_tempdir();
    let manifest = sample_manifest();
    let repo = backend.create_group_repo(&manifest).await.unwrap();

    // Advance main with one commit (the "local" side).
    let local_child = backend
        .write_commit(&repo, sample_commit("alice", "main", "local.md", "local"))
        .await
        .unwrap();

    // Now rewind main to the initial manifest commit, so we can
    // create a sibling commit off that parent.
    let tmp_path = _tmp.path().join(format!("{}.git", manifest.group_id));
    let gix_repo = gix::open(&tmp_path).unwrap();
    let local_child_obj = gix::ObjectId::from_hex(local_child.as_bytes()).unwrap();
    let parent_id = {
        let commit = gix_repo.find_object(local_child_obj).unwrap().into_commit();
        let decoded = commit.decode().unwrap();
        decoded.parents().next().expect("child has a parent")
    };
    gix_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .set_target_id(parent_id, "rewind for sibling")
        .unwrap();

    // Now write a different commit on main (the "remote" sibling).
    let remote_child = backend
        .write_commit(&repo, sample_commit("bob", "main", "remote.md", "remote"))
        .await
        .unwrap();
    let remote_child_obj = gix::ObjectId::from_hex(remote_child.as_bytes()).unwrap();

    // Park the remote sibling on the tracking ref and restore local
    // main to the local child so the two refs diverge.
    gix_repo
        .reference(
            "refs/remotes/origin/main",
            remote_child_obj,
            gix::refs::transaction::PreviousValue::MustNotExist,
            "seed tracking ref",
        )
        .unwrap();
    gix_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .set_target_id(local_child_obj, "restore local main")
        .unwrap();

    let outcome = backend
        .fast_forward(&repo, "refs/heads/main", "refs/remotes/origin/main")
        .await
        .unwrap();
    match outcome {
        FastForwardOutcome::NotFastForward { local, target } => {
            assert_eq!(local, local_child);
            assert_eq!(target, remote_child);
        }
        other => panic!("expected NotFastForward, got {other:?}"),
    }
}
