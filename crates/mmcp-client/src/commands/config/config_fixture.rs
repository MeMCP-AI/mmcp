//! Test fixture: a scratch home and a scratch project for the `config` operations.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use mmcp_core::config::NoticeLaunch;
use mmcp_git::GitError;
use mmcp_git::checkout::Exclusion;
use mmcp_store::home::MmcpHome;
use tempfile::TempDir;

use super::ConfigEnvironment;

/// Project UUID written to the scratch `.mmcp.toml`.
pub(super) const PROJECT_UUID: &str = "018f7c3e-4d2a-7b1f-9e5c-6a8d2f0b4c91";

/// Scratch global excludes file the stub reports as appended to.
pub(super) const SCRATCH_EXCLUDES_FILE: &str = "scratch-global-ignore";

/// A scratch home with an empty user config and a scratch project carrying only its identity.
pub(super) struct ConfigFixture {
    _tmp: TempDir,
    pub(super) home: MmcpHome,
    pub(super) project: PathBuf,
    pub(super) launch: NoticeLaunch,
}

impl ConfigFixture {
    pub(super) fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let home = MmcpHome::from_root(tmp.path().join("mmcp-home"));
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).expect("mkdir project");
        std::fs::write(
            project.join(".mmcp.toml"),
            format!("project_uuid = \"{PROJECT_UUID}\"\n"),
        )
        .expect("write .mmcp.toml");
        Self {
            _tmp: tmp,
            home,
            project,
            launch: NoticeLaunch::default(),
        }
    }

    /// An environment whose local-file exclusion is `exclude_local`, never the real global excludes file.
    pub(super) fn environment(
        &self,
        exclude_local: fn(&Path) -> Result<Exclusion, GitError>,
    ) -> ConfigEnvironment<'_> {
        ConfigEnvironment {
            home: &self.home,
            launch: &self.launch,
            exclude_local,
        }
    }

    pub(super) fn project_toml(&self) -> String {
        std::fs::read_to_string(self.project.join(".mmcp.toml")).expect("read .mmcp.toml")
    }
}

/// Reports the pattern as appended to the scratch excludes file.
pub(super) fn appends_to_scratch_excludes(_path: &Path) -> Result<Exclusion, GitError> {
    Ok(Exclusion::Appended {
        file: PathBuf::from(SCRATCH_EXCLUDES_FILE),
    })
}

/// Reports the file as already excluded.
pub(super) fn already_excluded(_path: &Path) -> Result<Exclusion, GitError> {
    Ok(Exclusion::AlreadyExcluded)
}

/// Reports the file as outside any repository.
pub(super) fn outside_a_repository(_path: &Path) -> Result<Exclusion, GitError> {
    Ok(Exclusion::NotInRepository)
}

/// Fails the way an unwritable global excludes file does.
pub(super) fn fails_to_exclude(path: &Path) -> Result<Exclusion, GitError> {
    Err(GitError::GlobalExcludesWrite {
        path: path.display().to_string(),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "scratch refusal"),
    })
}
