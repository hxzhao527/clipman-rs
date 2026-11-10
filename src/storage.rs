use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, Sqlite, SqlitePool};
use tokio::sync::{mpsc, oneshot};

use crate::clipboard::ClipboardSource;
use crate::domain::{CapturedPayload, Representation};

pub const DEFAULT_PRIMARY_ENABLED: bool = true;
pub const DEFAULT_MAX_HISTORY_ENTRIES: usize = 20;
pub const DEFAULT_MAX_MENU_ENTRIES: usize = 10;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    pub primary_enabled: bool,
    pub max_history_entries: usize,
    pub max_menu_entries: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            primary_enabled: DEFAULT_PRIMARY_ENABLED,
            max_history_entries: DEFAULT_MAX_HISTORY_ENTRIES,
            max_menu_entries: DEFAULT_MAX_MENU_ENTRIES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEntry {
    pub id: u64,
    pub payload: CapturedPayload,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistorySnapshot {
    pub regular: Vec<StoredEntry>,
    pub primary: Option<StoredEntry>,
    pub settings: Settings,
}

#[derive(Clone)]
pub struct StorageService {
    command_tx: mpsc::UnboundedSender<StorageCommand>,
}

enum StorageCommand {
    Upsert {
        source: ClipboardSource,
        payload: CapturedPayload,
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
    Touch {
        id: u64,
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
    Delete {
        id: u64,
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
    ClearRegular {
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
    UpdateSettings {
        settings: Settings,
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
    Snapshot {
        reply: oneshot::Sender<Result<HistorySnapshot>>,
    },
}

impl StorageService {
    pub async fn open(path: impl AsRef<Path>) -> Result<(Self, HistorySnapshot)> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create database directory {}", parent.display())
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| format!("failed to open SQLite database {}", path.display()))?;
        MIGRATOR
            .run(&pool)
            .await
            .context("failed to run database migrations")?;

        let initial_snapshot = load_snapshot(&pool).await?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        tokio::spawn(storage_actor(pool, command_rx));

        Ok((Self { command_tx }, initial_snapshot))
    }

    pub async fn upsert_capture(
        &self,
        source: ClipboardSource,
        payload: CapturedPayload,
    ) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::Upsert {
            source,
            payload,
            reply,
        })
        .await
    }

    pub async fn touch(&self, id: u64) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::Touch { id, reply })
            .await
    }

    pub async fn delete(&self, id: u64) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::Delete { id, reply })
            .await
    }

    pub async fn clear_regular(&self) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::ClearRegular { reply })
            .await
    }

    pub async fn update_settings(&self, settings: Settings) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::UpdateSettings { settings, reply })
            .await
    }

    pub async fn snapshot(&self) -> Result<HistorySnapshot> {
        self.request(|reply| StorageCommand::Snapshot { reply })
            .await
    }

    async fn request(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<HistorySnapshot>>) -> StorageCommand,
    ) -> Result<HistorySnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(command(reply_tx))
            .map_err(|_| anyhow!("storage actor is not running"))?;
        reply_rx
            .await
            .map_err(|_| anyhow!("storage actor dropped the response"))?
    }
}

pub fn default_database_path() -> Result<PathBuf> {
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .context("neither XDG_DATA_HOME nor HOME is set")?;
    Ok(base.join("clipman-rs").join("clipman.sqlite3"))
}

async fn storage_actor(pool: SqlitePool, mut command_rx: mpsc::UnboundedReceiver<StorageCommand>) {
    while let Some(command) = command_rx.recv().await {
        match command {
            StorageCommand::Upsert {
                source,
                payload,
                reply,
            } => {
                let result =
                    with_snapshot(&pool, process_upsert(&pool, source, payload).await).await;
                let _ = reply.send(result);
            }
            StorageCommand::Touch { id, reply } => {
                let result = with_snapshot(&pool, touch_entry(&pool, id).await).await;
                let _ = reply.send(result);
            }
            StorageCommand::Delete { id, reply } => {
                let result = with_snapshot(&pool, delete_entry(&pool, id).await).await;
                let _ = reply.send(result);
            }
            StorageCommand::ClearRegular { reply } => {
                let result = with_snapshot(&pool, clear_regular_entries(&pool).await).await;
                let _ = reply.send(result);
            }
            StorageCommand::UpdateSettings { settings, reply } => {
                let result = with_snapshot(&pool, update_settings(&pool, settings).await).await;
                let _ = reply.send(result);
            }
            StorageCommand::Snapshot { reply } => {
                let _ = reply.send(load_snapshot(&pool).await);
            }
        }
    }
}

async fn with_snapshot(pool: &SqlitePool, result: Result<()>) -> Result<HistorySnapshot> {
    result?;
    load_snapshot(pool).await
}

