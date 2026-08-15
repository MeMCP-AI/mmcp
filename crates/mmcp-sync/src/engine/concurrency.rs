//! Generic bounded-concurrency runner shared by `push`, `pull`'s
//! fast-forward step, and `fetch`. Extracted so the three verbs stop
//! duplicating the same tag-with-index / `buffer_unordered` /
//! sort-back-into-order skeleton (see [`run_bounded`]'s own doc
//! comment for the ordering rationale). This is the third copy of
//! that skeleton that would otherwise have existed; per this
//! project's commonization rule, shared logic used by three or more
//! consumers lives in its own owning module rather than staying
//! duplicated across them.

use std::future::Future;

use futures_util::StreamExt;
use futures_util::stream;

use super::defaults::MAX_CONCURRENT_GROUP_TRANSFERS;
use crate::error::SyncError;

/// Run `op` once per item in `items`, at most
/// [`MAX_CONCURRENT_GROUP_TRANSFERS`] concurrently, and return one
/// `(key, outcome)` pair per item in `items`' ORIGINAL order.
///
/// `buffer_unordered` completes tasks in COMPLETION order, not
/// submission order: this tags each task with its original index
/// before scheduling, then sorts the collected results back into
/// that order afterward, so `push` / `pull` / `fetch` can each keep
/// their documented "in iteration order" report contract regardless
/// of which network round trip actually finishes first.
///
/// `key_of` extracts the identity (a group id) that both the success
/// and the failure branches downstream attribute their outcome to,
/// so a caller never has to re-derive it from an already-consumed
/// item. Every item is attempted to completion regardless of an
/// earlier item's outcome; nothing here short-circuits on the first
/// failure.
pub(super) async fn run_bounded<T, K, R, F, Fut>(
    items: Vec<T>,
    key_of: impl Fn(&T) -> K,
    op: F,
) -> Vec<(K, Result<R, SyncError>)>
where
    K: Copy,
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<R, SyncError>>,
{
    let mut tagged: Vec<(usize, K, Result<R, SyncError>)> =
        stream::iter(items.into_iter().enumerate())
            .map(|(index, item)| {
                let key = key_of(&item);
                let outcome = op(item);
                async move { (index, key, outcome.await) }
            })
            .buffer_unordered(MAX_CONCURRENT_GROUP_TRANSFERS)
            .collect()
            .await;
    tagged.sort_by_key(|(index, ..)| *index);
    tagged
        .into_iter()
        .map(|(_, key, outcome)| (key, outcome))
        .collect()
}
