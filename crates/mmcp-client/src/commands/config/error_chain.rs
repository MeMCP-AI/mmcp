//! One line that carries an error and every cause under it.

use std::error::Error;

/// The message of `error` followed by the message of each cause, each joined to the one before by a colon.
/// A sentence's final period is dropped where a cause follows, so the line never reads `.:`.
#[must_use]
pub fn message_with_causes(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        let text = inner.to_string();
        // An error that already prints its cause is not told twice.
        if !message.contains(&text) {
            message = format!("{}: {text}", message.trim_end_matches('.'));
        }
        cause = inner.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Layer {
        message: &'static str,
        cause: Option<Box<Layer>>,
    }

    impl fmt::Display for Layer {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    impl Error for Layer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.cause
                .as_deref()
                .map(|cause| cause as &(dyn Error + 'static))
        }
    }

    fn layer(message: &'static str, cause: Option<Layer>) -> Layer {
        Layer {
            message,
            cause: cause.map(Box::new),
        }
    }

    #[test]
    fn an_error_without_a_cause_is_its_own_message_period_included() {
        assert_eq!(
            message_with_causes(&layer("The config could not be loaded.", None)),
            "The config could not be loaded."
        );
    }

    #[test]
    fn a_cause_follows_the_sentence_without_its_period() {
        let error = layer(
            "The config could not be loaded.",
            Some(layer("expected a table", None)),
        );
        assert_eq!(
            message_with_causes(&error),
            "The config could not be loaded: expected a table"
        );
    }

    #[test]
    fn a_cause_the_message_already_prints_is_not_repeated() {
        let error = layer(
            "failed to parse TOML: bad key",
            Some(layer("bad key", None)),
        );
        assert_eq!(message_with_causes(&error), "failed to parse TOML: bad key");
    }

    #[test]
    fn every_cause_of_the_chain_is_joined_in_order() {
        let error = layer("Outer.", Some(layer("Middle.", Some(layer("Inner", None)))));
        assert_eq!(message_with_causes(&error), "Outer: Middle: Inner");
    }
}
