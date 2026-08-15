//! Shared subscription plumbing.
//!
//! `commands::serve` (the `subscribe` / `unsubscribe` / `bootstrap_context`
//! MCP tools), `commands::subscribe` (the `mmcp subscribe` /
//! `mmcp unsubscribe` CLI subcommands), and `commands::bootstrap`
//! (the `mmcp bootstrap` CLI mirror) all need the same subscription
//! domain logic: validating and mutating a project's
//! `[subscriptions]` config (see [`target`]), and turning that
//! config into the concrete set of memory addresses it pulls into
//! scope (see [`resolve`]). Both live here, their own concern-named module, so
//! no single caller owns them and every caller imports downward
//! instead of reaching into a peer command module.

mod resolve;
mod target;

pub use resolve::resolve_subscribed_reads;
pub use target::{
    SubscribeError, SubscribeMcpArgs, SubscriptionAction, SubscriptionKind, apply_subscription,
    resolve_project_root, validate_subscription_target,
};
