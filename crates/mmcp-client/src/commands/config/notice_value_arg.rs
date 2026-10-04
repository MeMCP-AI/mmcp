//! [`NoticeValueArg`], a notice value as the `config` tool, the CLI and the serve flags name it.

use clap::ValueEnum;
use mmcp_core::config::NoticeValue;
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

// The values a `notice.*` setting accepts.
// A plain comment: a type-level doc comment would be sent to every client as schema text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
#[schemars(crate = "rmcp::schemars")]
pub enum NoticeValueArg {
    On,
    Off,
}

impl From<NoticeValueArg> for NoticeValue {
    fn from(arg: NoticeValueArg) -> Self {
        match arg {
            NoticeValueArg::On => Self::On,
            NoticeValueArg::Off => Self::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_arguments_are_exactly_the_accepted_spellings() {
        let names: Vec<String> = NoticeValueArg::value_variants()
            .iter()
            .map(|variant| variant.to_possible_value().unwrap().get_name().to_owned())
            .collect();
        assert_eq!(names, NoticeValue::ACCEPTED);
    }

    #[test]
    fn every_argument_converts_to_the_value_of_the_same_spelling() {
        for variant in NoticeValueArg::value_variants() {
            let name = variant.to_possible_value().unwrap().get_name().to_owned();
            assert_eq!(NoticeValue::from(*variant).as_str(), name);
        }
    }
}
