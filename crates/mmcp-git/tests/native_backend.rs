//! Integration tests for the native git backend.

use mmcp_git::{CommitSpec, GitBackend, GroupRef, NativeBackend, Rev};
use tempfile::TempDir;
use uuid::Uuid;

fn backend_in_tempdir() -> (NativeBackend, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let backend = NativeBackend::new(tmp.path()).expect("backend");
    (backend, tmp)
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
    let group = GroupRef::new(Uuid::now_v7(), "g");

    let handle_a = backend.create_group_repo(&group).await.unwrap();
    let handle_b = backend.create_group_repo(&group).await.unwrap();
    assert_eq!(handle_a, handle_b);
}

#[tokio::test]
async fn write_commit_and_read_file_round_trip() {
    let (backend, _tmp) = backend_in_tempdir();
    let group = GroupRef::new(Uuid::now_v7(), "g");
    let repo = backend.create_group_repo(&group).await.unwrap();

    let commit_id = backend
        .write_commit(
            &repo,
            sample_commit("alice", "main", "memories/hello.md", "+++\nname = \"hello\"\ndescription = \"h\"\nkind = \"rule\"\n+++\n# Hello\n"),
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
    let group = GroupRef::new(Uuid::now_v7(), "g");
    let repo = backend.create_group_repo(&group).await.unwrap();

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
    let group = GroupRef::new(Uuid::now_v7(), "g");
    let repo = backend.create_group_repo(&group).await.unwrap();

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
    let group = GroupRef::new(Uuid::now_v7(), "g");
    let repo = backend.create_group_repo(&group).await.unwrap();

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

#[tokio::test]
async fn remote_operations_are_unsupported() {
    let (backend, _tmp) = backend_in_tempdir();
    let group = GroupRef::new(Uuid::now_v7(), "g");
    let repo = backend.create_group_repo(&group).await.unwrap();

    assert!(matches!(
        backend.fetch(&repo, &[]).await.unwrap_err(),
        mmcp_git::GitError::Unsupported(_)
    ));
    assert!(matches!(
        backend.push(&repo, &[]).await.unwrap_err(),
        mmcp_git::GitError::Unsupported(_)
    ));
}
