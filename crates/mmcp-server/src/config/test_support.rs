//! Test-only tracing helper shared by `config`'s own submodule tests.
//!
//! Splitting `config/mod.rs` (formerly one file) into `cascade.rs`
//! and `server_config.rs` gave both files' test modules the same need
//! for a warning-counting `tracing::Subscriber`; per the project's
//! commonization rule this lives in its own module instead of each
//! file keeping a copy.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Minimal `tracing::Subscriber` counting `WARN`-level events, so a
/// rejected config value can be asserted to actually log instead of
/// silently discarding it.
pub(crate) struct WarnCounter(pub(crate) Arc<AtomicUsize>);

impl tracing::Subscriber for WarnCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() == tracing::Level::WARN
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}
