//! Version bump intent hinted by the editor (AI or user).

use semver::Version;
use serde::{Deserialize, Serialize};

/// Bump level requested by whoever edited a memory.
///
/// The AI (or a human editor via the WebUI) expresses *intent* rather
/// than choosing an explicit version number. The actual version is
/// assigned by the server at push time against the current canonical
/// version, which prevents offline edits from colliding on a number.
///
/// Default is [`Minor`](BumpIntent::Minor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BumpIntent {
    /// Wording fix, typo correction, clarification, example tweak.
    /// Previous behavior unchanged.
    Patch,

    /// Rule addition or removal, new section. Guidance evolves without
    /// contradicting prior advice.
    #[default]
    Minor,

    /// New implementation, structural refactor, or a reversal of a
    /// prior rule. Readers may need to re-check earlier assumptions.
    Major,
}

impl BumpIntent {
    /// Apply this bump intent to `current`, returning the resulting
    /// [`semver::Version`].
    ///
    /// Uses strict semver semantics: minor and major bumps zero the
    /// lower fields; pre-release and build metadata are cleared.
    #[must_use]
    pub fn apply(self, current: &Version) -> Version {
        match self {
            BumpIntent::Patch => Version::new(current.major, current.minor, current.patch + 1),
            BumpIntent::Minor => Version::new(current.major, current.minor + 1, 0),
            BumpIntent::Major => Version::new(current.major + 1, 0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_bump_increments_patch_field() {
        let current = Version::new(1, 2, 3);
        assert_eq!(BumpIntent::Patch.apply(&current), Version::new(1, 2, 4));
    }

    #[test]
    fn minor_bump_resets_patch_field() {
        let current = Version::new(1, 2, 7);
        assert_eq!(BumpIntent::Minor.apply(&current), Version::new(1, 3, 0));
    }

    #[test]
    fn major_bump_resets_minor_and_patch_fields() {
        let current = Version::new(1, 4, 9);
        assert_eq!(BumpIntent::Major.apply(&current), Version::new(2, 0, 0));
    }

    #[test]
    fn default_bump_intent_is_minor() {
        assert_eq!(BumpIntent::default(), BumpIntent::Minor);
    }
}
