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

impl From<&mmcp_core::memory::MemoryRef> for MemoryRefDto {
    fn from(r: &mmcp_core::memory::MemoryRef) -> Self {
        Self {
            target: r.target,
            commit: r.commit.clone(),
        }
    }
}

impl From<MemoryRefDto> for mmcp_core::memory::MemoryRef {
    fn from(dto: MemoryRefDto) -> Self {
        Self {
            target: dto.target,
            commit: dto.commit,
        }
    }
}

/// Shared wire shape for `FeatureMetadataDto` and `IssueMetadataDto`:
/// both trackers carry the same status/number/dependency/supersession
/// fields, only the `status` vocabulary differs, and that vocabulary
/// stays a plain `String` on the wire either way so the frontend
/// doesn't have to re-declare either enum.
#[derive(Debug, Serialize, Deserialize)]
pub struct TrackedTicketMetadataDto {
    pub status: String,
    pub number: Option<u32>,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub blocks: Vec<Uuid>,
    pub superseded_by: Option<MemoryRefDto>,
}

/// Serde-snake-case status vocabulary: requested, approved, pending, completed, blocked, deferred, duplicate, superseded.
pub type FeatureMetadataDto = TrackedTicketMetadataDto;

/// Serde-snake-case status vocabulary: open, closed, wontfix, blocked, deferred, duplicate, superseded.
pub type IssueMetadataDto = TrackedTicketMetadataDto;

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

/// Builder for [`MemoryFrontmatterDto`].
///
/// The DTO carries 10 fields, several of them nested option/collection shapes.
/// A bare struct literal at the single call site ([`frontmatter_to_dto`]) buries which value maps to which wire field.
/// `new` takes the four fields every frontmatter block always has (id, name, description, kind); `id` is optional.
/// The rest go through one setter each,
/// mirroring the `TestServerConfigBuilder` house style (`crates/mmcp-server/tests/common/mod.rs`).
struct MemoryFrontmatterDtoBuilder {
    dto: MemoryFrontmatterDto,
}

impl MemoryFrontmatterDtoBuilder {
    fn new(id: Option<Uuid>, name: String, description: String, kind: String) -> Self {
        Self {
            dto: MemoryFrontmatterDto {
                id,
                name,
                description,
                kind,
                mandatory: false,
                version: None,
                tags: Vec::new(),
                refs: Vec::new(),
                feature: None,
                issue: None,
            },
        }
    }

    fn mandatory(mut self, mandatory: bool) -> Self {
        self.dto.mandatory = mandatory;
        self
    }

    fn version(mut self, version: Option<String>) -> Self {
        self.dto.version = version;
        self
    }

    fn tags(mut self, tags: Vec<String>) -> Self {
        self.dto.tags = tags;
        self
    }

    fn refs(mut self, refs: Vec<MemoryRefDto>) -> Self {
        self.dto.refs = refs;
        self
    }

    fn feature(mut self, feature: Option<FeatureMetadataDto>) -> Self {
        self.dto.feature = feature;
        self
    }

    fn issue(mut self, issue: Option<IssueMetadataDto>) -> Self {
        self.dto.issue = issue;
        self
    }

    fn build(self) -> MemoryFrontmatterDto {
        self.dto
    }
}

/// Build the wire DTO for one frontmatter block.
/// Shared by [`MemoryFileDto::from`] and [`MemoryDescriptorDto`] so the two cannot drift.
fn frontmatter_to_dto(fm: &MemoryFrontmatter) -> MemoryFrontmatterDto {
    MemoryFrontmatterDtoBuilder::new(
        fm.id,
        fm.name.clone(),
        fm.description.clone(),
        fm.kind.as_str().to_string(),
    )
    .mandatory(fm.mandatory)
    .version(fm.version.as_ref().map(|v| v.to_string()))
    .tags(fm.tags.clone())
    .refs(fm.refs.iter().map(MemoryRefDto::from).collect())
    .feature(fm.feature.as_ref().map(|fm| FeatureMetadataDto {
        status: fm.status.as_str().to_string(),
        number: fm.number,
        depends_on: fm.depends_on.clone(),
        blocks: fm.blocks.clone(),
        superseded_by: fm.superseded_by.as_ref().map(MemoryRefDto::from),
    }))
    .issue(fm.issue.as_ref().map(|im| IssueMetadataDto {
        status: im.status.as_str().to_string(),
        number: im.number,
        depends_on: im.depends_on.clone(),
        blocks: im.blocks.clone(),
        superseded_by: im.superseded_by.as_ref().map(MemoryRefDto::from),
    }))
    .build()
}

