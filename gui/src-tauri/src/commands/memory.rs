//! Memory CRUD commands. Thin wrappers around mmcp-store primitives.

use mmcp_core::id::GroupId;
use mmcp_core::memory::{MemoryFile, MemoryFrontmatter, MemoryKind};
use mmcp_git::{GitBackend, Rev};
use mmcp_store::{
    AddressingMode, WriteFileOptions, WriteMemoryOptions, delete_file_at_path, resolve_memory,
    write_file_at_path, write_memory_by_id,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::error::{GuiError, GuiResult};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryRefDto {
    pub target: Uuid,
    pub commit: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeatureMetadataDto {
    /// Serde-snake-case: requested, approved, pending, completed, blocked, deferred, duplicate, superseded.
    /// Kept as a String on the wire so the frontend doesn't have to re-declare the enum variants.
    pub status: String,
    pub number: Option<u32>,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRefDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IssueMetadataDto {
    /// Serde-snake-case: open, closed, wontfix, blocked, deferred, duplicate, superseded.
    /// Kept as a String on the wire to mirror `FeatureMetadataDto::status`.
    pub status: String,
    pub number: Option<u32>,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRefDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryFrontmatterDto {
    pub id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub kind: String,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub refs: Vec<MemoryRefDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<FeatureMetadataDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueMetadataDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryFileDto {
    pub frontmatter: MemoryFrontmatterDto,
    pub body: String,
}

/// Build the wire DTO for one frontmatter block. Shared by
/// [`MemoryFileDto::from`] (full body reads) and
/// [`MemoryDescriptorDto`] listings (metadata-only reads) so the two
/// call sites can never drift on which fields the frontend sees.
fn frontmatter_to_dto(fm: &MemoryFrontmatter) -> MemoryFrontmatterDto {
    MemoryFrontmatterDto {
        id: fm.id,
        name: fm.name.clone(),
        description: fm.description.clone(),
        kind: fm.kind.as_str().to_string(),
        mandatory: fm.mandatory,
        version: fm.version.as_ref().map(|v| v.to_string()),
        tags: fm.tags.clone(),
        refs: fm
            .refs
            .iter()
            .map(|r| MemoryRefDto {
                target: r.target,
                commit: r.commit.clone(),
            })
            .collect(),
        feature: fm.feature.as_ref().map(|fm| FeatureMetadataDto {
            status: fm.status.as_str().to_string(),
            number: fm.number,
            depends_on: fm.depends_on.clone(),
            blocks: fm.blocks.clone(),
            superseded_by: fm.superseded_by.as_ref().map(|r| MemoryRefDto {
                target: r.target,
                commit: r.commit.clone(),
            }),
        }),
        issue: fm.issue.as_ref().map(|im| IssueMetadataDto {
            status: im.status.as_str().to_string(),
            number: im.number,
            depends_on: im.depends_on.clone(),
            blocks: im.blocks.clone(),
            superseded_by: im.superseded_by.as_ref().map(|r| MemoryRefDto {
                target: r.target,
                commit: r.commit.clone(),
            }),
        }),
    }
}

impl From<&MemoryFile> for MemoryFileDto {
    fn from(f: &MemoryFile) -> Self {
        MemoryFileDto {
            frontmatter: frontmatter_to_dto(&f.frontmatter),
            body: f.body.clone(),
        }
    }
}

/// One memory's frontmatter plus a change-detection identifier, with
/// no markdown body. Backs [`list_memory_descriptors`]: a single
/// batched call per group instead of one `load_memory` round trip
/// per memory (see issue #126).
#[derive(Debug, Serialize)]
pub struct MemoryDescriptorDto {
    pub slug: String,
    /// The group repository's tip commit at read time. Every
    /// descriptor in one `list_memory_descriptors` response carries
    /// the same value — the group is one git repo, so there is no
    /// cheaper per-file granularity available without walking each
    /// file's own history. The frontend re-downloads a cached body
    /// only when this value changes from what it last saw, which is
    /// a safe, conservative check: zero false negatives (a real
    /// change always changes the tip), occasional false positives
    /// (an unrelated file in the same group also triggers a
    /// re-check) that cost one wasted body read, never a stale read.
    pub commit: String,
    pub frontmatter: MemoryFrontmatterDto,
}

fn parse_kind(s: &str) -> GuiResult<MemoryKind> {
    s.parse::<MemoryKind>()
        .map_err(|e| GuiError::Other(e.to_string()))
}

fn group_id_from_str(s: &str) -> GuiResult<GroupId> {
    let uuid = Uuid::parse_str(s).map_err(|e| GuiError::Other(format!("group_id uuid: {e}")))?;
    Ok(GroupId::from_uuid(uuid))
}

#[tauri::command]
pub async fn list_memory_slugs(
    group_id: String,
    state: State<'_, AppState>,
) -> GuiResult<Vec<String>> {
    tracing::debug!(group_id = %group_id, "ipc: list_memory_slugs");
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let mut slugs = state
        .backend
        .list_subtrees(
            &entry.handle,
            mmcp_core::conventions::MEMORIES_DIR,
            &Rev::head(),
        )
        .await
        .map_err(GuiError::from)?;
    slugs.sort();
    Ok(slugs)
}

/// Metadata-only listing for one group: every memory's frontmatter
/// plus a shared change-detection commit id, with no markdown body.
/// Resolves the group's tip commit and reads every file's bytes in
/// one batch (`NativeBackend::read_files`, which resolves the
/// commit/tree once for the whole call) instead of the home route's
/// former per-slug `load_memory` round trip. A slug whose path can no
/// longer be resolved, or whose bytes fail to parse as a memory file
/// (e.g. a mid-write race with a concurrent commit), is logged and
/// skipped rather than failing the whole listing — see issue #126.
#[tauri::command]
pub async fn list_memory_descriptors(
    group_id: String,
    state: State<'_, AppState>,
) -> GuiResult<Vec<MemoryDescriptorDto>> {
    tracing::debug!(group_id = %group_id, "ipc: list_memory_descriptors");
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let mut slugs = state
        .backend
        .list_subtrees(
            &entry.handle,
            mmcp_core::conventions::MEMORIES_DIR,
            &Rev::head(),
        )
        .await
        .map_err(GuiError::from)?;
    slugs.sort();

    let tip = state
        .backend
        .tip_commit(&entry.handle, &Rev::head())
        .await
        .map_err(GuiError::from)?;

    let mut resolved_slugs = Vec::with_capacity(slugs.len());
    let mut paths = Vec::with_capacity(slugs.len());
    for slug in slugs {
        match resolve_memory(&state.backend, &entry.handle, Some(&slug), None).await {
            Ok(resolved) => {
                resolved_slugs.push(slug);
                paths.push(resolved.path);
            }
            Err(err) => {
                tracing::warn!(group_id = %group_id, slug = %slug, error = %err, "list_memory_descriptors: skipping unresolvable slug");
            }
        }
    }

    let batch = state
        .backend
        .read_files(&entry.handle, paths, &Rev::head())
        .await
        .map_err(GuiError::from)?;

    let mut out = Vec::with_capacity(batch.len());
    for (slug, (path, outcome)) in resolved_slugs.into_iter().zip(batch) {
        let bytes = match outcome {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(group_id = %group_id, slug = %slug, path = %path, error = %err, "list_memory_descriptors: skipping unreadable file");
                continue;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(err) => {
                tracing::warn!(group_id = %group_id, slug = %slug, path = %path, error = %err, "list_memory_descriptors: skipping non-utf8 file");
                continue;
            }
        };
        let mf = match MemoryFile::parse(text) {
            Ok(mf) => mf,
            Err(err) => {
                tracing::warn!(group_id = %group_id, slug = %slug, path = %path, error = %err, "list_memory_descriptors: skipping unparseable file");
                continue;
            }
        };
        out.push(MemoryDescriptorDto {
            slug,
            commit: tip.id.clone(),
            frontmatter: frontmatter_to_dto(&mf.frontmatter),
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn load_memory(
    group_id: String,
    slug: String,
    state: State<'_, AppState>,
) -> GuiResult<MemoryFileDto> {
    tracing::debug!(group_id = %group_id, slug = %slug, "ipc: load_memory");
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let bytes = state
        .backend
        .read_file(&entry.handle, &resolved.path, &Rev::head())
        .await
        .map_err(GuiError::from)?;
    let text = std::str::from_utf8(&bytes)?;
    let mf = MemoryFile::parse(text).map_err(GuiError::from)?;
    Ok(MemoryFileDto::from(&mf))
}

fn to_memory_file(dto: MemoryFileDto) -> GuiResult<MemoryFile> {
    let kind = parse_kind(&dto.frontmatter.kind)?;
    let version = match dto.frontmatter.version.as_deref() {
        Some(s) if !s.is_empty() => Some(
            semver::Version::parse(s)
                .map_err(|e| GuiError::Other(format!("version parse: {e}")))?,
        ),
        _ => None,
    };
    let fm = MemoryFrontmatter {
        id: dto.frontmatter.id,
        name: dto.frontmatter.name,
        description: dto.frontmatter.description,
        kind,
        mandatory: dto.frontmatter.mandatory,
        version,
        tags: dto.frontmatter.tags,
        bump_intent: None,
        milestone: None,
        feature: dto
            .frontmatter
            .feature
            .map(|f| -> GuiResult<mmcp_core::memory::FeatureMetadata> {
                Ok(mmcp_core::memory::FeatureMetadata {
                    status: mmcp_core::memory::FeatureStatus::parse(&f.status)
                        .map_err(|e| GuiError::Other(e.to_string()))?,
                    number: f.number,
                    depends_on: f.depends_on,
                    blocks: f.blocks,
                    superseded_by: f.superseded_by.map(|r| mmcp_core::memory::MemoryRef {
                        target: r.target,
                        commit: r.commit,
                    }),
                    // GUI does not yet expose milestone linking on features.
                    // Default-None mirrors the frontmatter milestone default above.
                    milestone: None,
                })
            })
            .transpose()?,
        issue: dto
            .frontmatter
            .issue
            .map(|i| -> GuiResult<mmcp_core::memory::IssueMetadata> {
                Ok(mmcp_core::memory::IssueMetadata {
                    status: mmcp_core::memory::IssueStatus::parse(&i.status)
                        .map_err(|e| GuiError::Other(e.to_string()))?,
                    number: i.number,
                    depends_on: i.depends_on,
                    blocks: i.blocks,
                    superseded_by: i.superseded_by.map(|r| mmcp_core::memory::MemoryRef {
                        target: r.target,
                        commit: r.commit,
                    }),
                })
            })
            .transpose()?,
        refs: dto
            .frontmatter
            .refs
            .into_iter()
            .map(|r| mmcp_core::memory::MemoryRef {
                target: r.target,
                commit: r.commit,
            })
            .collect(),
        // DTO carries no `source` field: every write drops whatever value was previously on disk.
        source: None,
    };
    Ok(MemoryFile {
        frontmatter: fm,
        body: dto.body,
        format: mmcp_core::memory::FrontmatterFormat::TomlPlus,
    })
}

#[tauri::command]
pub async fn create_memory(
    group_id: String,
    slug: String,
    memory: MemoryFileDto,
    state: State<'_, AppState>,
) -> GuiResult<String> {
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let mut file = to_memory_file(memory)?;
    let id = file.frontmatter.id.unwrap_or_else(Uuid::now_v7);
    file.frontmatter = file.frontmatter.clone().with_id(id);
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    let (commit, _validation) = write_memory_by_id(
        &state.backend,
        &entry.handle,
        &slug,
        id,
        &rendered,
        &*state.author.read().await,
        WriteMemoryOptions {
            addressing_mode: AddressingMode::ByFilename,
            ..Default::default()
        },
    )
    .await
    .map_err(GuiError::from)?;
    Ok(commit)
}

#[tauri::command]
pub async fn update_memory(
    group_id: String,
    slug: String,
    memory: MemoryFileDto,
    state: State<'_, AppState>,
) -> GuiResult<String> {
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let mut file = to_memory_file(memory)?;
    file.frontmatter = file.frontmatter.clone().with_id(resolved.id);
    let rendered = file
        .to_string()
        .map_err(|e| GuiError::Other(format!("render: {e}")))?;
    let (commit, _validation) = write_file_at_path(
        &state.backend,
        &entry.handle,
        &resolved.path,
        &rendered,
        &*state.author.read().await,
        WriteFileOptions {
            addressing_mode: resolved.addressing_mode,
            ..Default::default()
        },
    )
    .await
    .map_err(GuiError::from)?;
    Ok(commit)
}

#[tauri::command]
pub async fn delete_memory(
    group_id: String,
    slug: String,
    state: State<'_, AppState>,
) -> GuiResult<String> {
    let gid = group_id_from_str(&group_id)?;
    let entry =
        state.index.get(&gid).await.ok_or_else(|| {
            GuiError::Other(format!("group {group_id} is not in the local mirror"))
        })?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let commit = delete_file_at_path(
        &state.backend,
        &entry.handle,
        &resolved.path,
        &*state.author.read().await,
        None,
    )
    .await
    .map_err(GuiError::from)?;
    Ok(commit)
}