async fn process_upsert(
    pool: &SqlitePool,
    source: ClipboardSource,
    payload: CapturedPayload,
) -> Result<()> {
    match source {
        ClipboardSource::Regular => upsert_regular(pool, payload).await,
        ClipboardSource::Primary => upsert_primary(pool, payload).await,
    }
}

async fn upsert_regular(pool: &SqlitePool, payload: CapturedPayload) -> Result<()> {
    let settings = load_settings(pool).await?;
    let now = now_ms();
    let mut tx = pool.begin().await?;
    let next_seq = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(last_used_seq), 0) + 1 FROM history_entries",
    )
    .fetch_one(&mut *tx)
    .await?;
    let existing_id =
        sqlx::query_scalar::<_, i64>("SELECT id FROM history_entries WHERE fingerprint = ?")
            .bind(payload.fingerprint.as_slice())
            .fetch_optional(&mut *tx)
            .await?;

    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE history_entries
             SET plain_text = ?, last_used_at_ms = ?, last_used_seq = ?
             WHERE id = ?",
        )
        .bind(&payload.plain_text)
        .bind(now)
        .bind(next_seq)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    } else {
        let result = sqlx::query(
            "INSERT INTO history_entries (
                fingerprint, plain_text, captured_at_ms, last_used_at_ms, last_used_seq
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(payload.fingerprint.as_slice())
        .bind(&payload.plain_text)
        .bind(now)
        .bind(now)
        .bind(next_seq)
        .execute(&mut *tx)
        .await?;
        let entry_id = result.last_insert_rowid();
        insert_history_representations(&mut tx, entry_id, &payload.representations).await?;
    }

    sqlx::query(
        "DELETE FROM history_entries
         WHERE id IN (
             SELECT id FROM history_entries
             ORDER BY last_used_seq DESC
             LIMIT -1 OFFSET ?
         )",
    )
    .bind(settings.max_history_entries as i64)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

async fn insert_history_representations(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    entry_id: i64,
    representations: &[Representation],
) -> Result<()> {
    for representation in representations {
        sqlx::query(
            "INSERT INTO history_representations (entry_id, mime_type, data)
             VALUES (?, ?, ?)",
        )
        .bind(entry_id)
        .bind(&representation.mime_type)
        .bind(&representation.bytes)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn upsert_primary(pool: &SqlitePool, payload: CapturedPayload) -> Result<()> {
    let now = now_ms();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM primary_representations")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM primary_entry")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO primary_entry (
            singleton_id, fingerprint, plain_text, captured_at_ms
         ) VALUES (1, ?, ?, ?)",
    )
    .bind(payload.fingerprint.as_slice())
    .bind(&payload.plain_text)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    for representation in &payload.representations {
        sqlx::query(
            "INSERT INTO primary_representations (mime_type, data)
             VALUES (?, ?)",
        )
        .bind(&representation.mime_type)
        .bind(&representation.bytes)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn touch_entry(pool: &SqlitePool, id: u64) -> Result<()> {
    let next_seq = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(last_used_seq), 0) + 1 FROM history_entries",
    )
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "UPDATE history_entries
         SET last_used_at_ms = ?, last_used_seq = ?
         WHERE id = ?",
    )
    .bind(now_ms())
    .bind(next_seq)
    .bind(id as i64)
    .execute(pool)
    .await?;
    Ok(())
}