impl From<&MemoryFile> for MemoryFileDto {
    fn from(f: &MemoryFile) -> Self {
        MemoryFileDto {
            frontmatter: frontmatter_to_dto(&f.frontmatter),
            body: f.body.clone(),
        }
    }
}

/// One memory's frontmatter plus a change-detection identifier, with no markdown body.
/// Backs [`list_memory_descriptors`].
#[derive(Debug, Serialize)]
pub struct MemoryDescriptorDto {
    pub slug: String,
    /// Group repository tip commit at read time, shared by every descriptor in one response.
    /// Group-level granularity: coarser than per-file, so a re-check can be wasted, never stale.
    pub commit: String,
    pub frontmatter: MemoryFrontmatterDto,
}

impl MemoryDescriptorDto {
    fn new(slug: String, commit: String, frontmatter: MemoryFrontmatterDto) -> Self {
        Self {
            slug,
            commit,
            frontmatter,
        }
    }
}

/// Response shape for [`list_memory_descriptors`]: the descriptors that resolved cleanly,
/// plus every slug that was skipped and why, as [`mmcp_store::Finding`],
/// the workspace's one skip/failure record shape, instead of a bespoke pair.
/// Never `descriptors` alone: a caller that only reads `descriptors` still gets a complete list on the happy path,
/// but `skipped` makes a partial listing observable instead of silent.
#[derive(Debug, Serialize)]
pub struct MemoryDescriptorListDto {
    pub descriptors: Vec<MemoryDescriptorDto>,
    pub skipped: Vec<mmcp_store::Finding>,
}

/// Build the [`mmcp_store::Finding`] for a slug `list_memory_descriptors`
/// could not resolve or read, using a code local to this command since
/// neither failure mode has an existing `mmcp_store::tracker` builder.
fn skip_finding(
    group: &str,
    slug: String,
    code: &'static str,
    message: String,
) -> mmcp_store::Finding {
    mmcp_store::Finding {
        group: group.to_string(),
        slug: Some(slug),
        severity: "error",
        code,
        message,
    }
}

fn parse_kind(s: &str) -> GuiResult<MemoryKind> {
    Ok(s.parse::<MemoryKind>()?)
}

fn group_id_from_str(s: &str) -> GuiResult<GroupId> {
    let uuid = Uuid::parse_str(s).map_err(|source| GuiError::InvalidGroupId {
        value: s.to_string(),
        source,
    })?;
    Ok(GroupId::from_uuid(uuid))
}

/// Resolve `group_id` to its mirrored [`mmcp_store::GroupEntry`], or
/// [`GuiError::GroupNotInMirror`] when it is not indexed locally.
/// Shared by every command below that needs the group's git handle,
/// instead of each repeating the same lookup-and-error block.
async fn group_entry(state: &AppState, group_id: &str) -> GuiResult<mmcp_store::GroupEntry> {
    let gid = group_id_from_str(group_id)?;
    state
        .index
        .get(&gid)
        .await
        .ok_or_else(|| GuiError::GroupNotInMirror {
            group_id: group_id.to_string(),
        })
}

