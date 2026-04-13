//! Synchronous repository operations against a bare git repo.
//!
//! These functions are invoked from `spawn_blocking` inside the async
//! backend so the whole `gix` call tree stays synchronous.

use std::path::Path;

use bytes::Bytes;
use gix::bstr::BString;
use gix::objs::tree::EntryKind;

use crate::error::GitError;
use crate::types::{CommitMeta, CommitSpec, Rev};

fn gix_err<E: std::fmt::Display>(err: E) -> GitError {
    GitError::Gix(err.to_string())
}

/// Initialize a bare repository at `path`, idempotent.
pub fn init_bare(path: &Path) -> Result<(), GitError> {
    if path.exists() {
        return Ok(());
    }
    gix::init_bare(path).map_err(gix_err)?;
    Ok(())
}

/// Open a bare repository at `path`.
fn open_bare(path: &Path) -> Result<gix::Repository, GitError> {
    if !path.exists() {
        return Err(GitError::RepoNotFound(path.to_string_lossy().into_owned()));
    }
    gix::open(path).map_err(gix_err)
}

/// Resolve a `Rev` to a concrete commit object id.
fn resolve_rev(repo: &gix::Repository, rev: &Rev) -> Result<gix::ObjectId, GitError> {
    let target = match rev {
        Rev::Branch(name) => {
            let full = format!("refs/heads/{name}");
            let reference = repo
                .find_reference(full.as_str())
                .map_err(|_| GitError::RevNotFound(full.clone()))?;
            reference.id().detach()
        }
        Rev::Tag(name) => {
            let full = format!("refs/tags/{name}");
            let reference = repo
                .find_reference(full.as_str())
                .map_err(|_| GitError::RevNotFound(full.clone()))?;
            reference.id().detach()
        }
        Rev::Commit(hex) => gix::ObjectId::from_hex(hex.as_bytes())
            .map_err(|_| GitError::RevNotFound(hex.clone()))?,
    };
    Ok(target)
}

/// Walk `path` inside `tree`, returning the object id of the blob if
/// any. Handles nested directories separated by `/`.
fn find_blob_in_tree(
    repo: &gix::Repository,
    root_tree_id: gix::ObjectId,
    path: &str,
) -> Result<Option<gix::ObjectId>, GitError> {
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return Ok(None);
    }
    let mut current_tree_id = root_tree_id;
    for (idx, name) in components.iter().enumerate() {
        let tree_obj = repo.find_object(current_tree_id).map_err(gix_err)?;
        let tree: gix::objs::Tree = tree_obj.into_tree().decode().map_err(gix_err)?.into();
        let name_bytes = name.as_bytes();
        let entry = tree
            .entries
            .iter()
            .find(|e| AsRef::<[u8]>::as_ref(&e.filename) == name_bytes);
        match entry {
            None => return Ok(None),
            Some(entry) => {
                let is_last = idx == components.len() - 1;
                match entry.mode.kind() {
                    EntryKind::Blob | EntryKind::BlobExecutable if is_last => {
                        return Ok(Some(entry.oid));
                    }
                    EntryKind::Tree if !is_last => {
                        current_tree_id = entry.oid;
                    }
                    _ => return Ok(None),
                }
            }
        }
    }
    Ok(None)
}

/// Read the contents of `path` inside the commit at `rev`.
pub fn read_file(repo_path: &Path, path: &str, rev: &Rev) -> Result<Bytes, GitError> {
    let repo = open_bare(repo_path)?;
    let commit_id = resolve_rev(&repo, rev)?;
    let commit_obj = repo.find_object(commit_id).map_err(gix_err)?;
    let commit: gix::objs::Commit = commit_obj.into_commit().decode().map_err(gix_err)?.into();
    let blob_id = find_blob_in_tree(&repo, commit.tree, path)?
        .ok_or_else(|| GitError::PathNotFound(path.to_string()))?;
    let blob = repo.find_object(blob_id).map_err(gix_err)?;
    Ok(Bytes::from(blob.data.clone()))
}