async fn delete_entry(pool: &SqlitePool, id: u64) -> Result<()> {
    sqlx::query("DELETE FROM history_entries WHERE id = ?")
        .bind(id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

async fn clear_regular_entries(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM history_entries")
        .execute(pool)
        .await?;
    Ok(())
}

async fn update_settings(pool: &SqlitePool, settings: Settings) -> Result<()> {
    if settings.max_history_entries == 0
        || settings.max_menu_entries == 0
        || settings.max_menu_entries > settings.max_history_entries
    {
        return Err(anyhow!("invalid history or menu entry limit"));
    }

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE settings
         SET primary_enabled = ?, max_history_entries = ?, max_menu_entries = ?
         WHERE singleton_id = 1",
    )
    .bind(i64::from(settings.primary_enabled))
    .bind(settings.max_history_entries as i64)
    .bind(settings.max_menu_entries as i64)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM history_entries
         WHERE id IN (
             SELECT id FROM history_entries
             ORDER BY last_used_seq DESC
             LIMIT -1 OFFSET ?
         )",
    )
    .bind(settings.max_history_entries as i64)
    .execute(&mut *tx)
    .await?;
    if !settings.primary_enabled {
        sqlx::query("DELETE FROM primary_representations")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM primary_entry")
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load_snapshot(pool: &SqlitePool) -> Result<HistorySnapshot> {
    let settings = load_settings(pool).await?;
    let rows = sqlx::query(
        "SELECT id, fingerprint, plain_text
         FROM history_entries
         ORDER BY last_used_seq DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut regular = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.try_get::<i64, _>("id")? as u64;
        let fingerprint = fingerprint_from_blob(row.try_get("fingerprint")?)?;
        let plain_text = row.try_get("plain_text")?;
        let representations = load_history_representations(pool, id as i64).await?;
        regular.push(StoredEntry {
            id,
            payload: CapturedPayload {
                fingerprint,
                plain_text,
                representations,
            },
        });
    }

    let primary = sqlx::query(
        "SELECT fingerprint, plain_text
         FROM primary_entry
         WHERE singleton_id = 1",
    )
    .fetch_optional(pool)
    .await?
    .map(|row| {
        Ok::<_, anyhow::Error>(StoredEntry {
            id: 0,
            payload: CapturedPayload {
                fingerprint: fingerprint_from_blob(row.try_get("fingerprint")?)?,
                plain_text: row.try_get("plain_text")?,
                representations: Vec::new(),
            },
        })
    })
    .transpose()?;

    let primary = if let Some(mut primary) = primary {
        primary.payload.representations = load_primary_representations(pool).await?;
        Some(primary)
    } else {
        None
    };

    Ok(HistorySnapshot {
        regular,
        primary,
        settings,
    })
}

async fn load_settings(pool: &SqlitePool) -> Result<Settings> {
    let row = sqlx::query(
        "SELECT primary_enabled, max_history_entries, max_menu_entries
         FROM settings WHERE singleton_id = 1",
    )
    .fetch_one(pool)
    .await?;
    let max_history_entries = row.try_get::<i64, _>("max_history_entries")?;
    let max_menu_entries = row.try_get::<i64, _>("max_menu_entries")?;
    if max_history_entries < 1 || max_menu_entries < 1 || max_menu_entries > max_history_entries {
        return Err(anyhow!("database contains invalid entry limits"));
    }
    Ok(Settings {
        primary_enabled: row.try_get::<i64, _>("primary_enabled")? != 0,
        max_history_entries: max_history_entries as usize,
        max_menu_entries: max_menu_entries as usize,
    })
}

async fn load_history_representations(
    pool: &SqlitePool,
    entry_id: i64,
) -> Result<Vec<Representation>> {
    let rows = sqlx::query(
        "SELECT mime_type, data
         FROM history_representations
         WHERE entry_id = ?
         ORDER BY mime_type",
    )
    .bind(entry_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok::<_, anyhow::Error>(Representation {
                mime_type: row.try_get("mime_type")?,
                bytes: row.try_get("data")?,
            })
        })
        .collect::<Result<Vec<_>>>()
}

async fn load_primary_representations(pool: &SqlitePool) -> Result<Vec<Representation>> {
    let rows = sqlx::query(
        "SELECT mime_type, data
         FROM primary_representations
         ORDER BY mime_type",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok::<_, anyhow::Error>(Representation {
                mime_type: row.try_get("mime_type")?,
                bytes: row.try_get("data")?,
            })
        })
        .collect::<Result<Vec<_>>>()
}

