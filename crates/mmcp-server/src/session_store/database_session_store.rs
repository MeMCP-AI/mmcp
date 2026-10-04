//! `tower_sessions` session store over the `http_sessions` table.
//!
//! Rows are keyed by the SHA-256 digest of the session id.
//! A read of the table or of a backup therefore yields no usable cookie.
//! `load` rebuilds the record id from its argument, because the row holds no id.
//! The following `save` then writes under the same key.

use std::collections::HashMap;

use async_trait::async_trait;
use mmcp_db::entities::http_session::Model;
use mmcp_db::repository::http_session_repo::{self, HttpSessionInsertOutcome};
use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tower_sessions::SessionStore;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion};

use crate::defaults::{
    EXPIRED_SESSION_SWEEP_BATCH_SIZE, NANOSECONDS_PER_MILLISECOND,
    SESSION_ID_COLLISION_MAX_ATTEMPTS, SESSION_VALUE_TAKE_MAX_ATTEMPTS,
};
use crate::session_store::error::DatabaseSessionStoreError;

/// Session store persisting records in the server's own database, on SQLite and Postgres alike.
#[derive(Debug, Clone)]
pub struct DatabaseSessionStore {
    conn: DatabaseConnection,
}

/// Lowercase hex SHA-256 of the 16 bytes the session cookie encodes.
fn session_id_key(id: &Id) -> String {
    hex::encode(Sha256::digest(id.0.to_le_bytes()))
}

fn to_unix_millis(instant: OffsetDateTime) -> Result<i64, DatabaseSessionStoreError> {
    i64::try_from(instant.unix_timestamp_nanos() / NANOSECONDS_PER_MILLISECOND).map_err(|_| {
        DatabaseSessionStoreError::ExpiryOutOfRange {
            expiry_date: instant,
        }
    })
}

fn from_unix_millis(millis: i64) -> Result<OffsetDateTime, DatabaseSessionStoreError> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * NANOSECONDS_PER_MILLISECOND)
        .map_err(|source| DatabaseSessionStoreError::StoredExpiryOutOfRange {
            expires_at: millis,
            source,
        })
}

fn row_for(record: &Record) -> Result<Model, DatabaseSessionStoreError> {
    Ok(Model {
        session_id_sha256: session_id_key(&record.id),
        data: serde_json::to_string(&record.data).map_err(DatabaseSessionStoreError::Encode)?,
        expires_at: to_unix_millis(record.expiry_date)?,
    })
}

/// Logs a store failure with its source chain and maps it to the trait's string-carrying error.
fn surface(error: DatabaseSessionStoreError) -> session_store::Error {
    tracing::error!(error = ?error, "session store operation failed");
    error.into()
}

impl DatabaseSessionStore {
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    /// Delete every expired record, at most `batch_size` rows per statement, and return the total deleted.
    ///
    /// Batches run until one comes back short, so a sweep never holds the connection for an unbounded time.
    pub async fn delete_expired_records(
        &self,
        batch_size: u64,
    ) -> Result<u64, DatabaseSessionStoreError> {
        let now = jiff::Timestamp::now().as_millisecond();
        let mut total_deleted = 0;
        loop {
            let deleted =
                http_session_repo::delete_expired_batch(&self.conn, now, batch_size).await?;
            total_deleted += deleted;
            if deleted == 0 || deleted < batch_size {
                return Ok(total_deleted);
            }
        }
    }

