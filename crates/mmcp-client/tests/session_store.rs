//! Integration tests for `SessionStore`.
//!
//! Cover the flat per-session TOML store owned by the client.

use std::io::Write;

use mmcp_store::SessionStore;
use tempfile::TempDir;
use uuid::Uuid;

fn fresh_store() -> (SessionStore, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let store = SessionStore::open(tmp.path()).expect("open store");
    (store, tmp)
}

#[test]
fn load_returns_none_for_unknown_session() {
    let (store, _tmp) = fresh_store();
    let result = store.load("never-existed").expect("load ok");
    assert!(result.is_none());
}

#[test]
fn upsert_then_load_round_trips() {
    let (store, _tmp) = fresh_store();
    let user = Uuid::now_v7();
    store
        .upsert_session(
            "sess-1",
            Some(user),
            None,
            Some("C:/tmp/transcript.jsonl".into()),
        )
        .expect("upsert");
    let loaded = store.load("sess-1").expect("load").expect("present");
    assert_eq!(loaded.session_id, "sess-1");
    assert_eq!(loaded.user_id, Some(user));
    assert_eq!(loaded.turn_counter, 0);
    assert_eq!(
        loaded.transcript_path.as_deref(),
        Some("C:/tmp/transcript.jsonl")
    );
    assert!(!loaded.post_compaction);
}

#[test]
fn bump_turn_is_monotonic() {
    let (store, _tmp) = fresh_store();
    store
        .upsert_session("sess-1", None, None, None)
        .expect("upsert");

    let first = store.bump_turn("sess-1").expect("bump 1");
    let second = store.bump_turn("sess-1").expect("bump 2");
    let third = store.bump_turn("sess-1").expect("bump 3");
    assert_eq!((first, second, third), (1, 2, 3));
}

#[test]
fn bump_turn_without_upsert_starts_at_one() {
    let (store, _tmp) = fresh_store();
    let first = store.bump_turn("auto").expect("bump auto");
    assert_eq!(first, 1);
    let state = store.load("auto").expect("load").expect("present");
    assert_eq!(state.turn_counter, 1);
}

#[test]
fn transcript_shrink_is_detected_as_compaction() {
    let (store, _tmp) = fresh_store();
    let transcript = tempfile::NamedTempFile::new().expect("tmp transcript");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(transcript.path())
            .unwrap();
        f.write_all(&vec![b'a'; 2000]).unwrap();
    }

    store
        .upsert_session(
            "sess-1",
            None,
            None,
            Some(transcript.path().to_string_lossy().into_owned()),
        )
        .expect("upsert");

    // First check stores the signature but does not compact.
    let first = store.check_transcript("sess-1").expect("first check");
    assert!(!first);
    let after_first = store.load("sess-1").unwrap().unwrap();
    assert!(!after_first.post_compaction);
    assert!(after_first.transcript_signature.is_some());

    // Rewrite the transcript with a smaller body.
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(transcript.path())
            .unwrap();
        f.write_all(b"tiny").unwrap();
    }
    let second = store.check_transcript("sess-1").expect("second check");
    assert!(second, "shrink should be detected as compaction");
    let after_second = store.load("sess-1").unwrap().unwrap();
    assert!(after_second.post_compaction);
}

#[test]
fn non_compaction_check_does_not_clear_the_flag() {
    // Regression test for the flag-flap bug fixed as part of the
    // state refactor: once post-compaction is set, a subsequent
    // check without a shrink must leave it set.
    let (store, _tmp) = fresh_store();
    let transcript = tempfile::NamedTempFile::new().expect("tmp transcript");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(transcript.path())
            .unwrap();
        f.write_all(&vec![b'a'; 1000]).unwrap();
    }

    store
        .upsert_session(
            "sess-1",
            None,
            None,
            Some(transcript.path().to_string_lossy().into_owned()),
        )
        .expect("upsert");
    store.check_transcript("sess-1").expect("initial");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(transcript.path())
            .unwrap();
        f.write_all(b"x").unwrap();
    }
    store.check_transcript("sess-1").expect("shrink");
    assert!(store.load("sess-1").unwrap().unwrap().post_compaction);

    // Grow the transcript back to a larger size. The next check
    // must keep `post_compaction = true` because the caller has
    // not explicitly acknowledged the compaction yet.
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(transcript.path())
            .unwrap();
        f.write_all(&vec![b'b'; 3000]).unwrap();
    }
    let third = store.check_transcript("sess-1").expect("grow");
    assert!(!third);
    assert!(store.load("sess-1").unwrap().unwrap().post_compaction);
}

#[test]
fn multiple_sessions_do_not_interfere() {
    let (store, _tmp) = fresh_store();
    store.upsert_session("a", None, None, None).unwrap();
    store.upsert_session("b", None, None, None).unwrap();

    let a1 = store.bump_turn("a").unwrap();
    let b1 = store.bump_turn("b").unwrap();
    let a2 = store.bump_turn("a").unwrap();
    assert_eq!(a1, 1);
    assert_eq!(b1, 1);
    assert_eq!(a2, 2);
    assert_eq!(store.load("b").unwrap().unwrap().turn_counter, 1);
}
