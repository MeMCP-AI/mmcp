//! Validated first-pull adoption of an explicitly configured Git repository.
//!
//! Engine construction remains local; push and fetch never create a group.
//! Only the default remote may initialize local main.

use std::sync::Arc;

use mmcp_core::config::{Remote, RemoteAuth};
use mmcp_core::manifest::{GroupManifest, GroupScope};
use mmcp_git::{Credentials, GitBackend, NativeBackend, RepoHandle, Rev};
use mmcp_sync::{SyncEngine, SyncFilter};
use uuid::Uuid;

use super::engine_wiring::resolve_credentials;
use super::{EffectiveRemotes, IndexResolver, ResolvedRemote, build_engine};
use crate::error::{FileOperation, StoreError};
use crate::groups::GroupIndex;

/// A syntactically validated selector, before local group resolution.
/// Callers reject absent or conflicting selectors before invoking prepare_pull.
#[derive(Debug, Clone)]
pub enum PullSelector {
    /// Select a configured or mirrored group by UUID or slug.
    Group(String),
    /// Select groups carrying this scope in their manifests.
    Scope(GroupScope),
    /// Select every applicable group.
    All,
}

/// Prepare a pull on either an existing mirror or a fresh device.
///
/// A missing default direct-Git group is cloned into a temporary bare repo.
/// Its main-branch manifest must match the configured UUID/slug and selector
/// before publication. Existing paths are never replaced. Failed or cancelled
/// clones leave no group visible to the index.
///
/// Unmirrored non-default repositories remain untouched: only the default
/// remote may initialize main. Afterwards the existing engine builder validates
/// the complete original configuration, preserving all remote-selection errors.
pub async fn prepare_pull(
    backend: Arc<NativeBackend>,
    groups: GroupIndex,
    effective: &EffectiveRemotes,
    selector: PullSelector,
) -> Result<(SyncEngine, IndexResolver, SyncFilter), StoreError> {
    if let Some(remote) = effective.default_remote()
        && let Some(group_ref) = remote.direct_git_group.as_deref()
        && crate::memory::resolve_group(&groups, group_ref)
            .await
            .is_err()
        && could_select(&selector, group_ref, &groups).await
    {
        adopt(&backend, &groups, remote, &selector).await?;
    }
    let filter = match &selector {
        PullSelector::All => SyncFilter::All,
        PullSelector::Scope(scope) => SyncFilter::Scope(*scope),
        PullSelector::Group(query) => {
            let entry = crate::memory::resolve_group(&groups, query)
                .await
                .map_err(|_| StoreError::PullGroupNotFound {
                    query: query.clone(),
                })?;
            SyncFilter::Group(*entry.manifest.group_id.as_uuid())
        }
    };

    let (engine, resolver) = build_engine(backend, groups, effective).await?;
    Ok((engine, resolver, filter))
}

async fn could_select(selector: &PullSelector, group_ref: &str, groups: &GroupIndex) -> bool {
    let PullSelector::Group(query) = selector else {
        return true;
    };
    if query == group_ref {
        return true;
    }
    if crate::memory::resolve_group(groups, query).await.is_ok() {
        return false;
    }
    // A slug selector against a UUID-bound remote needs its manifest to learn
    // the slug. Two different UUIDs can be excluded without network access.
    match (Uuid::parse_str(query), Uuid::parse_str(group_ref)) {
        (Ok(query), Ok(configured)) => query == configured,
        _ => true,
    }
}

fn matches_identity(reference: &str, manifest: &GroupManifest) -> bool {
    match Uuid::parse_str(reference) {
        Ok(uuid) => &uuid == manifest.group_id.as_uuid(),
        Err(_) => reference == manifest.slug,
    }
}

fn matches_selector(selector: &PullSelector, manifest: &GroupManifest) -> bool {
    match selector {
        PullSelector::All => true,
        PullSelector::Scope(scope) => *scope == manifest.scope,
        PullSelector::Group(query) => matches_identity(query, manifest),
    }
}

fn credentials(remote: &ResolvedRemote) -> Credentials {
    if let Remote::DirectGit {
        auth: RemoteAuth::Bearer,
        ..
    } = &remote.remote
    {
        let (token, _) = resolve_credentials(remote, &|key| std::env::var(key).ok());
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            return Credentials::bearer(token);
        }
    }
    Credentials::None
}

async fn adopt(
    backend: &NativeBackend,
    groups: &GroupIndex,
    remote: &ResolvedRemote,
    selector: &PullSelector,
) -> Result<(), StoreError> {
    let Remote::DirectGit { url, .. } = &remote.remote else {
        return Ok(());
    };
    let group_ref = remote.direct_git_group.as_deref().unwrap_or_default();
    let failure = |reason: String| StoreError::DirectGitBootstrap {
        remote_name: remote.name().to_string(),
        reason,
    };
    let probe = backend.repo_path(Uuid::nil());
    let root = probe
        .parent()
        .ok_or_else(|| failure("missing repository root".into()))?;
    let stage = tempfile::Builder::new()
        .prefix(".pull-")
        .tempdir_in(root)
        .map_err(|source| StoreError::io(root, FileOperation::CreateDir, source))?;
    let clone_path = stage.path().join("repository");
    backend
        .clone_bare_to(url, &clone_path, &credentials(remote))
        .await?;
    // Validate using a separate backend so cached handles do not survive the
    // move from staging to the final path, especially on Windows.
    let manifest = {
        let validation = NativeBackend::new(stage.path())?;
        let handle = RepoHandle::new(Uuid::nil(), clone_path.to_string_lossy().into_owned());
        // Git accepts a tag for --branch; require the branch used by sync.
        validation
            .tip_commit(&handle, &Rev::main())
            .await
            .map_err(|error| failure(format!("missing refs/heads/main: {error}")))?;
        validation
            .read_manifest(&handle)
            .await
            .map_err(|error| failure(format!("invalid main-branch group manifest: {error}")))?
    };
    if !matches_identity(group_ref, &manifest) {
        return Err(failure(format!(
            "expected group {group_ref}, remote contains {} ({})",
            manifest.group_id, manifest.slug
        )));
    }
    if !matches_selector(selector, &manifest) {
        return Ok(());
    }
    let id = *manifest.group_id.as_uuid();
    let destination = backend.repo_path(id);
    // A nonempty directory cannot replace another repository through rename.
    // Explicitly reject even an empty pre-existing destination.
    if destination
        .try_exists()
        .map_err(|source| StoreError::io(&destination, FileOperation::Metadata, source))?
    {
        groups.refresh().await?;
        if let Some(existing) = groups.get(&manifest.group_id).await
            && existing.manifest == manifest
        {
            return Ok(());
        }
        return Err(failure(format!(
            "destination already exists: {}",
            destination.display()
        )));
    }
    std::fs::rename(&clone_path, &destination)
        .map_err(|source| StoreError::io(&destination, FileOperation::Rename, source))?;
    groups.refresh().await?;
    if groups.get(&manifest.group_id).await.is_none() {
        return Err(failure("cloned repository was not indexed".into()));
    }
    crate::cache::notify_pull(backend, groups, &[id]).await;
    Ok(())
}
