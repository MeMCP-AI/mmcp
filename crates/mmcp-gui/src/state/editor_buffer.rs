//! Draft buffer for the memory editor (New / Edit modes).
//!
//! Holds the user-editable fields plus the original
//! [`MemoryFrontmatter`] so non-editable bits (`version`,
//! `bump_intent`, `feature`) survive a save round-trip untouched.
//! `tags` is held as a comma-separated string for low-friction
//! entry; [`EditorBuffer::tags_split`] normalises it at save time
//! (trim + dedup + drop empties + sort).

use mmcp_core::id::GroupId;
use mmcp_core::memory::{FrontmatterFormat, MemoryFile, MemoryFrontmatter, MemoryKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    New,
    Edit,
}

#[derive(Debug, Clone)]
pub struct EditorBuffer {
    pub mode: EditorMode,
    pub group_id: GroupId,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub kind: MemoryKind,
    pub mandatory: bool,
    pub tags_raw: String,
    pub body: String,
    /// For Edit mode: the frontmatter as loaded from disk. Used to
    /// preserve `version`, `bump_intent`, `feature`, and any other
    /// fields the form does not expose. `None` in New mode — the
    /// save path builds a fresh frontmatter instead.
    pub original_frontmatter: Option<MemoryFrontmatter>,
    /// Target render format. Preserved from disk in Edit mode so a
    /// YAML-fronted memory stays YAML on round-trip; defaults to
    /// `TomlPlus` for New.
    pub format: FrontmatterFormat,
}

impl EditorBuffer {
    pub fn for_new(group_id: GroupId) -> Self {
        Self {
            mode: EditorMode::New,
            group_id,
            slug: String::new(),
            name: String::new(),
            description: String::new(),
            kind: MemoryKind::Scratch,
            mandatory: false,
            tags_raw: String::new(),
            body: String::new(),
            original_frontmatter: None,
            format: FrontmatterFormat::TomlPlus,
        }
    }

    pub fn for_edit(group_id: GroupId, slug: String, file: &MemoryFile) -> Self {
        Self {
            mode: EditorMode::Edit,
            group_id,
            slug,
            name: file.frontmatter.name.clone(),
            description: file.frontmatter.description.clone(),
            kind: file.frontmatter.kind,
            mandatory: file.frontmatter.mandatory,
            tags_raw: file.frontmatter.tags.join(", "),
            body: file.body.clone(),
            original_frontmatter: Some(file.frontmatter.clone()),
            format: file.format,
        }
    }

    fn tags_split(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .tags_raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }

    /// Assemble a `MemoryFile` ready for rendering + commit. For
    /// Edit mode, non-form fields (`version`, `bump_intent`,
    /// `feature`) are copied verbatim from the original.
    pub fn to_memory_file(&self) -> MemoryFile {
        let mut fm = self
            .original_frontmatter
            .clone()
            .unwrap_or(MemoryFrontmatter {
                id: None,
                name: String::new(),
                description: String::new(),
                kind: MemoryKind::Scratch,
                mandatory: false,
                version: None,
                tags: Vec::new(),
                bump_intent: None,
                feature: None,
            });
        fm.name = self.name.clone();
        fm.description = self.description.clone();
        fm.kind = self.kind;
        fm.mandatory = self.mandatory;
        fm.tags = self.tags_split();
        MemoryFile {
            frontmatter: fm,
            body: self.body.clone(),
            format: self.format,
        }
    }

    pub fn validation_error(&self) -> Option<&'static str> {
        if self.slug.trim().is_empty() {
            return Some("slug is required");
        }
        if self.name.trim().is_empty() {
            return Some("name is required");
        }
        if self.description.trim().is_empty() {
            return Some("description is required");
        }
        None
    }
}
