//! [`mmcp_store::cache::init_from_home`] must stay a no-op on a
//! repeat call naming the SAME home, but reject a repeat call naming
//! a DIFFERENT one instead of silently keeping the first home's pool
//! active under the caller's mistaken belief it switched.
//!
//! Deliberately its own integration-test binary, and deliberately a
//! single test function within it (cargo compiles every file under
//! `tests/` as a separate process, but every `#[tokio::test]` inside
//! ONE such file still shares that process and can run concurrently
//! by default): the process-global `ACTIVE_POOL` slot this test
//! exercises must not race a sibling test's own `init_from_home`
//! call, same rationale as `cache_write_trigger.rs`.

use mmcp_store::cache::{self, CacheError};
use mmcp_store::testing::ScratchHome;

#[tokio::test]
async fn init_from_home_same_home_is_a_no_op_but_a_different_home_errors() {
    let first = ScratchHome::new().await.expect("first scratch home");
    let second = ScratchHome::new().await.expect("second scratch home");

    cache::init_from_home(first.home())
        .await
        .expect("first init_from_home call installs the pool");

    cache::init_from_home(first.home())
        .await
        .expect("a repeat call against the SAME home stays a no-op");

    let err = cache::init_from_home(second.home())
        .await
        .expect_err("a call naming a DIFFERENT home must error");
    match err {
        CacheError::MismatchedHome { active, requested } => {
            assert_eq!(active, cache::default_db_path(first.home()));
            assert_eq!(requested, cache::default_db_path(second.home()));
        }
        other => panic!("expected MismatchedHome, got {other:?}"),
    }

    // The first home's pool must still be the active one: a
    // rejected mismatched call must not have replaced or torn it
    // down.
    let pool = cache::active_pool().expect("first home's pool stays active");
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sqlite_master")
        .fetch_one(&pool)
        .await
        .expect("active pool still answers queries");
    assert!(row.0 >= 0);
}