    /// Remove `key` from the stored data of session `id` and return its value.
    ///
    /// The removal is atomic across requests and server instances: the stored data is rewritten only while
    /// it still holds what was read, and a lost race re-reads and retries.
    /// So of any number of concurrent takers of one value exactly one receives it.
    /// Nothing is written when the session is unknown or expired, or holds no value under `key`.
    pub async fn take_value(
        &self,
        id: &Id,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseSessionStoreError> {
        let row_key = session_id_key(id);
        for _ in 0..SESSION_VALUE_TAKE_MAX_ATTEMPTS {
            let now = jiff::Timestamp::now().as_millisecond();
            let Some(row) = http_session_repo::find_unexpired(&self.conn, &row_key, now).await?
            else {
                return Ok(None);
            };
            let mut data: HashMap<String, serde_json::Value> =
                serde_json::from_str(&row.data).map_err(DatabaseSessionStoreError::Decode)?;
            let Some(value) = data.remove(key) else {
                return Ok(None);
            };
            let remaining =
                serde_json::to_string(&data).map_err(DatabaseSessionStoreError::Encode)?;
            if http_session_repo::replace_data_if_unchanged(
                &self.conn, &row_key, &row.data, remaining,
            )
            .await?
            {
                return Ok(Some(value));
            }
        }
        Err(DatabaseSessionStoreError::ValueTakeContended {
            attempts: SESSION_VALUE_TAKE_MAX_ATTEMPTS,
        })
    }

    async fn create_record(&self, record: &mut Record) -> Result<(), DatabaseSessionStoreError> {
        for _ in 0..SESSION_ID_COLLISION_MAX_ATTEMPTS {
            match http_session_repo::insert_if_absent(&self.conn, row_for(record)?).await? {
                HttpSessionInsertOutcome::Inserted => return Ok(()),
                HttpSessionInsertOutcome::IdTaken => record.id = Id::default(),
            }
        }
        Err(DatabaseSessionStoreError::IdCollisionRetriesExhausted {
            attempts: SESSION_ID_COLLISION_MAX_ATTEMPTS,
        })
    }

    async fn save_record(&self, record: &Record) -> Result<(), DatabaseSessionStoreError> {
        http_session_repo::upsert(&self.conn, row_for(record)?).await?;
        Ok(())
    }

    async fn load_record(&self, id: &Id) -> Result<Option<Record>, DatabaseSessionStoreError> {
        let now = jiff::Timestamp::now().as_millisecond();
        let Some(row) =
            http_session_repo::find_unexpired(&self.conn, &session_id_key(id), now).await?
        else {
            return Ok(None);
        };
        Ok(Some(Record {
            id: *id,
            data: serde_json::from_str(&row.data).map_err(DatabaseSessionStoreError::Decode)?,
            expiry_date: from_unix_millis(row.expires_at)?,
        }))
    }

    async fn delete_record(&self, id: &Id) -> Result<(), DatabaseSessionStoreError> {
        http_session_repo::delete(&self.conn, &session_id_key(id)).await?;
        Ok(())
    }
}

#[async_trait]
impl SessionStore for DatabaseSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        self.create_record(record).await.map_err(surface)
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.save_record(record).await.map_err(surface)
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        self.load_record(session_id).await.map_err(surface)
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.delete_record(session_id).await.map_err(surface)
    }
}

