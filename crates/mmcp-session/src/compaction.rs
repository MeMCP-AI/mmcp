//! Compaction detection by transcript signature.
//!
//! Claude Code does not expose a direct compaction signal to MCP
//! servers, so mmcp watches the transcript file associated with the
//! session. A compaction rewrites the transcript in place with a
//! smaller body, so we notice it by comparing a signature of the
//! file against the one we recorded on the previous hook call.
//!
//! The signature encodes both the file length and a SHA-256 digest
//! of its contents. Length alone would be fooled by an edit that
//! happens to preserve size; the full hash is cheap compared to the
//! inference work already happening on the same machine.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SessionError;

/// Signature of a transcript file used for compaction detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSignature {
    /// File length in bytes at the time of signing.
    pub length: u64,

    /// Hex-encoded SHA-256 digest of the full file contents.
    pub digest: String,
}

impl TranscriptSignature {
    /// Serialize the signature as a compact string suitable for
    /// storage in the `sessions.transcript_signature` column.
    #[must_use]
    pub fn encode(&self) -> String {
        format!("{}:{}", self.length, self.digest)
    }

    /// Parse a previously encoded signature.
    #[must_use]
    pub fn decode(encoded: &str) -> Option<Self> {
        let (len, digest) = encoded.split_once(':')?;
        Some(Self {
            length: len.parse().ok()?,
            digest: digest.to_string(),
        })
    }
}

/// Compute a fresh [`TranscriptSignature`] for the file at `path`.
///
/// Returns `Ok(None)` if the file does not exist. Any other I/O
/// failure propagates as [`SessionError::Io`].
pub fn compute_signature(path: &Path) -> Result<Option<TranscriptSignature>, SessionError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok(Some(TranscriptSignature {
        length: bytes.len() as u64,
        digest: hex(&digest),
    }))
}

/// Decide whether a compaction occurred between `previous` and
/// `current`.
///
/// Rules:
/// - No previous signature means we are seeing the transcript for
///   the first time. Not a compaction.
/// - Current length strictly smaller than previous length is a
///   compaction (the only way the transcript file shrinks).
/// - Same length but different digest is *also* a compaction. Edit
///   compactions can preserve size in pathological cases, so the
///   digest mismatch is the backstop.
/// - Current length greater or equal with same length-prefix digest
///   stability is normal progression.
#[must_use]
pub fn detect_compaction(
    previous: Option<&TranscriptSignature>,
    current: &TranscriptSignature,
) -> bool {
    let Some(prev) = previous else {
        return false;
    };
    if current.length < prev.length {
        return true;
    }
    if current.length == prev.length && current.digest != prev.digest {
        return true;
    }
    false
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn signature_round_trips_through_encode_decode() {
        let sig = TranscriptSignature {
            length: 42,
            digest: "abcd".into(),
        };
        let encoded = sig.encode();
        let decoded = TranscriptSignature::decode(&encoded).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn missing_file_produces_no_signature() {
        // Use a process-local unique name; no need to pull the
        // uuid crate just for a probe path.
        let path = std::env::temp_dir().join(format!(
            "mmcp-nonexistent-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let sig = compute_signature(&path).unwrap();
        assert!(sig.is_none());
    }

    #[test]
    fn signature_changes_with_content() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(tmp.path()).unwrap();
            f.write_all(b"hello world").unwrap();
        }
        let s1 = compute_signature(tmp.path()).unwrap().unwrap();
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(tmp.path())
                .unwrap();
            f.write_all(b"hello world!").unwrap();
        }
        let s2 = compute_signature(tmp.path()).unwrap().unwrap();
        assert_ne!(s1, s2);
    }

    #[test]
    fn detect_compaction_no_previous_is_false() {
        let current = TranscriptSignature {
            length: 10,
            digest: "a".into(),
        };
        assert!(!detect_compaction(None, &current));
    }

    #[test]
    fn shrinking_transcript_is_compaction() {
        let prev = TranscriptSignature {
            length: 100,
            digest: "aaa".into(),
        };
        let current = TranscriptSignature {
            length: 40,
            digest: "bbb".into(),
        };
        assert!(detect_compaction(Some(&prev), &current));
    }

    #[test]
    fn growing_transcript_is_not_compaction() {
        let prev = TranscriptSignature {
            length: 100,
            digest: "aaa".into(),
        };
        let current = TranscriptSignature {
            length: 150,
            digest: "ccc".into(),
        };
        assert!(!detect_compaction(Some(&prev), &current));
    }

    #[test]
    fn same_length_different_digest_is_compaction() {
        let prev = TranscriptSignature {
            length: 100,
            digest: "aaa".into(),
        };
        let current = TranscriptSignature {
            length: 100,
            digest: "zzz".into(),
        };
        assert!(detect_compaction(Some(&prev), &current));
    }

    #[test]
    fn unchanged_signature_is_not_compaction() {
        let sig = TranscriptSignature {
            length: 100,
            digest: "aaa".into(),
        };
        assert!(!detect_compaction(Some(&sig), &sig));
    }
}
