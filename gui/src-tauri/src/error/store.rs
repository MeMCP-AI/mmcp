//! Store-layer failures surfaced to the GUI.

use thiserror::Error;

/// Groups every distinct store-adjacent source type the GUI can
/// receive (the store's own I/O/manifest/git errors, plus import,
/// archive, and memory-parse failures) behind one facade so
/// `GuiError::Store` keeps a single field while each cause stays its
/// own variant with its own source chain.
#[derive(Debug, Error)]
pub enum GuiStoreError {
    #[error("{0}")]
    Store(#[from] mmcp_store::StoreError),

    #[error("{0}")]
    Import(#[from] mmcp_store::ImportError),

    #[error("{0}")]
    Archive(#[from] mmcp_store::ArchiveError),

    #[error("{0}")]
    MemoryParse(#[from] mmcp_core::memory::MemoryParseError),
}
