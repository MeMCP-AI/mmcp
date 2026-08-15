//! Failure modes specific to the archive-command Tauri layer: the
//! picked-path confinement / size-cap checks around reading an
//! archive file from disk (see `commands/archive.rs`). Dialog
//! plumbing itself is [`crate::error::GuiDialogError`], not
//! duplicated here.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GuiArchiveError {
    /// `value` does not name a recognized memory kind.
    #[error("unknown memory kind '{0}'")]
    UnknownKind(String),

    /// Export was requested with no groups selected.
    #[error("no groups to export")]
    NoGroupsSelected,

    /// `path` was never returned by the archive file picker, so the
    /// read is refused rather than trusting an arbitrary IPC-supplied
    /// filesystem path.
    #[error("archive path {path} was not selected through the file picker")]
    PathNotPicked { path: String },

    /// `path` is `size` bytes, exceeding the `max`-byte read cap.
    #[error("archive {path} is {size} bytes, exceeding the {max} byte limit")]
    TooLarge { path: String, size: u64, max: u64 },

    /// Reading the archive bytes from disk failed.
    #[error("reading archive {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;
    use crate::error::GuiError;

    #[test]
    fn archive_error_chains_to_the_real_source() {
        let source = GuiArchiveError::PathNotPicked {
            path: "/tmp/archive.tar".into(),
        };
        let err: GuiError = source.into();

        assert!(matches!(err, GuiError::Archive(_)));
        let chained = err
            .source()
            .and_then(|s| s.downcast_ref::<GuiArchiveError>())
            .expect("archive source must be preserved");
        assert!(matches!(
            chained,
            GuiArchiveError::PathNotPicked { path } if path == "/tmp/archive.tar"
        ));

        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["kind"], "archive");
        assert_eq!(
            value["message"],
            "archive path /tmp/archive.tar was not selected through the file picker"
        );
    }

    #[test]
    fn archive_too_large_reports_size_and_limit_in_its_message() {
        let err = GuiArchiveError::TooLarge {
            path: "/tmp/big.tar".into(),
            size: 200,
            max: 100,
        };
        assert_eq!(
            err.to_string(),
            "archive /tmp/big.tar is 200 bytes, exceeding the 100 byte limit"
        );
    }
}
