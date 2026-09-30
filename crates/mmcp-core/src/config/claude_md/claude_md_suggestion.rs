//! [`ClaudeMdSuggestion`], whether mmcp suggests its managed block for a project's own CLAUDE.md.

use serde::{Deserialize, Serialize};

/// Setting value of `claude_md.project_file_suggestion`.
/// An absent value is not a variant: the next layer of the resolution decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeMdSuggestion {
    /// mmcp suggests its managed block for the project's CLAUDE.md.
    Suggest,
    /// mmcp stops suggesting it.
    Decline,
}

impl ClaudeMdSuggestion {
    /// Every value a configuration file accepts, in the spelling the file uses.
    pub const ACCEPTED: &'static [&'static str] = &["suggest", "decline"];

    /// Spelling of this value in a configuration file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Suggest => "suggest",
            Self::Decline => "decline",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_spellings_match_as_str_of_every_variant() {
        assert_eq!(
            ClaudeMdSuggestion::ACCEPTED,
            [
                ClaudeMdSuggestion::Suggest.as_str(),
                ClaudeMdSuggestion::Decline.as_str(),
            ]
        );
    }
}
