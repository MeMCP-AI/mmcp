//! [`NoticeValue`], whether a notice is emitted.

use serde::{Deserialize, Serialize};

use super::NoticeValueParseError;

/// Value of a `notice.*` key.
/// An absent value is not a variant: the next layer of the resolution decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeValue {
    /// The notice is emitted.
    On,
    /// The notice is not emitted.
    Off,
}

impl NoticeValue {
    /// Every value a configuration file accepts, in the spelling the file uses.
    pub const ACCEPTED: [&'static str; 2] = ["on", "off"];

    /// Spelling of this value in a configuration file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

impl std::str::FromStr for NoticeValue {
    type Err = NoticeValueParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            _ => Err(NoticeValueParseError {
                input: raw.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_spellings_match_as_str_of_every_variant() {
        assert_eq!(
            NoticeValue::ACCEPTED,
            [NoticeValue::On.as_str(), NoticeValue::Off.as_str()]
        );
    }

    #[test]
    fn every_accepted_spelling_parses_back_to_its_variant() {
        assert_eq!("on".parse(), Ok(NoticeValue::On));
        assert_eq!("off".parse(), Ok(NoticeValue::Off));
    }

    #[test]
    fn an_unaccepted_spelling_is_a_typed_error_carrying_the_input() {
        assert_eq!(
            "maybe".parse::<NoticeValue>(),
            Err(NoticeValueParseError {
                input: "maybe".to_string()
            })
        );
        assert!("ON".parse::<NoticeValue>().is_err());
    }
}
