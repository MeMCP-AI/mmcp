//! [`SyncConfig`]: remote sync configuration shared by project and
//! user level.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::env_vars::{SYNC_PUSH_TOKEN_ENV, SYNC_TOKEN_ENV};
use crate::config::{ConfigError, Remote};

/// Remote sync configuration, reused verbatim on both
/// [`crate::config::ProjectConfig`] and [`crate::config::UserConfig`]
/// so the sync surface is uniform between project and user level.
///
/// Carries no credential field: every token is env-only
/// ([`SyncConfig::resolve_token`], [`SyncConfig::resolve_push_token`],
/// [`SyncConfig::token_env_name`], [`SyncConfig::push_token_env_name`]),
/// so a `.mmcp.toml` rewrite can never persist a secret to a file
/// that convention tracks in git.
///
/// Always defaulted (not `Option<SyncConfig>`): a config with no
/// `[sync]` table at all still parses to `SyncConfig::default()`
/// (`server_url: None`, `remotes` empty), so a toggle that lives on
/// the enclosing config (see
/// [`crate::config::ProjectConfig::project_remote_only`]) stays
/// reachable even when this struct itself is unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    /// Legacy shorthand for a single `mmcp-server` remote. Purely
    /// additive: composed with `remotes` at resolution time (a later
    /// wave), never validated as mutually exclusive with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,

    /// Any number of named remotes. Kept empty (and absent from
    /// rendered TOML) on configs that only use the legacy
    /// `server_url` shorthand.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remotes: Vec<Remote>,
}

