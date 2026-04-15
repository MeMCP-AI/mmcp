//! Integration tests for the session tracker against an in-memory
//! SQLite database.

use std::io::Write;

use mmcp_db::entities::group::OwnerKind;
use mmcp_db::entities::memory::MemoryKind;
use mmcp_db::repository::{group_repo, memory_repo, user_repo};
use mmcp_db::{Database, connect};
use mmcp_session::{SessionTracker, StartSession};
use uuid::Uuid;

async fn seed_memory(db: &Database) -> Uuid {
    let user_id = Uuid::now_v7();
    user_repo::create(
        db.connection(),
        user_repo::NewUser {
            id: user_id,
            handle: "alice".into(),
            display_name: None,
            password_hash: None,
            email: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    let group_id = Uuid::now_v7();
    group_repo::create(
        db.connection(),
        group_repo::NewGroup {
            id: group_id,
            slug: "g".into(),
            owner_kind: OwnerKind::User,
            owner_id: user_id,
            display_name: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    let memory_id = Uuid::now_v7();
    memory_repo::create(
        db.connection(),
        memory_repo::NewMemory {
            id: memory_id,
            group_id,
            slug: "rules".into(),
            kind: MemoryKind::Rule,
            mandatory: true,
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();
    memory_id
}

async fn fresh_tracker() -> (SessionTracker, Database) {
    let db = connect("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    let tracker = SessionTracker::new(db.connection().clone());
    (tracker, db)
}

#[tokio::test]
async fn start_and_find_session() {
    let (tracker, _db) = fresh_tracker().await;
    let start = StartSession {
        session_id: "sess-1".into(),
        user_id: None,
        project_uuid: None,
        transcript_path: None,
    };
    tracker.start(start.clone()).await.unwrap();
    let found = tracker.find("sess-1").await.unwrap().expect("present");
    assert_eq!(found.session_id, "sess-1");
    assert_eq!(found.turn_counter, 0);
}

#[tokio::test]
async fn turn_counter_bumps_monotonically() {
    let (tracker, _db) = fresh_tracker().await;
    tracker
        .start(StartSession {
            session_id: "sess-1".into(),
            user_id: None,
            project_uuid: None,
            transcript_path: None,
        })
        .await
        .unwrap();

    let first = tracker.bump_turn("sess-1").await.unwrap();
    let second = tracker.bump_turn("sess-1").await.unwrap();
    let third = tracker.bump_turn("sess-1").await.unwrap();
    assert_eq!((first, second, third), (1, 2, 3));
}

#[tokio::test]
async fn read_tracking_marks_memory_as_seen() {
    let (tracker, db) = fresh_tracker().await;
    let memory = seed_memory(&db).await;
    tracker
        .start(StartSession {
            session_id: "sess-1".into(),
            user_id: None,
            project_uuid: None,
            transcript_path: None,
        })
        .await
        .unwrap();

    assert!(!tracker.has_read("sess-1", memory).await.unwrap());

    tracker
        .record_read("sess-1", memory, 1, Some("0.1.0".into()), false)
        .await
        .unwrap();
    assert!(tracker.has_read("sess-1", memory).await.unwrap());
}

#[tokio::test]
async fn transcript_shrink_is_detected_as_compaction() {
    let (tracker, _db) = fresh_tracker().await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(tmp.path()).unwrap();
        f.write_all(&vec![b'a'; 2000]).unwrap();
    }

    tracker
        .start(StartSession {
            session_id: "sess-1".into(),
            user_id: None,
            project_uuid: None,
            transcript_path: Some(path.clone()),
        })
        .await
        .unwrap();

    // First check stores the signature but does not report a
    // compaction because there is nothing to compare against.
    let first = tracker.check_transcript("sess-1").await.unwrap();
    assert!(!first);
    assert!(!tracker.is_post_compaction("sess-1").await.unwrap());

    // Rewrite the file with a smaller body.
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(tmp.path())
            .unwrap();
        f.write_all(b"tiny").unwrap();
    }
    let second = tracker.check_transcript("sess-1").await.unwrap();
    assert!(second, "compaction should be detected after shrink");
    assert!(tracker.is_post_compaction("sess-1").await.unwrap());
}

#[tokio::test]
async fn acknowledge_clears_post_compaction_flag() {
    let (tracker, _db) = fresh_tracker().await;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(tmp.path()).unwrap();
        f.write_all(&vec![b'a'; 1000]).unwrap();
    }
    tracker
        .start(StartSession {
            session_id: "sess-2".into(),
            user_id: None,
            project_uuid: None,
            transcript_path: Some(path),
        })
        .await
        .unwrap();

    tracker.check_transcript("sess-2").await.unwrap();
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(tmp.path())
            .unwrap();
        f.write_all(b"x").unwrap();
    }
    tracker.check_transcript("sess-2").await.unwrap();
    assert!(tracker.is_post_compaction("sess-2").await.unwrap());

    tracker.acknowledge_compaction("sess-2").await.unwrap();
    assert!(!tracker.is_post_compaction("sess-2").await.unwrap());
}
