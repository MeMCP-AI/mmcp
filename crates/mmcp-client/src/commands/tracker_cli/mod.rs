//! Shared CLI-layer tracker plumbing.
//!
//! `feature`, `issue`, `milestone`, and the generic `memory` surface
//! are structurally near-identical CLI subcommand surfaces, each
//! once triplicating the same two small helpers: reading a body
//! argument (literal text, stdin via `-`, or a file via `@PATH`) and
//! rendering a UUID list for human-readable output. Mirrors the
//! justification in `mmcp-store`'s `tracker.rs`: the helper lives in
//! its own concern-named module so no single tracker CLI surface
//! owns it.

use anyhow::{Context, Result};

/// Interpret a body argument, in the three shapes every write
/// subcommand across `feature`/`issue`/`milestone`/`memory` accepts:
/// the literal `-` reads from stdin so operators can pipe in
/// markdown without quoting hell, an `@PATH` prefix reads from a
/// file, and any other value is used verbatim.
///
/// `subject_label` names what is being read (e.g. `"feature"`,
/// `"issue"`, `"milestone"`, `"memory"`) for the stdin/file-read
/// error context.
pub fn read_body(raw: &str, subject_label: &str) -> Result<String> {
    use std::io::Read;
    if raw == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .with_context(|| format!("reading {subject_label} body from stdin"))?;
        Ok(buf)
    } else if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path)
            .with_context(|| format!("reading {subject_label} body from {path}"))
    } else {
        Ok(raw.to_string())
    }
}

/// Join UUIDs into a comma-separated display string for the
/// human-readable CLI output.
pub fn join_uuids(values: &[uuid::Uuid]) -> String {
    values
        .iter()
        .map(uuid::Uuid::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn read_body_passes_through_literal_text() {
        let body = read_body("hello world", "feature").expect("literal body");
        assert_eq!(body, "hello world");
    }

    #[test]
    fn read_body_reads_an_at_prefixed_file_path() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        std::fs::write(tmp.path(), "body from disk").expect("write scratch file");
        let arg = format!("@{}", tmp.path().display());
        let body = read_body(&arg, "memory").expect("file body");
        assert_eq!(body, "body from disk");
    }

    #[test]
    fn join_uuids_comma_separates_values() {
        let ids = [
            uuid::Uuid::parse_str("018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91").unwrap(),
            uuid::Uuid::parse_str("0196e5bb-a000-7000-8000-000000000001").unwrap(),
        ];
        assert_eq!(
            join_uuids(&ids),
            "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91, 0196e5bb-a000-7000-8000-000000000001"
        );
    }

    #[test]
    fn join_uuids_empty_slice_is_empty_string() {
        assert_eq!(join_uuids(&[]), "");
    }
}