#[tauri::command]
pub async fn list_memory_slugs(
    group_id: String,
    state: State<'_, AppState>,
) -> GuiResult<Vec<String>> {
    tracing::debug!(group_id = %group_id, "ipc: list_memory_slugs");
    let entry = group_entry(&state, &group_id).await?;
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

/// Decode `bytes` as UTF-8 and parse them as a memory file, producing either a
/// resolved descriptor or a [`mmcp_store::Finding`] naming why the file was unusable.
/// Pulled out of [`list_memory_descriptors`] so the skip-populating behavior is
/// unit-testable without a live git backend or `AppState`.
fn classify_memory_bytes(
    group: &str,
    slug: String,
    commit: String,
    bytes: &[u8],
) -> Result<MemoryDescriptorDto, mmcp_store::Finding> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) => return Err(mmcp_store::tracker::not_utf8_finding(group, &slug, &err)),
    };
    let mf = match MemoryFile::parse(text) {
        Ok(mf) => mf,
        Err(err) => {
            return Err(mmcp_store::tracker::parse_failed_finding(
                group, &slug, &err,
            ));
        }
    };
    Ok(MemoryDescriptorDto::new(
        slug,
        commit,
        frontmatter_to_dto(&mf.frontmatter),
    ))
}

/// Frontmatter for every memory in one group, plus the group tip commit.
/// Returns [`MemoryDescriptorListDto`].
/// A slug that cannot be resolved, read, decoded as UTF-8, or parsed is logged
/// AND reported back in the response's `skipped` list, never dropped silently:
/// `Ok(descriptors)` alone would let the caller mistake a truncated listing for
/// a complete one, with no way to tell the two apart.
#[tauri::command]
pub async fn list_memory_descriptors(
    group_id: String,
    state: State<'_, AppState>,
) -> GuiResult<MemoryDescriptorListDto> {
    tracing::debug!(group_id = %group_id, "ipc: list_memory_descriptors");
    let entry = group_entry(&state, &group_id).await?;
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

    let mut skipped = Vec::new();
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
                skipped.push(skip_finding(
                    &group_id,
                    slug,
                    "memory_unresolvable",
                    format!("unresolvable: {err}"),
                ));
            }
        }
    }

    let batch = state
        .backend
        .read_files(&entry.handle, paths, &Rev::head())
        .await
        .map_err(GuiError::from)?;

    let mut descriptors = Vec::with_capacity(batch.len());
    for (slug, (path, outcome)) in resolved_slugs.into_iter().zip(batch) {
        let bytes = match outcome {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(group_id = %group_id, slug = %slug, path = %path, error = %err, "list_memory_descriptors: skipping unreadable file");
                skipped.push(skip_finding(
                    &group_id,
                    slug,
                    "memory_unreadable",
                    format!("unreadable: {err}"),
                ));
                continue;
            }
        };
        match classify_memory_bytes(&group_id, slug, tip.id.clone(), &bytes) {
            Ok(descriptor) => descriptors.push(descriptor),
            Err(skip) => {
                tracing::warn!(group_id = %group_id, slug = ?skip.slug, path = %path, reason = %skip.message, "list_memory_descriptors: skipping unusable file");
                skipped.push(skip);
            }
        }
    }
    Ok(MemoryDescriptorListDto {
        descriptors,
        skipped,
    })
}

