//! [`LaunchValues`], the values one key was launched with.

use super::NoticeValue;

/// The two launch layers of one key, collected once by the argument parser.
/// They act as a per-registration switch: they rank below the local and project layers and above the user layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchValues {
    /// Value of the launch flag.
    pub flag: Option<NoticeValue>,
    /// Value of the launch environment variable.
    pub environment: Option<NoticeValue>,
}
