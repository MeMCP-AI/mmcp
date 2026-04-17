//! In-memory cache of already-parsed memory files.
//!
//! The background worker parses each memory once via
//! `MemoryFile::parse`; the UI side stores the parsed result here so
//! redrawing the viewer doesn't re-fetch or re-parse. Arc'd so the
//! UI and the cache share ownership without a clone per frame.

use std::collections::HashMap;
use std::sync::Arc;

use mmcp_core::id::GroupId;
use mmcp_core::memory::MemoryFile;

#[derive(Default)]
pub struct ViewerCache {
    entries: HashMap<(GroupId, String), Arc<MemoryFile>>,
}

impl ViewerCache {
    pub fn get(&self, group: &GroupId, slug: &str) -> Option<Arc<MemoryFile>> {
        self.entries.get(&(*group, slug.to_string())).cloned()
    }

    pub fn insert(&mut self, group: GroupId, slug: String, memory: Arc<MemoryFile>) {
        self.entries.insert((group, slug), memory);
    }
}
