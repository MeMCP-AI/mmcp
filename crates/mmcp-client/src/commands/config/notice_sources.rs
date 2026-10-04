//! [`NoticeSources`], the three configuration files a notice key resolves from.

use std::path::Path;

use mmcp_core::config::{
    ConfigKey, LocalConfig, NoticeLaunch, NoticeLayers, NoticeResolution, ProjectConfig, UserConfig,
};
use mmcp_store::config::{load as load_project, load_local};
use mmcp_store::home::MmcpHome;

use super::ConfigOpError;

/// The loaded user, project and local configuration of one resolution.
/// `local` and `project` are `None` when no project root resolved.
pub struct NoticeSources {
    /// `.mmcp.local.toml`.
    pub local: Option<LocalConfig>,
    /// `.mmcp.toml`.
    pub project: Option<ProjectConfig>,
    /// `~/.mmcp/config.toml`.
    pub user: UserConfig,
}

impl NoticeSources {
    /// Load every file that exists for `project_root`, failing on the first that cannot be read.
    /// For the `config` operations, which report a broken file instead of resolving past it.
    ///
    /// # Errors
    /// [`ConfigOpError::LoadUser`], [`ConfigOpError::LoadProject`] or [`ConfigOpError::LoadLocal`] naming the file.
    pub fn load(home: &MmcpHome, project_root: Option<&Path>) -> Result<Self, ConfigOpError> {
        let user = home.load_user_config().map_err(ConfigOpError::LoadUser)?;
        let (project, local) = match project_root {
            Some(root) => (
                Some(load_project(root).map_err(ConfigOpError::LoadProject)?),
                Some(load_local(root).map_err(ConfigOpError::LoadLocal)?),
            ),
            None => (None, None),
        };
        Ok(Self {
            local,
            project,
            user,
        })
    }

    /// Load the files a bootstrap resolves from, counting a file that fails to load as unset after logging a warning that names it.
    /// The project config is already loaded by the caller, which reports its own failure.
    /// The local file is read only beside a parsed project config.
    #[must_use]
    pub fn load_tolerant(
        home: &MmcpHome,
        project_root: Option<&Path>,
        project: Option<&ProjectConfig>,
    ) -> Self {
        let user = home.load_user_config().unwrap_or_else(|error| {
            tracing::warn!(
                path = %home.user_config_path().display(),
                error = %error,
                "user config failed to load; its notice settings count as unset"
            );
            UserConfig::default()
        });
        let local = match (project_root, project) {
            (Some(root), Some(_)) => Some(load_local(root).unwrap_or_else(|error| {
                tracing::warn!(
                    root = %root.display(),
                    error = %error,
                    "local config failed to load; its notice settings count as unset"
                );
                LocalConfig::default()
            })),
            _ => None,
        };
        Self {
            local,
            project: project.cloned(),
            user,
        }
    }

    /// The effective value of `key` with the layer it comes from.
    #[must_use]
    pub fn resolve(&self, key: ConfigKey, launch: &NoticeLaunch) -> NoticeResolution {
        NoticeLayers::collect(
            key,
            self.local.as_ref(),
            self.project.as_ref(),
            &self.user,
            launch,
        )
        .resolve(key)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use mmcp_core::config::{NoticeSource, NoticeValue};

    use super::super::config_fixture::ConfigFixture;
    use super::*;

    const KEY: ConfigKey = ConfigKey::NoticeMdProject;

    #[test]
    fn a_failing_local_or_user_file_counts_as_unset_and_keeps_the_rest() {
        let fixture = ConfigFixture::new();
        std::fs::create_dir_all(fixture.home.root()).unwrap();
        std::fs::write(fixture.home.user_config_path(), "not = [valid").unwrap();
        std::fs::write(fixture.project.join(".mmcp.local.toml"), "[notic]\n").unwrap();
        let project = load_project(&fixture.project).unwrap();

        let sources =
            NoticeSources::load_tolerant(&fixture.home, Some(&fixture.project), Some(&project));
        let resolution = sources.resolve(KEY, &NoticeLaunch::default());

        assert_eq!(resolution.effective, NoticeValue::On);
        assert_eq!(resolution.source, NoticeSource::Default);
        assert!(sources.project.is_some());
    }

    #[test]
    fn the_local_file_is_read_only_beside_a_parsed_project_config() {
        let fixture = ConfigFixture::new();
        std::fs::write(
            fixture.project.join(".mmcp.local.toml"),
            "[notice.md]\nproject = \"off\"\n",
        )
        .unwrap();

        let without_project =
            NoticeSources::load_tolerant(&fixture.home, Some(&fixture.project), None);
        let project = load_project(&fixture.project).unwrap();
        let with_project =
            NoticeSources::load_tolerant(&fixture.home, Some(&fixture.project), Some(&project));

        assert!(without_project.local.is_none());
        assert_eq!(
            with_project
                .resolve(KEY, &NoticeLaunch::default())
                .effective,
            NoticeValue::Off
        );
    }

    #[test]
    fn no_project_root_leaves_local_and_project_unset() {
        let fixture = ConfigFixture::new();

        let sources = NoticeSources::load(&fixture.home, None).unwrap();

        assert!(sources.local.is_none());
        assert!(sources.project.is_none());
    }
}
