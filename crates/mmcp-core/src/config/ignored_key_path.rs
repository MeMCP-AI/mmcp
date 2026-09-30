//! Dotted key path of a key a lenient deserialization ignored.

use serde_ignored::Path;

/// Render `path` as the dotted key path a configuration file spells.
/// Transparent wrappers (`Option`, newtypes) add no segment, and a sequence index renders as `[index]`.
pub(super) fn ignored_key_path(path: &Path<'_>) -> String {
    match path {
        Path::Root => String::new(),
        Path::Seq { parent, index } => format!("{}[{index}]", ignored_key_path(parent)),
        Path::Map { parent, key } => {
            let parent = ignored_key_path(parent);
            if parent.is_empty() {
                key.clone()
            } else {
                format!("{parent}.{key}")
            }
        }
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => ignored_key_path(parent),
    }
}
