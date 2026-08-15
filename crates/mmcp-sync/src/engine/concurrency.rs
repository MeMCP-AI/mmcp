//! Generic bounded-concurrency runner shared by `push`, `pull`'s
//! fast-forward step, and `fetch`, and reused across crates by
//! `mmcp-server`'s `/sync/manifest` row lookup. Extracted so callers
//! stop duplicating the same tag-with-index / `buffer_unordered` /
//! sort-back-into-order skeleton (see [`run_bounded`]'s own doc
//! comment for the ordering rationale). Lives in `mmcp-sync`, not
//! `mmcp-core`: `mmcp-core` is documented as a pure, synchronous data
//! model with no async entry points, while `mmcp-sync` already sits
//! on the dependency path shared by every consumer of this helper
//! (`mmcp-server` and `mmcp-store` both depend on `mmcp-sync`).

use std::future::Future;

use futures_util::StreamExt;
use futures_util::stream;

/// Run `op` once per item in `items`, at most `concurrency` at a
/// time, and return one `(key, outcome)` pair per item in `items`'
/// ORIGINAL order.
///
/// `buffer_unordered` completes tasks in COMPLETION order, not
/// submission order: this tags each task with its original index
/// before scheduling, then sorts the collected results back into
/// that order afterward, so a caller can keep an "in iteration
/// order" report contract regardless of which task actually finishes
/// first.
///
/// `key_of` extracts the identity that a caller's downstream
/// success/failure handling attributes an outcome to, so it never
/// has to re-derive that identity from an already-consumed item.
/// Every item is attempted to completion regardless of an earlier
/// item's outcome; nothing here short-circuits on a failure, and `R`
/// carries no assumption about being a `Result`: a caller that wants
/// per-item success/failure tracking uses `R = Result<T, E>` itself.
pub async fn run_bounded<T, K, R, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    key_of: impl Fn(&T) -> K,
    op: F,
) -> Vec<(K, R)>
where
    K: Copy,
    F: Fn(T) -> Fut,
    Fut: Future<Output = R>,
{
    let mut tagged: Vec<(usize, K, R)> = stream::iter(items.into_iter().enumerate())
        .map(|(index, item)| {
            let key = key_of(&item);
            let outcome = op(item);
            async move { (index, key, outcome.await) }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    tagged.sort_by_key(|(index, ..)| *index);
    tagged
        .into_iter()
        .map(|(_, key, outcome)| (key, outcome))
        .collect()
}
