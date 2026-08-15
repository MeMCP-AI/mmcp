//! Native in-process git backend using `gix`.

mod backend;
mod defaults;
mod repo_ops;

pub use backend::NativeBackend;
