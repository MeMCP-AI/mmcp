//! The per-provider cookie carrying a pending OAuth flow token between authorize and callback.
//!
//! The cookie is `HttpOnly`, `SameSite=Lax` (the callback is a cross-site top-level navigation) and path `/`.
//! On an HTTPS origin it is also `Secure` and carries the `__Host-` name prefix.

use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::header::{COOKIE, InvalidHeaderValue};
use time::Duration;
use tower_sessions::cookie::{Cookie, SameSite};

use crate::routes::defaults::{
    HOST_COOKIE_NAME_PREFIX, OAUTH_FLOW_COOKIE_NAME_PREFIX, OAUTH_FLOW_LIFETIME,
};

/// Whether `slug` can be appended to the flow cookie name unchanged.
///
/// The name must stay a cookie token: ASCII letters, digits, `-` and `_` always are, and a configured slug
/// holding a separator such as `;`, `=` or a space would corrupt the `Set-Cookie` line and the `Cookie` parse.
pub(crate) fn provider_slug_is_cookie_safe(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn cookie_name(provider: &str, secure_origin: bool) -> String {
    let host_prefix = if secure_origin {
        HOST_COOKIE_NAME_PREFIX
    } else {
        ""
    };
    format!("{host_prefix}{OAUTH_FLOW_COOKIE_NAME_PREFIX}{provider}")
}

fn flow_cookie(
    provider: &str,
    value: String,
    secure_origin: bool,
    max_age: Duration,
) -> Cookie<'static> {
    Cookie::build((cookie_name(provider, secure_origin), value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure_origin)
        .path("/")
        .max_age(max_age)
        .build()
}

/// `Set-Cookie` value carrying `token` for the flow against `provider`, valid for [`OAUTH_FLOW_LIFETIME`].
pub(crate) fn set_header(
    provider: &str,
    token: &str,
    secure_origin: bool,
) -> Result<HeaderValue, InvalidHeaderValue> {
    HeaderValue::from_str(
        &flow_cookie(
            provider,
            token.to_owned(),
            secure_origin,
            OAUTH_FLOW_LIFETIME,
        )
        .to_string(),
    )
}

/// `Set-Cookie` value that removes the flow cookie of `provider` from the browser.
pub(crate) fn clear_header(
    provider: &str,
    secure_origin: bool,
) -> Result<HeaderValue, InvalidHeaderValue> {
    HeaderValue::from_str(
        &flow_cookie(provider, String::new(), secure_origin, Duration::ZERO).to_string(),
    )
}

/// The flow token the request carries for `provider`, if any.
pub(crate) fn read_token(
    headers: &HeaderMap,
    provider: &str,
    secure_origin: bool,
) -> Option<String> {
    let name = cookie_name(provider, secure_origin);
    for header in headers.get_all(COOKIE) {
        let Ok(text) = header.to_str() else {
            continue;
        };
        for cookie in Cookie::split_parse(text).flatten() {
            if cookie.name() == name {
                return Some(cookie.value().to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn header_text(value: HeaderValue) -> String {
        value.to_str().unwrap().to_owned()
    }

    #[test]
    fn provider_slug_is_cookie_safe_accepts_tokens_and_refuses_separators() {
        for safe in ["github", "git-hub", "git_hub2", "A1"] {
            assert!(provider_slug_is_cookie_safe(safe), "{safe}");
        }
        for unsafe_slug in [
            "", "git hub", "git;hub", "git=hub", "git,hub", "git\"hub", "gité",
        ] {
            assert!(
                !provider_slug_is_cookie_safe(unsafe_slug),
                "{unsafe_slug:?}"
            );
        }
    }

    #[test]
    fn flow_cookie_is_httponly_lax_scoped_to_the_site_and_bounded_in_time() {
        let text = header_text(set_header("github", "token-value", false).unwrap());

        assert!(
            text.starts_with("mmcp_oauth_flow_github=token-value"),
            "{text}"
        );
        assert!(text.contains("HttpOnly"), "{text}");
        assert!(text.contains("SameSite=Lax"), "{text}");
        assert!(text.contains("Path=/"), "{text}");
        assert!(
            text.contains(&format!("Max-Age={}", OAUTH_FLOW_LIFETIME.whole_seconds())),
            "{text}"
        );
        assert!(
            !text.contains("Secure"),
            "an HTTP origin must not set Secure: {text}"
        );
    }

    #[test]
    fn flow_cookie_on_an_https_origin_is_secure_and_host_prefixed() {
        let text = header_text(set_header("github", "token-value", true).unwrap());

        assert!(
            text.starts_with("__Host-mmcp_oauth_flow_github=token-value"),
            "{text}"
        );
        assert!(text.contains("Secure"), "{text}");
        assert!(text.contains("Path=/"), "{text}");
        assert!(
            !text.contains("Domain"),
            "a __Host- cookie must not set Domain: {text}"
        );
    }

    #[test]
    fn clearing_the_flow_cookie_expires_it_at_once() {
        let text = header_text(clear_header("github", false).unwrap());

        assert!(text.starts_with("mmcp_oauth_flow_github="), "{text}");
        assert!(text.contains("Max-Age=0"), "{text}");
        assert!(text.contains("Path=/"), "{text}");
    }

    #[test]
    fn read_token_returns_the_cookie_of_the_requested_provider_only() {
        let mut headers = HeaderMap::new();
        headers.append(
            COOKIE,
            HeaderValue::from_static("id=session-value; mmcp_oauth_flow_other=other-token"),
        );
        headers.append(
            COOKIE,
            HeaderValue::from_static("mmcp_oauth_flow_github=github-token"),
        );

        assert_eq!(
            read_token(&headers, "github", false).as_deref(),
            Some("github-token")
        );
        assert_eq!(
            read_token(&headers, "other", false).as_deref(),
            Some("other-token")
        );
        assert_eq!(read_token(&headers, "missing", false), None);
    }

    #[test]
    fn read_token_ignores_the_unprefixed_name_on_an_https_origin() {
        let mut headers = HeaderMap::new();
        headers.append(
            COOKIE,
            HeaderValue::from_static("mmcp_oauth_flow_github=planted-token"),
        );

        assert_eq!(read_token(&headers, "github", true), None);
    }
}
