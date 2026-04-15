//! Version bump negotiation.

use mmcp_core::memory::BumpIntent;
use semver::Version;

use crate::error::SyncError;

/// Compute the next published version for a memory given the current
/// canonical version string and the editor's requested bump intent.
///
/// Rules:
/// - If `current` is `None` (the memory has never been published),
///   the first published version is always `0.1.0` regardless of
///   the caller's bump intent.
/// - Otherwise the intent is applied to `current` using the same
///   arithmetic as [`BumpIntent::apply`]: patch increments patch,
///   minor increments minor and zeroes patch, major increments
///   major and zeroes the lower fields.
/// - Pre-release and build metadata on `current` are cleared.
pub fn negotiate_next_version(
    current: Option<&str>,
    intent: BumpIntent,
) -> Result<Version, SyncError> {
    match current {
        None => Ok(Version::new(0, 1, 0)),
        Some(raw) => {
            let parsed = Version::parse(raw)?;
            Ok(intent.apply(&parsed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_publish_is_always_0_1_0() {
        for intent in [BumpIntent::Patch, BumpIntent::Minor, BumpIntent::Major] {
            let v = negotiate_next_version(None, intent).unwrap();
            assert_eq!(v, Version::new(0, 1, 0));
        }
    }

    #[test]
    fn patch_intent_increments_patch() {
        let v = negotiate_next_version(Some("1.2.3"), BumpIntent::Patch).unwrap();
        assert_eq!(v, Version::new(1, 2, 4));
    }

    #[test]
    fn minor_intent_resets_patch() {
        let v = negotiate_next_version(Some("1.2.7"), BumpIntent::Minor).unwrap();
        assert_eq!(v, Version::new(1, 3, 0));
    }

    #[test]
    fn major_intent_zeroes_lower_fields() {
        let v = negotiate_next_version(Some("1.4.9"), BumpIntent::Major).unwrap();
        assert_eq!(v, Version::new(2, 0, 0));
    }

    #[test]
    fn prerelease_and_build_metadata_are_cleared() {
        let v =
            negotiate_next_version(Some("1.0.0-rc.1+build.42"), BumpIntent::Patch).unwrap();
        assert_eq!(v, Version::new(1, 0, 1));
    }

    #[test]
    fn invalid_semver_is_rejected() {
        assert!(negotiate_next_version(Some("not-a-version"), BumpIntent::Patch).is_err());
    }
}