fn fingerprint_from_blob(blob: Vec<u8>) -> Result<[u8; 32]> {
    blob.try_into()
        .map_err(|value: Vec<u8>| anyhow!("invalid fingerprint length {}", value.len()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::build_payload;
    use tempfile::TempDir;

    fn payload(text: &str, rich_text: &str) -> CapturedPayload {
        build_payload(vec![
            Representation {
                mime_type: "text/plain;charset=utf-8".to_owned(),
                bytes: text.as_bytes().to_vec(),
            },
            Representation {
                mime_type: "text/html".to_owned(),
                bytes: rich_text.as_bytes().to_vec(),
            },
        ])
        .unwrap()
    }

    async fn open_test_storage() -> (TempDir, PathBuf, StorageService, HistorySnapshot) {
        let directory = TempDir::new().unwrap();
        let database_path = directory.path().join("clipman.sqlite3");
        let (storage, snapshot) = StorageService::open(&database_path).await.unwrap();
        (directory, database_path, storage, snapshot)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_database_has_default_settings() {
        let (_directory, _path, _storage, snapshot) = open_test_storage().await;
        assert!(snapshot.regular.is_empty());
        assert!(snapshot.primary.is_none());
        assert_eq!(snapshot.settings, Settings::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn regular_history_and_order_survive_restart() {
        let (directory, database_path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(ClipboardSource::Regular, payload("first", "<b>first</b>"))
            .await
            .unwrap();
        let second = storage
            .upsert_capture(ClipboardSource::Regular, payload("second", "<b>second</b>"))
            .await
            .unwrap();
        assert_eq!(second.regular[0].payload.plain_text, "second");

        let first_id = second.regular[1].id;
        let reordered = storage.touch(first_id).await.unwrap();
        assert_eq!(reordered.regular[0].payload.plain_text, "first");

        drop(storage);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (reopened, restored) = StorageService::open(&database_path).await.unwrap();
        assert_eq!(restored.regular.len(), 2);
        assert_eq!(restored.regular[0].payload.plain_text, "first");
        assert_eq!(
            restored.regular[0].payload.representations[1].mime_type,
            "text/plain;charset=utf-8"
        );
        assert_eq!(
            restored.regular[0].payload.representations[1].bytes,
            b"first"
        );
        drop(reopened);
        drop(directory);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_payload_moves_existing_entry_to_top() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(ClipboardSource::Regular, payload("same", "<b>same</b>"))
            .await
            .unwrap();
        storage
            .upsert_capture(ClipboardSource::Regular, payload("other", "<b>other</b>"))
            .await
            .unwrap();
        let snapshot = storage
            .upsert_capture(ClipboardSource::Regular, payload("same", "<b>same</b>"))
            .await
            .unwrap();

        assert_eq!(snapshot.regular.len(), 2);
        assert_eq!(snapshot.regular[0].payload.plain_text, "same");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reducing_history_limit_prunes_oldest_entries() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(ClipboardSource::Regular, payload("one", "<b>one</b>"))
            .await
            .unwrap();
        storage
            .upsert_capture(ClipboardSource::Regular, payload("two", "<b>two</b>"))
            .await
            .unwrap();
        storage
            .upsert_capture(ClipboardSource::Regular, payload("three", "<b>three</b>"))
            .await
            .unwrap();

        let snapshot = storage
            .update_settings(Settings {
                max_history_entries: 2,
                max_menu_entries: 2,
                ..Settings::default()
            })
            .await
            .unwrap();
        assert_eq!(snapshot.regular.len(), 2);
        assert_eq!(snapshot.settings.max_history_entries, 2);
        assert_eq!(snapshot.regular[0].payload.plain_text, "three");
        assert_eq!(snapshot.regular[1].payload.plain_text, "two");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_and_clear_operations_are_persisted() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        let primary = storage
            .upsert_capture(
                ClipboardSource::Primary,
                payload("primary", "<b>primary</b>"),
            )
            .await
            .unwrap();
        assert_eq!(
            primary.primary.as_ref().unwrap().payload.plain_text,
            "primary"
        );

        storage
            .upsert_capture(
                ClipboardSource::Regular,
                payload("regular", "<b>regular</b>"),
            )
            .await
            .unwrap();
        let cleared = storage.clear_regular().await.unwrap();
        assert!(cleared.regular.is_empty());
        assert!(cleared.primary.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_settings_are_rejected() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        let result = storage
            .update_settings(Settings {
                max_history_entries: 2,
                max_menu_entries: 3,
                ..Settings::default()
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn settings_survive_restart() {
        let (_directory, database_path, storage, _snapshot) = open_test_storage().await;
        storage
            .update_settings(Settings {
                primary_enabled: false,
                max_history_entries: 7,
                max_menu_entries: 4,
            })
            .await
            .unwrap();

        drop(storage);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (reopened, restored) = StorageService::open(&database_path).await.unwrap();
        assert_eq!(
            restored.settings,
            Settings {
                primary_enabled: false,
                max_history_entries: 7,
                max_menu_entries: 4,
            }
        );
        drop(reopened);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_survives_restart() {
        let (_directory, database_path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(
                ClipboardSource::Primary,
                payload("persistent primary", "<b>persistent primary</b>"),
            )
            .await
            .unwrap();

        drop(storage);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (reopened, restored) = StorageService::open(&database_path).await.unwrap();
        let primary = restored.primary.unwrap();
        assert_eq!(primary.payload.plain_text, "persistent primary");
        assert_eq!(primary.payload.representations.len(), 2);
        drop(reopened);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deleting_one_entry_keeps_other_history() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(ClipboardSource::Regular, payload("first", "<b>first</b>"))
            .await
            .unwrap();
        let second = storage
            .upsert_capture(ClipboardSource::Regular, payload("second", "<b>second</b>"))
            .await
            .unwrap();

        let snapshot = storage.delete(second.regular[0].id).await.unwrap();
        assert_eq!(snapshot.regular.len(), 1);
        assert_eq!(snapshot.regular[0].payload.plain_text, "first");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn disabling_primary_clears_persisted_primary() {
        let (_directory, _path, storage, _snapshot) = open_test_storage().await;
        storage
            .upsert_capture(
                ClipboardSource::Primary,
                payload("primary to clear", "<b>primary to clear</b>"),
            )
            .await
            .unwrap();

        let snapshot = storage
            .update_settings(Settings {
                primary_enabled: false,
                ..Settings::default()
            })
            .await
            .unwrap();
        assert!(!snapshot.settings.primary_enabled);
        assert!(snapshot.primary.is_none());
    }
}