#[async_trait]
impl ExpiredDeletion for DatabaseSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        self.delete_expired_records(EXPIRED_SESSION_SWEEP_BATCH_SIZE)
            .await
            .map(|_| ())
            .map_err(surface)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::collections::HashMap;

    use mmcp_db::entities::http_session::Entity;
    use sea_orm::EntityTrait;
    use time::Duration;

    use super::*;
    use crate::session_store::test_support::test_store;

    fn record_expiring_in(lifetime: Duration) -> Record {
        let mut data = HashMap::new();
        data.insert("user".to_owned(), serde_json::json!("alice"));
        Record {
            id: Id::default(),
            data,
            expiry_date: OffsetDateTime::now_utc() + lifetime,
        }
    }

    async fn stored_rows(store: &DatabaseSessionStore) -> Vec<Model> {
        Entity::find().all(&store.conn).await.expect("read rows")
    }

    #[tokio::test]
    async fn database_session_store_round_trips_a_record() {
        let store = test_store().await;
        let mut record = record_expiring_in(Duration::minutes(15));

        store.create(&mut record).await.unwrap();
        let loaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, record.id);
        assert_eq!(loaded.data, record.data);
        assert_eq!(
            to_unix_millis(loaded.expiry_date).unwrap(),
            to_unix_millis(record.expiry_date).unwrap(),
            "the expiry survives at millisecond precision"
        );

        record
            .data
            .insert("extra".to_owned(), serde_json::json!(42));
        store.save(&record).await.unwrap();
        let reloaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(reloaded.data, record.data);

        store.delete(&record.id).await.unwrap();
        assert!(store.load(&record.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn database_session_store_create_retries_on_id_collision() {
        let store = test_store().await;
        let mut first = record_expiring_in(Duration::minutes(15));
        store.create(&mut first).await.unwrap();
        let first_id = first.id;

        let mut colliding = record_expiring_in(Duration::minutes(15));
        colliding.id = first_id;
        colliding
            .data
            .insert("owner".to_owned(), serde_json::json!("second"));
        store.create(&mut colliding).await.unwrap();

        assert_ne!(
            colliding.id, first_id,
            "a taken id is replaced by a fresh one"
        );
        let original = store.load(&first_id).await.unwrap().unwrap();
        assert_eq!(
            original.data, first.data,
            "the colliding create must not overwrite the existing row"
        );
        let created = store.load(&colliding.id).await.unwrap().unwrap();
        assert_eq!(created.data, colliding.data);
        assert_eq!(stored_rows(&store).await.len(), 2);
    }

    #[tokio::test]
    async fn database_session_store_keys_rows_by_the_sha256_of_the_id() {
        let store = test_store().await;
        let mut record = record_expiring_in(Duration::minutes(15));
        store.create(&mut record).await.unwrap();

        let rows = stored_rows(&store).await;
        assert_eq!(rows.len(), 1);
        let key = &rows[0].session_id_sha256;
        assert_ne!(
            *key,
            record.id.to_string(),
            "the cookie value must never be a row key"
        );
        assert_eq!(*key, hex::encode(Sha256::digest(record.id.0.to_le_bytes())));
    }

    #[tokio::test]
    async fn database_session_store_load_returns_the_callers_id() {
        let store = test_store().await;
        let mut record = record_expiring_in(Duration::minutes(15));
        store.create(&mut record).await.unwrap();

        let mut loaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, record.id);
        loaded
            .data
            .insert("touched".to_owned(), serde_json::json!(true));
        store.save(&loaded).await.unwrap();

        assert_eq!(
            stored_rows(&store).await.len(),
            1,
            "a save after a load updates the same row"
        );
        let reloaded = store.load(&record.id).await.unwrap().unwrap();
        assert_eq!(reloaded.data.get("touched"), Some(&serde_json::json!(true)));
    }

    #[tokio::test]
    async fn database_session_store_never_loads_an_expired_record() {
        let store = test_store().await;
        let mut record = record_expiring_in(Duration::minutes(-1));
        store.create(&mut record).await.unwrap();

        assert!(
            store.load(&record.id).await.unwrap().is_none(),
            "an expired record is not served even before the sweep deletes it"
        );
        assert_eq!(stored_rows(&store).await.len(), 1);
    }

    fn record_holding(key: &str, value: serde_json::Value) -> Record {
        let mut record = record_expiring_in(Duration::minutes(15));
        record.data.insert(key.to_owned(), value);
        record
    }

    #[tokio::test]
    async fn take_value_returns_the_value_and_removes_only_that_key() {
        let store = test_store().await;
        let mut record = record_holding("ceremony", serde_json::json!({ "state": 1 }));
        store.create(&mut record).await.unwrap();

        let taken = store.take_value(&record.id, "ceremony").await.unwrap();

        assert_eq!(taken, Some(serde_json::json!({ "state": 1 })));
        let reloaded = store.load(&record.id).await.unwrap().unwrap();
        assert!(!reloaded.data.contains_key("ceremony"));
        assert_eq!(
            reloaded.data.get("user"),
            Some(&serde_json::json!("alice")),
            "the other keys of the record stay"
        );
        assert_eq!(
            store.take_value(&record.id, "ceremony").await.unwrap(),
            None,
            "a value is taken once"
        );
    }

    #[tokio::test]
    async fn take_value_of_an_absent_key_or_session_writes_nothing() {
        let store = test_store().await;
        let mut record = record_expiring_in(Duration::minutes(15));
        store.create(&mut record).await.unwrap();
        let before = stored_rows(&store).await;

        assert_eq!(store.take_value(&record.id, "absent").await.unwrap(), None);
        assert_eq!(
            store.take_value(&Id::default(), "user").await.unwrap(),
            None,
            "an unknown session holds nothing"
        );

        assert_eq!(stored_rows(&store).await, before);
    }

    #[tokio::test]
    async fn take_value_of_an_expired_session_returns_none() {
        let store = test_store().await;
        let mut record = record_holding("ceremony", serde_json::json!(1));
        record.expiry_date = OffsetDateTime::now_utc() - Duration::minutes(1);
        store.create(&mut record).await.unwrap();

        assert_eq!(
            store.take_value(&record.id, "ceremony").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn take_value_reaches_exactly_one_of_many_concurrent_takers() {
        const TAKERS: usize = 16;
        let store = test_store().await;
        let mut record = record_holding("ceremony", serde_json::json!("state"));
        store.create(&mut record).await.unwrap();

        let takers: Vec<_> = (0..TAKERS)
            .map(|_| {
                let store = store.clone();
                let id = record.id;
                tokio::spawn(async move { store.take_value(&id, "ceremony").await.unwrap() })
            })
            .collect();
        let mut winners = 0;
        for taker in takers {
            if taker.await.unwrap().is_some() {
                winners += 1;
            }
        }

        assert_eq!(
            winners, 1,
            "a value must reach one taker, however many race"
        );
    }

    #[tokio::test]
    async fn database_session_store_reports_a_stored_expiry_it_cannot_represent() {
        let store = test_store().await;
        let id = Id::default();
        http_session_repo::upsert(
            &store.conn,
            Model {
                session_id_sha256: session_id_key(&id),
                data: "{}".to_owned(),
                expires_at: i64::MAX,
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            store.load(&id).await,
            Err(session_store::Error::Backend(_))
        ));
    }

    #[tokio::test]
    async fn database_session_store_reports_stored_data_it_cannot_decode() {
        let store = test_store().await;
        let id = Id::default();
        http_session_repo::upsert(
            &store.conn,
            Model {
                session_id_sha256: session_id_key(&id),
                data: "not json".to_owned(),
                expires_at: jiff::Timestamp::now().as_millisecond() + 60_000,
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            store.load(&id).await,
            Err(session_store::Error::Decode(_))
        ));
    }

    #[tokio::test]
    async fn delete_expired_records_deletes_every_expired_row_across_batches() {
        const BATCH_SIZE: u64 = 2;
        const EXPIRED_RECORDS: u64 = 5;
        let store = test_store().await;
        for _ in 0..EXPIRED_RECORDS {
            store
                .create(&mut record_expiring_in(Duration::minutes(-1)))
                .await
                .unwrap();
        }
        let mut live = record_expiring_in(Duration::minutes(15));
        store.create(&mut live).await.unwrap();

        let deleted = store.delete_expired_records(BATCH_SIZE).await.unwrap();

        assert_eq!(deleted, EXPIRED_RECORDS);
        assert!(store.load(&live.id).await.unwrap().is_some());
        assert_eq!(stored_rows(&store).await.len(), 1);
    }

    #[tokio::test]
    async fn delete_expired_records_with_a_zero_batch_size_terminates() {
        let store = test_store().await;
        store
            .create(&mut record_expiring_in(Duration::minutes(-1)))
            .await
            .unwrap();

        assert_eq!(store.delete_expired_records(0).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn expired_deletion_trait_removes_expired_records() {
        let store = test_store().await;
        store
            .create(&mut record_expiring_in(Duration::minutes(-1)))
            .await
            .unwrap();

        store.delete_expired().await.unwrap();

        assert!(stored_rows(&store).await.is_empty());
    }
}
