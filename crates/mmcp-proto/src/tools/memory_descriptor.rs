//! [`MemoryDescriptor`]: the short memory summary shared by every
//! listing-shaped tool response.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Short summary of a memory used in listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDescriptor {
    pub id: Uuid,
    pub group: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub mandatory: bool,
    pub latest_version: Option<String>,
}