#[tauri::command]
pub async fn load_memory(
    group_id: String,
    slug: String,
    state: State<'_, AppState>,
) -> GuiResult<MemoryFileDto> {
    tracing::debug!(group_id = %group_id, slug = %slug, "ipc: load_memory");
    let entry = group_entry(&state, &group_id).await?;
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
        Some(s) if !s.is_empty() => Some(semver::Version::parse(s)?),
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
                    status: mmcp_core::memory::FeatureStatus::parse(&f.status)?,
                    number: f.number,
                    depends_on: f.depends_on,
                    blocks: f.blocks,
                    superseded_by: f.superseded_by.map(mmcp_core::memory::MemoryRef::from),
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
                    status: mmcp_core::memory::IssueStatus::parse(&i.status)?,
                    number: i.number,
                    depends_on: i.depends_on,
                    blocks: i.blocks,
                    superseded_by: i.superseded_by.map(mmcp_core::memory::MemoryRef::from),
                })
            })
            .transpose()?,
        refs: dto
            .frontmatter
            .refs
            .into_iter()
            .map(mmcp_core::memory::MemoryRef::from)
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
    let entry = group_entry(&state, &group_id).await?;
    let mut file = to_memory_file(memory)?;
    let id = file.frontmatter.id.unwrap_or_else(Uuid::now_v7);
    file.frontmatter = file.frontmatter.clone().with_id(id);
    let rendered = file.to_string().map_err(GuiError::from)?;
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
    let entry = group_entry(&state, &group_id).await?;
    let resolved = resolve_memory(&state.backend, &entry.handle, Some(&slug), None)
        .await
        .map_err(GuiError::from)?;
    let mut file = to_memory_file(memory)?;
    file.frontmatter = file.frontmatter.clone().with_id(resolved.id);
    let rendered = file.to_string().map_err(GuiError::from)?;
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
    let entry = group_entry(&state, &group_id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MEMORY_BYTES: &[u8] = b"+++\nname = \"Rust Coding Rules\"\ndescription = \"Strict Rust coding conventions\"\nkind = \"rule\"\nmandatory = true\n+++\n# Rust Coding Rules\n\nBody text.\n";

    /// Asserts a deliberately-malformed file (no frontmatter fence at all) produces a
    /// populated `skipped` entry carrying `mmcp_store`'s stable
    /// `frontmatter_parse_failed` code, not a caller-invisible log line.
    #[test]
    fn classify_memory_bytes_populates_a_skipped_entry_for_unparseable_content() {
        let garbage = b"this is not a memory file, it has no frontmatter fence at all";

        let outcome = classify_memory_bytes(
            "test-group",
            "broken-memory".to_string(),
            "deadbeef".to_string(),
            garbage,
        );

        let skip = outcome.expect_err("garbage content must not classify as a descriptor");
        assert_eq!(skip.slug.as_deref(), Some("broken-memory"));
        assert_eq!(skip.code, "frontmatter_parse_failed");
    }

    #[test]
    fn classify_memory_bytes_populates_a_skipped_entry_for_non_utf8_content() {
        let non_utf8: &[u8] = &[0xFF, 0xFE, 0xFD];

        let outcome = classify_memory_bytes(
            "test-group",
            "binary-memory".to_string(),
            "deadbeef".to_string(),
            non_utf8,
        );

        let skip = outcome.expect_err("non-UTF-8 bytes must not classify as a descriptor");
        assert_eq!(skip.slug.as_deref(), Some("binary-memory"));
        assert_eq!(skip.code, "memory_not_utf8");
    }

    #[test]
    fn classify_memory_bytes_returns_a_descriptor_for_valid_content() {
        let outcome = classify_memory_bytes(
            "test-group",
            "good-memory".to_string(),
            "deadbeef".to_string(),
            VALID_MEMORY_BYTES,
        );

        let descriptor = outcome.expect("valid memory bytes must classify as a descriptor");
        assert_eq!(descriptor.slug, "good-memory");
        assert_eq!(descriptor.commit, "deadbeef");
        assert_eq!(descriptor.frontmatter.name, "Rust Coding Rules");
    }

    /// `MemoryDescriptorListDto` carries `skipped` alongside `descriptors`
    /// rather than the caller having to infer a partial listing from a
    /// length mismatch against a separate slug count.
    #[test]
    fn descriptor_list_dto_serializes_both_descriptors_and_skipped() {
        let good = classify_memory_bytes(
            "test-group",
            "good".to_string(),
            "deadbeef".to_string(),
            VALID_MEMORY_BYTES,
        )
        .expect("valid bytes classify");
        let bad = classify_memory_bytes(
            "test-group",
            "bad".to_string(),
            "deadbeef".to_string(),
            b"not a memory file",
        )
        .expect_err("garbage bytes do not classify");

        let dto = MemoryDescriptorListDto {
            descriptors: vec![good],
            skipped: vec![bad],
        };
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["descriptors"].as_array().unwrap().len(), 1);
        assert_eq!(value["skipped"].as_array().unwrap().len(), 1);
        assert_eq!(value["skipped"][0]["slug"], "bad");
    }
}