/// Node in the in-memory tree we build before flushing to git.
///
/// Directories hold a map from child name to another node. Leaves
/// hold a blob object id. Pending deletes are represented as
/// `Option::None` blob nodes that the flusher drops.
enum TreeNode {
    Dir(std::collections::BTreeMap<BString, TreeNode>),
    Blob(Option<gix::ObjectId>),
}

impl TreeNode {
    fn empty_dir() -> Self {
        TreeNode::Dir(std::collections::BTreeMap::new())
    }
}

/// Load `tree_id` into an in-memory [`TreeNode::Dir`] recursively.
fn load_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
) -> Result<TreeNode, GitError> {
    let obj = repo.find_object(tree_id).map_err(gix_err)?;
    let tree: gix::objs::Tree = obj.into_tree().decode().map_err(gix_err)?.into();
    let mut entries = std::collections::BTreeMap::new();
    for entry in tree.entries {
        let node = match entry.mode.kind() {
            EntryKind::Tree => load_tree(repo, entry.oid)?,
            EntryKind::Blob | EntryKind::BlobExecutable => TreeNode::Blob(Some(entry.oid)),
            _ => continue,
        };
        entries.insert(entry.filename.clone(), node);
    }
    Ok(TreeNode::Dir(entries))
}

/// Walk `node` (must be a dir) following `components`, creating
/// intermediate directories as needed, and apply the given leaf edit.
fn apply_edit(
    node: &mut TreeNode,
    components: &[&str],
    leaf: TreeNode,
) -> Result<(), GitError> {
    let TreeNode::Dir(map) = node else {
        return Err(GitError::Gix(
            "path component collides with an existing blob".to_string(),
        ));
    };
    let (head, rest) = components.split_first().expect("non-empty components");
    let key = BString::from(*head);
    if rest.is_empty() {
        if matches!(leaf, TreeNode::Blob(None)) {
            map.remove(&key);
        } else {
            map.insert(key, leaf);
        }
        return Ok(());
    }
    let child = map.entry(key).or_insert_with(TreeNode::empty_dir);
    apply_edit(child, rest, leaf)
}