impl SyncConfig {
    /// True when this config declares neither the legacy `server_url`
    /// shorthand nor any `remotes` entry. Replaces the old
    /// `Option<SyncConfig>::is_none()` check now that this struct is
    /// always present and always defaulted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.server_url.is_none() && self.remotes.is_empty()
    }

    /// Env var name carrying the control-plane bearer token for the
    /// remote named `remote_name`, `MMCP_SYNC_TOKEN_<NAME>`.
    ///
    /// Derivation rule, exact and stable (a later wave, mmcp-sync,
    /// reads env vars produced by this exact rule): ASCII-uppercase
    /// `remote_name`, then replace every `-` with `_` so a hyphenated
    /// remote name still yields a portable env var identifier. See
    /// [`SyncConfig::normalized_remote_name`], the single owning
    /// definition of this rule. Deliberately NOT a free-text config
    /// field: see [`crate::config::RemoteAuth::Bearer`].
    #[must_use]
    pub fn token_env_name(remote_name: &str) -> String {
        format!(
            "MMCP_SYNC_TOKEN_{}",
            Self::normalized_remote_name(remote_name)
        )
    }

    /// Env var name carrying the content-plane git push credential
    /// for the remote named `remote_name`,
    /// `MMCP_SYNC_PUSH_TOKEN_<NAME>`. Same derivation rule as
    /// [`SyncConfig::token_env_name`].
    #[must_use]
    pub fn push_token_env_name(remote_name: &str) -> String {
        format!(
            "MMCP_SYNC_PUSH_TOKEN_{}",
            Self::normalized_remote_name(remote_name)
        )
    }

    /// Injective key used both to derive a remote's credential env
    /// var suffix ([`SyncConfig::token_env_name`],
    /// [`SyncConfig::push_token_env_name`]) and to decide whether two
    /// remote names collide: ASCII-uppercase `remote_name`, then
    /// replace every `-` with `_`.
    ///
    /// Every place that checks a remote name for uniqueness or for
    /// spoofing a reserved name MUST compare this normalized form,
    /// never the raw name: `token_env_name` collapses `prod-eu` and
    /// `prod_eu` (or `PROD-EU`) to the identical env var
    /// `MMCP_SYNC_TOKEN_PROD_EU`, so two differently-spelled remotes
    /// that pass a raw-string uniqueness check still share one
    /// credential slot. A user-level remote `prod-eu` and a
    /// git-tracked project-level remote `prod_eu` pointed at an
    /// attacker URL would otherwise both read the user's real token
    /// from that shared env var, exfiltrating it to the attacker's
    /// remote on the next fetch. `pub` (not crate-private): both
    /// [`SyncConfig::validate`] (same-file check) and
    /// `mmcp_store::sync::remotes` (cross-level check, reserved-name
    /// guard) compare against this exact key, so the transform has
    /// exactly one owning definition instead of being reimplemented
    /// at each call site.
    #[must_use]
    pub fn normalized_remote_name(remote_name: &str) -> String {
        remote_name.to_ascii_uppercase().replace('-', "_")
    }

    /// Single-file, single-level validation of this `remotes` list:
    /// rejects two entries whose [`SyncConfig::normalized_remote_name`]
    /// collide (not just entries sharing a raw `name`), and rejects
    /// more than one entry marked `default = true`. Comparing the
    /// normalized form here closes the same credential-collision hole
    /// [`SyncConfig::normalized_remote_name`]'s own doc comment
    /// describes, for two colliding names declared in the SAME file.
    ///
    /// Cross-level validation, a project remote colliding with a user
    /// remote by name, or picking a winner between two
    /// `default = true` remotes declared at different levels, needs
    /// both config files loaded together and is deliberately left to
    /// the resolver, a later wave.
    ///
    /// # Errors
    /// [`ConfigError::DuplicateRemoteName`] (reported with the
    /// colliding entry's own original, unnormalized `name`, for a
    /// readable message) or [`ConfigError::MultipleDefaultRemotes`].
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let mut seen_normalized: HashSet<String> = HashSet::new();
        for remote in &self.remotes {
            if !seen_normalized.insert(Self::normalized_remote_name(remote.name())) {
                return Err(ConfigError::DuplicateRemoteName {
                    name: remote.name().to_string(),
                });
            }
        }

        let default_names: Vec<String> = self
            .remotes
            .iter()
            .filter(|remote| remote.is_default())
            .map(|remote| remote.name().to_string())
            .collect();
        if default_names.len() > 1 {
            return Err(ConfigError::MultipleDefaultRemotes {
                names: default_names,
            });
        }

        Ok(())
    }

    /// Effective control-plane bearer token for `/sync/*` requests,
    /// reading `MMCP_SYNC_TOKEN` from the real process environment.
    /// Env-only: no file-backed fallback exists, so this credential
    /// never round-trips through `.mmcp.toml`.
    #[must_use]
    pub fn resolve_token(&self) -> Option<String> {
        Self::resolve_token_with(|key| std::env::var(key).ok())
    }

    /// Same resolution as [`SyncConfig::resolve_token`], with the
    /// env lookup injected so tests never touch `std::env`, which
    /// would race under cargo's default parallel test runner. An
    /// empty env value is treated as absent, matching
    /// `SyncClient::git_credentials`'s existing
    /// `Some(token) if !token.is_empty()` pattern in the
    /// `mmcp-sync` crate.
    fn resolve_token_with(get: impl Fn(&str) -> Option<String>) -> Option<String> {
        get(SYNC_TOKEN_ENV).filter(|token| !token.is_empty())
    }

    /// Effective content-plane git push credential, reading
    /// `MMCP_SYNC_PUSH_TOKEN` from the real process environment.
    /// Structurally a twin of [`SyncConfig::resolve_token`]: env-only,
    /// same empty-value handling, distinct env var and distinct
    /// destination (`SyncClient::with_push_credential`, never
    /// `SyncClient::with_bearer`).
    #[must_use]
    pub fn resolve_push_token(&self) -> Option<String> {
        Self::resolve_push_token_with(|key| std::env::var(key).ok())
    }

    /// Same resolution as [`SyncConfig::resolve_push_token`], with
    /// the env lookup injected for the same race-avoidance reason as
    /// [`SyncConfig::resolve_token_with`].
    fn resolve_push_token_with(get: impl Fn(&str) -> Option<String>) -> Option<String> {
        get(SYNC_PUSH_TOKEN_ENV).filter(|token| !token.is_empty())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn resolve_token_with_reads_the_env_value() {
        let resolved = SyncConfig::resolve_token_with(|key| {
            (key == SYNC_TOKEN_ENV).then(|| "env-token".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("env-token"));
    }

    #[test]
    fn resolve_token_with_is_none_when_env_absent() {
        let resolved = SyncConfig::resolve_token_with(|_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_token_with_treats_an_empty_env_value_as_absent() {
        let resolved =
            SyncConfig::resolve_token_with(|key| (key == SYNC_TOKEN_ENV).then(String::new));
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_push_token_with_reads_the_env_value() {
        let resolved = SyncConfig::resolve_push_token_with(|key| {
            (key == SYNC_PUSH_TOKEN_ENV).then(|| "env-push-token".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("env-push-token"));
    }

    #[test]
    fn resolve_push_token_with_is_none_when_env_absent() {
        let resolved = SyncConfig::resolve_push_token_with(|_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_push_token_with_treats_an_empty_env_value_as_absent() {
        let resolved = SyncConfig::resolve_push_token_with(|key| {
            (key == SYNC_PUSH_TOKEN_ENV).then(String::new)
        });
        assert!(resolved.is_none());
    }

    /// The two resolvers read distinct env vars: setting one must
    /// never leak into the other's result. Structural regression
    /// test for the credential-plane split.
    #[test]
    fn resolve_token_and_resolve_push_token_read_distinct_env_vars() {
        let get = |key: &str| match key {
            k if k == SYNC_TOKEN_ENV => Some("control-plane-value".to_string()),
            k if k == SYNC_PUSH_TOKEN_ENV => Some("content-plane-value".to_string()),
            _ => None,
        };
        assert_eq!(
            SyncConfig::resolve_token_with(get).as_deref(),
            Some("control-plane-value")
        );
        assert_eq!(
            SyncConfig::resolve_push_token_with(get).as_deref(),
            Some("content-plane-value")
        );
    }

    #[test]
    fn token_env_name_uses_the_documented_derivation_rule() {
        assert_eq!(
            SyncConfig::token_env_name("primary"),
            "MMCP_SYNC_TOKEN_PRIMARY"
        );
        assert_eq!(
            SyncConfig::token_env_name("prod-eu"),
            "MMCP_SYNC_TOKEN_PROD_EU"
        );
    }

    #[test]
    fn push_token_env_name_uses_the_documented_derivation_rule() {
        assert_eq!(
            SyncConfig::push_token_env_name("primary"),
            "MMCP_SYNC_PUSH_TOKEN_PRIMARY"
        );
        assert_eq!(
            SyncConfig::push_token_env_name("prod-eu"),
            "MMCP_SYNC_PUSH_TOKEN_PROD_EU"
        );
    }

    #[test]
    fn normalized_remote_name_collapses_case_and_hyphen_underscore_variants() {
        assert_eq!(
            SyncConfig::normalized_remote_name("prod-eu"),
            SyncConfig::normalized_remote_name("prod_eu")
        );
        assert_eq!(
            SyncConfig::normalized_remote_name("prod-eu"),
            SyncConfig::normalized_remote_name("PROD-EU")
        );
    }
}
