//! [`PushScope`]: caller-supplied selector for which
//! [`crate::BoundRemote`]s [`crate::SyncEngine::push`] targets.

/// Caller-supplied selector for which [`crate::BoundRemote`]s
/// [`crate::SyncEngine::push`] targets.
///
/// `fetch` and `pull` are unaffected by this scoping: `fetch` always
/// aggregates every `mmcp-server`-kind remote plus the in-scope
/// `direct-git` remote, and `pull` always fast-forwards from exactly
/// the default remote. Only `push` fans out across a caller-chosen
/// subset, since pushing is the one operation with a real cost to
/// targeting more than intended.
///
/// Own file, split out of the sibling module that defines
/// [`crate::BoundRemote`] / [`crate::RemoteTransport`]: a push-scope
/// SELECTOR is a distinct concern from those types, which describe
/// one already-bound remote rather than choosing among several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushScope {
    /// The one `BoundRemote` marked `default: true`, or the sole
    /// bound remote when exactly one exists and none is marked
    /// default.
    Default,
    /// Every `BoundRemote` whose `include_in_push_all` is `true`.
    All,
    /// The one `BoundRemote` whose `name` matches exactly.
    Named(String),
}