/// Recursively flush an in-memory tree to the object database.
fn flush_tree(
    repo: &gix::Repository,
    node: &TreeNode,
) -> Result<Option<gix::ObjectId>, GitError> {
    let TreeNode::Dir(map) = node else {
        return Err(GitError::Gix("flush_tree expects a Dir".to_string()));
    };
    let mut entries: Vec<gix::objs::tree::Entry> = Vec::new();
    for (name, child) in map {
        match child {
            TreeNode::Blob(Some(oid)) => {
                entries.push(gix::objs::tree::Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.clone(),
                    oid: *oid,
                });
            }
            TreeNode::Blob(None) => {}
            TreeNode::Dir(_) => {
                if let Some(sub_id) = flush_tree(repo, child)? {
                    entries.push(gix::objs::tree::Entry {
                        mode: EntryKind::Tree.into(),
                        filename: name.clone(),
                        oid: sub_id,
                    });
                }
            }
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort();
    let tree = gix::objs::Tree { entries };
    Ok(Some(repo.write_object(&tree).map_err(gix_err)?.detach()))
}

/// Construct a new tree by applying the edits in `files` on top of
/// the parent tree. Returns the new tree object id.
fn build_tree(
    repo: &gix::Repository,
    parent_tree_id: Option<gix::ObjectId>,
    files: &[(String, Option<Vec<u8>>)],
) -> Result<gix::ObjectId, GitError> {
    let mut root = match parent_tree_id {
        Some(parent) => load_tree(repo, parent)?,
        None => TreeNode::empty_dir(),
    };

    for (path, contents) in files {
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if components.is_empty() {
            return Err(GitError::PathNotFound(path.clone()));
        }
        let leaf = match contents {
            Some(bytes) => {
                let blob_id = repo.write_blob(bytes.as_slice()).map_err(gix_err)?.detach();
                TreeNode::Blob(Some(blob_id))
            }
            None => TreeNode::Blob(None),
        };
        apply_edit(&mut root, &components, leaf)?;
    }

    match flush_tree(repo, &root)? {
        Some(id) => Ok(id),
        None => {
            // Empty tree: write a zero-entry tree object.
            let empty = gix::objs::Tree { entries: Vec::new() };
            Ok(repo.write_object(&empty).map_err(gix_err)?.detach())
        }
    }
}

/// Create a new commit on the given branch applying a set of file
/// edits. The branch is created if it does not yet exist.
pub fn write_commit(repo_path: &Path, spec: CommitSpec) -> Result<String, GitError> {
    let repo = open_bare(repo_path)?;
    let branch_ref = format!("refs/heads/{}", spec.branch);

    let (parent_commit_id, parent_tree_id) = match repo.find_reference(branch_ref.as_str()) {
        Ok(reference) => {
            let parent_commit_id = reference.id().detach();
            let commit_obj = repo.find_object(parent_commit_id).map_err(gix_err)?;
            let commit: gix::objs::Commit =
                commit_obj.into_commit().decode().map_err(gix_err)?.into();
            (Some(parent_commit_id), Some(commit.tree))
        }
        Err(_) => (None, None),
    };

    let new_tree_id = build_tree(&repo, parent_tree_id, &spec.files)?;

    let now = gix::date::Time::now_local_or_utc();
    let signature = gix::actor::Signature {
        name: BString::from(spec.author_name.as_str()),
        email: BString::from(spec.author_email.as_str()),
        time: now,
    };
    let commit = gix::objs::Commit {
        tree: new_tree_id,
        parents: parent_commit_id.into_iter().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: BString::from(spec.message.as_str()),
        extra_headers: Vec::new(),
    };
    let commit_id = repo.write_object(&commit).map_err(gix_err)?.detach();

    let log_message = "mmcp: write commit";
    match repo.find_reference(branch_ref.as_str()) {
        Ok(mut reference) => {
            reference
                .set_target_id(commit_id, log_message)
                .map_err(gix_err)?;
        }
        Err(_) => {
            repo.reference(
                branch_ref.as_str(),
                commit_id,
                gix::refs::transaction::PreviousValue::MustNotExist,
                log_message,
            )
            .map_err(gix_err)?;
        }
    }

    Ok(commit_id.to_string())
}

/// Create a lightweight tag pointing at `target_hex`.
pub fn tag(repo_path: &Path, name: &str, target_hex: &str) -> Result<(), GitError> {
    let repo = open_bare(repo_path)?;
    let target = gix::ObjectId::from_hex(target_hex.as_bytes())
        .map_err(|_| GitError::RevNotFound(target_hex.to_string()))?;
    let refname = format!("refs/tags/{name}");
    repo.reference(
        refname.as_str(),
        target,
        gix::refs::transaction::PreviousValue::Any,
        "mmcp: tag",
    )
    .map_err(gix_err)?;
    Ok(())
}

/// Walk the commit history that touches `path`, most recent first.
pub fn walk_history(repo_path: &Path, path: &str) -> Result<Vec<CommitMeta>, GitError> {
    let repo = open_bare(repo_path)?;

    let head = match repo.find_reference("refs/heads/main") {
        Ok(r) => r.id().detach(),
        Err(_) => match repo.head_id() {
            Ok(id) => id.detach(),
            Err(_) => return Ok(Vec::new()),
        },
    };

    let mut out = Vec::new();
    let walk = repo.rev_walk([head]).all().map_err(gix_err)?;
    for info in walk {
        let info = info.map_err(gix_err)?;
        let commit_obj = repo.find_object(info.id).map_err(gix_err)?;
        let decoded_commit: gix::objs::Commit =
            commit_obj.into_commit().decode().map_err(gix_err)?.into();

        // Only include commits that contain `path` in their tree.
        let blob = find_blob_in_tree(&repo, decoded_commit.tree, path)?;
        if blob.is_none() {
            continue;
        }

        let message_str = decoded_commit.message.to_string();
        let subject = message_str.lines().next().unwrap_or("").to_string();

        out.push(CommitMeta {
            id: info.id.to_string(),
            subject,
            message: message_str,
            author_name: decoded_commit.author.name.to_string(),
            author_email: decoded_commit.author.email.to_string(),
            timestamp: decoded_commit.author.time.seconds,
        });
    }
    Ok(out)
}
