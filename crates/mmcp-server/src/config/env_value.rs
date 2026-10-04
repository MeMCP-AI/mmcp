//! Blank-as-absent reading of optional environment values.

/// Trimmed `raw`, `None` when it is unset or only whitespace.
///
/// A blank value is the conventional unset form.
/// Compose interpolation of an unset `${VAR:-}` and a copy-pasted `.env` line both produce one.
pub(super) fn non_blank(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_value_is_absent() {
        assert_eq!(non_blank(None), None);
    }

    #[test]
    fn empty_and_whitespace_only_values_are_absent() {
        assert_eq!(non_blank(Some("")), None);
        assert_eq!(non_blank(Some("   \n\t")), None);
    }

    #[test]
    fn present_value_is_returned_without_surrounding_whitespace() {
        assert_eq!(non_blank(Some("  s3cr3t\n")), Some("s3cr3t"));
        assert_eq!(non_blank(Some("abc")), Some("abc"));
    }
}
