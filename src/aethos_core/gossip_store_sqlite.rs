use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::cell::RefCell;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use crate::aethos_core::logging::log_verbose;
use crate::aethos_core::protocol::decode_envelope_payload_b64;

const SQLITE_STORE_FILE_NAME: &str = "gossip-object-store.sqlite3";
const LEGACY_JSON_STORE_FILE_NAME: &str = "gossip-object-store.json";
const BUSY_TIMEOUT_MS: u64 = 5_000;
const CLOCK_SKEW_TOLERANCE_MS: u64 = 30_000;
const MIGRATION_META_KEY: &str = "legacy_json_to_sqlite_migrated_v1";
const SQLITE_MAX_VARIABLES: usize = 999;

#[derive(Debug, Clone)]
pub struct StoredItemRecord {
    pub item_id: String,
    pub envelope_b64: String,
    pub expiry_unix_ms: u64,
    pub hop_count: u16,
    pub recorded_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ImportWriteObject {
    pub item_id: String,
    pub envelope_b64: String,
    pub expiry_unix_ms: u64,
    pub hop_count: u16,
    pub recorded_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordPutOutcome {
    Inserted,
    Refreshed { refreshed_expiry_unix_ms: u64 },
    Dedupe,
}

#[derive(Default)]
struct StoreRuntime {
    current_db_path: Option<PathBuf>,
    conn: Option<Connection>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyGossipStore {
    #[serde(default)]
    items: HashMap<String, LegacyStoredItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyStoredItem {
    item_id: String,
    envelope_b64: String,
    expiry_unix_ms: u64,
    hop_count: u16,
    recorded_at_unix_ms: u64,
}

fn runtime_mutex() -> &'static Mutex<StoreRuntime> {
    static RUNTIME: OnceLock<Mutex<StoreRuntime>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(StoreRuntime::default()))
}

#[cfg(test)]
thread_local! {
    static TEST_STATE_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn with_test_state_dir<T>(state_dir: &Path, f: impl FnOnce() -> T) -> T {
    TEST_STATE_DIR_OVERRIDE.with(|slot| {
        let previous = slot.borrow().clone();
        *slot.borrow_mut() = Some(state_dir.to_path_buf());
        let out = f();
        *slot.borrow_mut() = previous;
        out
    })
}

fn test_state_dir_override() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_STATE_DIR_OVERRIDE.with(|slot| slot.borrow().clone())
    }

    #[cfg(not(test))]
    {
        None
    }
}

fn with_connection<T>(
    op_name: &str,
    f: impl FnOnce(&mut Connection) -> Result<T, String>,
) -> Result<T, String> {
    let db_path = sqlite_store_path();
    let mut runtime = runtime_mutex()
        .lock()
        .map_err(|_| "gossip sqlite runtime mutex poisoned".to_string())?;
    runtime.ensure_connected(&db_path)?;
    let conn = runtime
        .conn
        .as_mut()
        .ok_or_else(|| "gossip sqlite connection unavailable".to_string())?;
    let started = Instant::now();
    let result = f(conn);
    let elapsed_ms = started.elapsed().as_millis();
    if elapsed_ms > 8 {
        log_verbose(&format!(
            "sqlite_timing: op={} elapsed_ms={} db_path={}",
            op_name,
            elapsed_ms,
            db_path.display()
        ));
    }
    result
}

impl StoreRuntime {
    fn ensure_connected(&mut self, db_path: &Path) -> Result<(), String> {
        if self
            .current_db_path
            .as_ref()
            .map(|path| path == db_path)
            .unwrap_or(false)
            && self.conn.is_some()
        {
            return Ok(());
        }

        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed creating gossip sqlite directory {}: {err}",
                    parent.display()
                )
            })?;
        }

        let conn = Connection::open(db_path).map_err(|err| {
            format!(
                "failed opening gossip sqlite db {}: {err}",
                db_path.display()
            )
        })?;
        conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(|err| format!("failed setting sqlite busy timeout: {err}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| format!("failed setting sqlite WAL mode: {err}"))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|err| format!("failed setting sqlite synchronous mode: {err}"))?;
        conn.execute_batch(
            "
                CREATE TABLE IF NOT EXISTS gossip_items (
                    item_id TEXT PRIMARY KEY NOT NULL,
                    envelope_b64 TEXT NOT NULL,
                    expiry_unix_ms INTEGER NOT NULL,
                    hop_count INTEGER NOT NULL,
                    recorded_at_unix_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_gossip_items_expiry
                    ON gossip_items(expiry_unix_ms);
                CREATE INDEX IF NOT EXISTS idx_gossip_items_rank
                    ON gossip_items(hop_count, recorded_at_unix_ms DESC, item_id);
                CREATE TABLE IF NOT EXISTS gossip_meta (
                    meta_key TEXT PRIMARY KEY NOT NULL,
                    meta_value INTEGER NOT NULL
                );
            ",
        )
        .map_err(|err| format!("failed ensuring gossip sqlite schema: {err}"))?;

        self.current_db_path = Some(db_path.to_path_buf());
        self.conn = Some(conn);
        self.run_legacy_migration_if_needed()?;
        Ok(())
    }

    fn run_legacy_migration_if_needed(&mut self) -> Result<(), String> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| "sqlite connection missing during migration".to_string())?;
        if legacy_json_migration_is_done(conn)? {
            return Ok(());
        }

        let legacy_path = legacy_json_store_path();
        let legacy_backup_path = backup_store_path(&legacy_path);
        let migration_source_path = if legacy_path.exists() {
            legacy_path.clone()
        } else if legacy_backup_path.exists() {
            legacy_backup_path.clone()
        } else {
            return Ok(());
        };

        let started = Instant::now();
        let raw = fs::read_to_string(&migration_source_path).map_err(|err| {
            format!(
                "failed reading legacy gossip json {}: {err}",
                migration_source_path.display()
            )
        })?;
        let parsed: LegacyGossipStore = serde_json::from_str(&raw).map_err(|err| {
            format!(
                "failed parsing legacy gossip json {}: {err}",
                migration_source_path.display()
            )
        })?;

        let tx = conn
            .transaction()
            .map_err(|err| format!("failed beginning sqlite migration txn: {err}"))?;

        let mut imported = 0usize;
        let mut skipped = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "
                        INSERT OR IGNORE INTO gossip_items (
                            item_id,
                            envelope_b64,
                            expiry_unix_ms,
                            hop_count,
                            recorded_at_unix_ms
                        ) VALUES (?1, ?2, ?3, ?4, ?5)
                    ",
                )
                .map_err(|err| format!("failed preparing sqlite migration insert: {err}"))?;

            for legacy in parsed.items.values() {
                if legacy.item_id.trim().is_empty()
                    || legacy.envelope_b64.trim().is_empty()
                    || decode_envelope_payload_b64(&legacy.envelope_b64).is_err()
                {
                    skipped = skipped.saturating_add(1);
                    continue;
                }

                stmt.execute(params![
                    &legacy.item_id,
                    &legacy.envelope_b64,
                    legacy.expiry_unix_ms as i64,
                    legacy.hop_count as i64,
                    legacy.recorded_at_unix_ms as i64,
                ])
                .map_err(|err| format!("failed inserting migrated gossip record: {err}"))?;
                imported = imported.saturating_add(1);
            }
        }
        set_legacy_json_migration_done(&tx)?;
        tx.commit()
            .map_err(|err| format!("failed committing sqlite migration txn: {err}"))?;

        let archived_path = if migration_source_path == legacy_path {
            let next_backup_path = unique_backup_store_path(&legacy_path);
            fs::rename(&legacy_path, &next_backup_path).map_err(|err| {
                format!(
                    "failed renaming legacy gossip json {} -> {}: {err}",
                    legacy_path.display(),
                    next_backup_path.display()
                )
            })?;
            Some(next_backup_path)
        } else {
            None
        };

        let archived_path_display = archived_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string());

        log_verbose(&format!(
            "sqlite_migration_done: imported={} skipped={} elapsed_ms={} source_path={} archived_path={}",
            imported,
            skipped,
            started.elapsed().as_millis(),
            migration_source_path.display(),
            archived_path_display
        ));
        Ok(())
    }
}

fn legacy_json_migration_is_done(conn: &Connection) -> Result<bool, String> {
    let marker: Option<i64> = conn
        .query_row(
            "SELECT meta_value FROM gossip_meta WHERE meta_key = ?1 LIMIT 1",
            params![MIGRATION_META_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("failed reading sqlite migration marker: {err}"))?;
    match marker {
        None | Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(value) => Err(format!(
            "invalid sqlite migration marker value `{value}` for key `{MIGRATION_META_KEY}`"
        )),
    }
}

fn set_legacy_json_migration_done(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.execute(
        "
            INSERT INTO gossip_meta (meta_key, meta_value)
            VALUES (?1, 1)
            ON CONFLICT(meta_key) DO UPDATE
            SET meta_value = excluded.meta_value
        ",
        params![MIGRATION_META_KEY],
    )
    .map_err(|err| format!("failed writing sqlite migration marker: {err}"))?;
    Ok(())
}

fn backup_store_path(legacy_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", legacy_path.display()))
}

fn unique_backup_store_path(legacy_path: &Path) -> PathBuf {
    let default_backup_path = backup_store_path(legacy_path);
    if !default_backup_path.exists() {
        return default_backup_path;
    }

    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let mut suffix = 0u32;
    loop {
        let candidate = if suffix == 0 {
            PathBuf::from(format!("{}.bak.{unix_ms}", legacy_path.display()))
        } else {
            PathBuf::from(format!("{}.bak.{unix_ms}.{suffix}", legacy_path.display()))
        };
        if !candidate.exists() {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

#[cfg(test)]
pub fn has_item(item_id: &str) -> Result<bool, String> {
    with_connection("has_item", |conn| {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM gossip_items WHERE item_id = ?1 LIMIT 1",
                params![item_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| format!("sqlite has_item query failed: {err}"))?;
        Ok(exists.is_some())
    })
}

pub fn eligible_item_ids(now_ms: u64) -> Result<Vec<String>, String> {
    with_connection("eligible_item_ids", |conn| {
        let prune_started = Instant::now();
        let pruned = prune_expired(conn, now_ms)?;
        log_verbose(&format!(
            "sqlite_prune_expired: deleted={} elapsed_ms={}",
            pruned,
            prune_started.elapsed().as_millis()
        ));

        let select_started = Instant::now();
        let mut stmt = conn
            .prepare("SELECT item_id FROM gossip_items ORDER BY item_id ASC")
            .map_err(|err| format!("sqlite eligible select prepare failed: {err}"))?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| format!("sqlite eligible select query failed: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("sqlite eligible select row decode failed: {err}"))?;
        log_verbose(&format!(
            "sqlite_eligible_select: item_ids={} elapsed_ms={}",
            ids.len(),
            select_started.elapsed().as_millis()
        ));
        Ok(ids)
    })
}

pub fn eligible_relay_ingest_item_ids(
    now_ms: u64,
    max_items: usize,
) -> Result<Vec<String>, String> {
    with_connection("eligible_relay_ingest", |conn| {
        let prune_started = Instant::now();
        let pruned = prune_expired(conn, now_ms)?;
        log_verbose(&format!(
            "sqlite_prune_expired: deleted={} elapsed_ms={}",
            pruned,
            prune_started.elapsed().as_millis()
        ));

        let select_started = Instant::now();
        let mut stmt = conn
            .prepare(
                "
                    SELECT item_id
                    FROM gossip_items
                    ORDER BY hop_count ASC,
                             LENGTH(envelope_b64) ASC,
                             recorded_at_unix_ms DESC,
                             item_id ASC
                    LIMIT ?1
                ",
            )
            .map_err(|err| format!("sqlite relay_ingest select prepare failed: {err}"))?;
        let ids = stmt
            .query_map(params![max_items as i64], |row| row.get::<_, String>(0))
            .map_err(|err| format!("sqlite relay_ingest select query failed: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("sqlite relay_ingest select row decode failed: {err}"))?;
        log_verbose(&format!(
            "sqlite_relay_ingest_select: item_ids={} elapsed_ms={}",
            ids.len(),
            select_started.elapsed().as_millis()
        ));
        Ok(ids)
    })
}

pub fn record_local_item(
    item_id: &str,
    envelope_b64: &str,
    expiry_unix_ms: u64,
    hop_count: u16,
    recorded_at_unix_ms: u64,
) -> Result<RecordPutOutcome, String> {
    with_connection("record_local_item", |conn| {
        let started = Instant::now();
        let tx = conn
            .transaction()
            .map_err(|err| format!("sqlite record txn begin failed: {err}"))?;

        let existing: Option<(String, u64, u64)> = tx
            .query_row(
                "
                    SELECT envelope_b64, expiry_unix_ms, recorded_at_unix_ms
                    FROM gossip_items
                    WHERE item_id = ?1
                ",
                params![item_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, i64>(2)? as u64,
                    ))
                },
            )
            .optional()
            .map_err(|err| format!("sqlite record existing row lookup failed: {err}"))?;

        let outcome = match existing {
            Some((existing_envelope_b64, existing_expiry, existing_recorded_at)) => {
                if existing_envelope_b64 != envelope_b64 {
                    return Err("existing item_id maps to different envelope bytes".to_string());
                }
                let next_expiry = existing_expiry.max(expiry_unix_ms);
                let next_recorded_at = existing_recorded_at.max(recorded_at_unix_ms);
                if next_expiry != existing_expiry || next_recorded_at != existing_recorded_at {
                    tx.execute(
                        "
                            UPDATE gossip_items
                            SET expiry_unix_ms = ?2,
                                recorded_at_unix_ms = ?3
                            WHERE item_id = ?1
                        ",
                        params![item_id, next_expiry as i64, next_recorded_at as i64],
                    )
                    .map_err(|err| format!("sqlite record refresh update failed: {err}"))?;
                    RecordPutOutcome::Refreshed {
                        refreshed_expiry_unix_ms: next_expiry,
                    }
                } else {
                    RecordPutOutcome::Dedupe
                }
            }
            None => {
                tx.execute(
                    "
                        INSERT INTO gossip_items (
                            item_id,
                            envelope_b64,
                            expiry_unix_ms,
                            hop_count,
                            recorded_at_unix_ms
                        ) VALUES (?1, ?2, ?3, ?4, ?5)
                    ",
                    params![
                        item_id,
                        envelope_b64,
                        expiry_unix_ms as i64,
                        hop_count as i64,
                        recorded_at_unix_ms as i64,
                    ],
                )
                .map_err(|err| format!("sqlite record insert failed: {err}"))?;
                RecordPutOutcome::Inserted
            }
        };

        let _ = prune_expired_tx(&tx, recorded_at_unix_ms)?;
        tx.commit()
            .map_err(|err| format!("sqlite record txn commit failed: {err}"))?;
        log_verbose(&format!(
            "sqlite_item_put: item_id={} outcome={:?} elapsed_ms={}",
            item_id,
            outcome,
            started.elapsed().as_millis()
        ));
        Ok(outcome)
    })
}

pub fn transfer_candidates_for_request(
    requested_item_ids: &[String],
    now_ms: u64,
) -> Result<Vec<StoredItemRecord>, String> {
    with_connection("transfer_candidates", |conn| {
        let min_expiry_ms = now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS);
        let mut out = Vec::new();
        let started = Instant::now();

        let mut stmt = conn
            .prepare(
                "
                    SELECT item_id, envelope_b64, expiry_unix_ms, hop_count, recorded_at_unix_ms
                    FROM gossip_items
                    WHERE item_id = ?1 AND expiry_unix_ms > ?2
                    LIMIT 1
                ",
            )
            .map_err(|err| format!("sqlite transfer select prepare failed: {err}"))?;

        for item_id in requested_item_ids {
            let row = stmt
                .query_row(params![item_id, min_expiry_ms as i64], |row| {
                    Ok(StoredItemRecord {
                        item_id: row.get(0)?,
                        envelope_b64: row.get(1)?,
                        expiry_unix_ms: row.get::<_, i64>(2)? as u64,
                        hop_count: row.get::<_, i64>(3)? as u16,
                        recorded_at_unix_ms: row.get::<_, i64>(4)? as u64,
                    })
                })
                .optional()
                .map_err(|err| format!("sqlite transfer select query failed: {err}"))?;
            if let Some(record) = row {
                out.push(record);
            }
        }

        log_verbose(&format!(
            "sqlite_transfer_candidates_select: requested={} candidates={} elapsed_ms={}",
            requested_item_ids.len(),
            out.len(),
            started.elapsed().as_millis()
        ));
        Ok(out)
    })
}

pub fn get_existing_items_for_ids(
    item_ids: &[String],
) -> Result<HashMap<String, StoredItemRecord>, String> {
    with_connection("existing_items_for_ids", |conn| {
        let started = Instant::now();
        let mut out = HashMap::new();

        for chunk in item_ids.chunks(SQLITE_MAX_VARIABLES) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "
                    SELECT item_id, envelope_b64, expiry_unix_ms, hop_count, recorded_at_unix_ms
                    FROM gossip_items
                    WHERE item_id IN ({placeholders})
                "
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|err| format!("sqlite existing-items prepare failed: {err}"))?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok(StoredItemRecord {
                        item_id: row.get(0)?,
                        envelope_b64: row.get(1)?,
                        expiry_unix_ms: row.get::<_, i64>(2)? as u64,
                        hop_count: row.get::<_, i64>(3)? as u16,
                        recorded_at_unix_ms: row.get::<_, i64>(4)? as u64,
                    })
                })
                .map_err(|err| format!("sqlite existing-items query failed: {err}"))?;

            for row in rows {
                let record =
                    row.map_err(|err| format!("sqlite existing-items row decode failed: {err}"))?;
                out.insert(record.item_id.clone(), record);
            }
        }

        log_verbose(&format!(
            "sqlite_existing_items_select: requested={} found={} elapsed_ms={}",
            item_ids.len(),
            out.len(),
            started.elapsed().as_millis()
        ));
        Ok(out)
    })
}

pub fn insert_import_items(items: &[ImportWriteObject], now_ms: u64) -> Result<(), String> {
    with_connection("insert_import_items", |conn| {
        let started = Instant::now();
        let tx = conn
            .transaction()
            .map_err(|err| format!("sqlite import txn begin failed: {err}"))?;

        if !items.is_empty() {
            let mut insert_stmt = tx
                .prepare(
                    "
                        INSERT OR IGNORE INTO gossip_items (
                            item_id,
                            envelope_b64,
                            expiry_unix_ms,
                            hop_count,
                            recorded_at_unix_ms
                        ) VALUES (?1, ?2, ?3, ?4, ?5)
                    ",
                )
                .map_err(|err| format!("sqlite import insert prepare failed: {err}"))?;

            for item in items {
                insert_stmt
                    .execute(params![
                        &item.item_id,
                        &item.envelope_b64,
                        item.expiry_unix_ms as i64,
                        item.hop_count as i64,
                        item.recorded_at_unix_ms as i64,
                    ])
                    .map_err(|err| format!("sqlite import insert failed: {err}"))?;
            }
        }

        let pruned = prune_expired_tx(&tx, now_ms)?;
        tx.commit()
            .map_err(|err| format!("sqlite import txn commit failed: {err}"))?;
        log_verbose(&format!(
            "sqlite_import_txn: attempted_inserts={} pruned={} elapsed_ms={}",
            items.len(),
            pruned,
            started.elapsed().as_millis()
        ));
        Ok(())
    })
}

pub fn summary_preview_candidates(now_ms: u64) -> Result<Vec<StoredItemRecord>, String> {
    with_connection("summary_preview_candidates", |conn| {
        let prune_started = Instant::now();
        let pruned = prune_expired(conn, now_ms)?;
        log_verbose(&format!(
            "sqlite_prune_expired: deleted={} elapsed_ms={}",
            pruned,
            prune_started.elapsed().as_millis()
        ));

        let select_started = Instant::now();
        let mut stmt = conn
            .prepare(
                "
                    SELECT item_id, envelope_b64, expiry_unix_ms, hop_count, recorded_at_unix_ms
                    FROM gossip_items
                    ORDER BY hop_count ASC,
                             LENGTH(envelope_b64) ASC,
                             recorded_at_unix_ms DESC,
                             item_id ASC
                ",
            )
            .map_err(|err| format!("sqlite summary-preview select prepare failed: {err}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StoredItemRecord {
                    item_id: row.get(0)?,
                    envelope_b64: row.get(1)?,
                    expiry_unix_ms: row.get::<_, i64>(2)? as u64,
                    hop_count: row.get::<_, i64>(3)? as u16,
                    recorded_at_unix_ms: row.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(|err| format!("sqlite summary-preview select query failed: {err}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("sqlite summary-preview select row decode failed: {err}"))?;
        log_verbose(&format!(
            "sqlite_summary_preview_select: items={} elapsed_ms={}",
            rows.len(),
            select_started.elapsed().as_millis()
        ));
        Ok(rows)
    })
}

fn prune_expired(conn: &mut Connection, now_ms: u64) -> Result<usize, String> {
    let tx = conn
        .transaction()
        .map_err(|err| format!("sqlite prune txn begin failed: {err}"))?;
    let deleted = prune_expired_tx(&tx, now_ms)?;
    tx.commit()
        .map_err(|err| format!("sqlite prune txn commit failed: {err}"))?;
    Ok(deleted)
}

fn prune_expired_tx(tx: &rusqlite::Transaction<'_>, now_ms: u64) -> Result<usize, String> {
    let min_expiry = now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS);
    tx.execute(
        "DELETE FROM gossip_items WHERE expiry_unix_ms <= ?1 OR envelope_b64 = ''",
        params![min_expiry as i64],
    )
    .map_err(|err| format!("sqlite prune expired failed: {err}"))
}

pub fn sqlite_store_path() -> PathBuf {
    if let Some(state_dir) = test_state_dir_override() {
        return state_dir.join(SQLITE_STORE_FILE_NAME);
    }

    if let Ok(state_dir) = std::env::var("AETHOS_STATE_DIR") {
        if !state_dir.trim().is_empty() {
            return Path::new(&state_dir).join(SQLITE_STORE_FILE_NAME);
        }
    }

    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(SQLITE_STORE_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(SQLITE_STORE_FILE_NAME);
    }

    std::env::temp_dir().join(SQLITE_STORE_FILE_NAME)
}

fn legacy_json_store_path() -> PathBuf {
    if let Some(state_dir) = test_state_dir_override() {
        return state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
    }

    if let Ok(state_dir) = std::env::var("AETHOS_STATE_DIR") {
        if !state_dir.trim().is_empty() {
            return Path::new(&state_dir).join(LEGACY_JSON_STORE_FILE_NAME);
        }
    }

    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(LEGACY_JSON_STORE_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(LEGACY_JSON_STORE_FILE_NAME);
    }

    std::env::temp_dir().join(LEGACY_JSON_STORE_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::Digest;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_env_lock() -> &'static Mutex<()> {
        TEST_ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unique_state_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("{prefix}-{nanos}-{counter}"))
    }

    fn reset_runtime_for_tests() {
        let mut runtime = runtime_mutex()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        runtime.conn = None;
        runtime.current_db_path = None;
    }

    fn backup_path_for(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.bak", path.display()))
    }

    fn build_legacy_payload_and_item_id(seed: [u8; 32], text: &str) -> (String, String) {
        let payload = crate::aethos_core::protocol::build_envelope_payload_b64_from_utf8(
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            text,
            &seed,
        )
        .expect("legacy payload");
        let item_id = {
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&payload)
                .expect("decode payload");
            crate::aethos_core::protocol::bytes_to_hex_lower(&sha2::Sha256::digest(raw))
        };
        (payload, item_id)
    }

    fn write_legacy_json(path: &Path, payload: &str, item_id: &str) {
        let legacy_json = serde_json::json!({
            "items": {
                item_id: {
                    "item_id": item_id,
                    "envelope_b64": payload,
                    "expiry_unix_ms": 1_900_000_000_000u64,
                    "hop_count": 0,
                    "recorded_at_unix_ms": 1_700_000_000_000u64
                }
            }
        });
        fs::write(
            path,
            serde_json::to_string_pretty(&legacy_json).expect("serialize legacy"),
        )
        .expect("write legacy store");
    }

    #[test]
    fn sqlite_insert_dedupe_refresh_and_prune_behave() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-store");
        with_test_state_dir(&state_dir, || {
            let item_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            let payload = crate::aethos_core::protocol::build_envelope_payload_b64_from_utf8(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "hello",
                &[7u8; 32],
            )
            .expect("payload");
            let now_ms = 1_700_000_000_000u64;

            let first = record_local_item(item_id, &payload, now_ms + 60_000, 0, now_ms)
                .expect("insert item");
            assert_eq!(first, RecordPutOutcome::Inserted);

            let second = record_local_item(item_id, &payload, now_ms + 60_000, 0, now_ms)
                .expect("dedupe item");
            assert_eq!(second, RecordPutOutcome::Dedupe);

            let third = record_local_item(item_id, &payload, now_ms + 120_000, 0, now_ms + 5)
                .expect("refresh item");
            assert!(matches!(
                third,
                RecordPutOutcome::Refreshed {
                    refreshed_expiry_unix_ms: _
                }
            ));

            let ids = eligible_item_ids(now_ms).expect("eligible ids");
            assert!(ids.iter().any(|value| value == item_id));
            assert!(has_item(item_id).expect("item should exist after insert"));

            let _ = eligible_item_ids(now_ms + 200_000).expect("eligible ids after prune");
            assert!(!has_item(item_id).expect("item should be pruned"));
        });
    }

    #[test]
    fn sqlite_eligible_relay_ingest_selection_is_ranked() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-ranked");
        with_test_state_dir(&state_dir, || {
            let now_ms = 1_700_000_000_000u64;
            let payload_a = crate::aethos_core::protocol::build_envelope_payload_b64_from_utf8(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "a",
                &[1u8; 32],
            )
            .expect("payload a");
            let payload_b = crate::aethos_core::protocol::build_envelope_payload_b64_from_utf8(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "bbbbbb",
                &[2u8; 32],
            )
            .expect("payload b");

            record_local_item(
                "1111111111111111111111111111111111111111111111111111111111111111",
                &payload_b,
                now_ms + 60_000,
                1,
                now_ms,
            )
            .expect("write item one");
            record_local_item(
                "0000000000000000000000000000000000000000000000000000000000000000",
                &payload_a,
                now_ms + 60_000,
                0,
                now_ms,
            )
            .expect("write item two");

            let ranked = eligible_relay_ingest_item_ids(now_ms, 8).expect("ranked eligible");
            let first_idx = ranked
                .iter()
                .position(|value| {
                    value == "0000000000000000000000000000000000000000000000000000000000000000"
                })
                .expect("ranked should include first item");
            let second_idx = ranked
                .iter()
                .position(|value| {
                    value == "1111111111111111111111111111111111111111111111111111111111111111"
                })
                .expect("ranked should include second item");
            assert!(
                first_idx < second_idx,
                "expected lower-hop candidate to rank ahead"
            );
        });
    }

    #[test]
    fn sqlite_migrates_legacy_json_once_and_renames_backup() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-migration");
        fs::create_dir_all(&state_dir).expect("create state dir");
        with_test_state_dir(&state_dir, || {
            let (payload, item_id) = build_legacy_payload_and_item_id([9u8; 32], "legacy");
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            write_legacy_json(&legacy_path, &payload, &item_id);

            let ids = eligible_item_ids(1_700_000_000_000u64).expect("eligible after migration");
            assert!(ids.iter().any(|value| value == &item_id));
            assert!(has_item(&item_id).expect("migrated item should exist"));
            assert!(!legacy_path.exists());
            assert!(backup_path_for(&legacy_path).exists());
        });
        reset_runtime_for_tests();
    }

    #[test]
    fn sqlite_migration_is_idempotent_when_retried_from_backup() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-migration-idempotent");
        fs::create_dir_all(&state_dir).expect("create state dir");
        with_test_state_dir(&state_dir, || {
            let (payload, item_id) = build_legacy_payload_and_item_id([10u8; 32], "legacy");
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            write_legacy_json(&legacy_path, &payload, &item_id);

            let first_ids = eligible_item_ids(1_700_000_000_000u64).expect("first migration run");
            assert!(first_ids.iter().any(|value| value == &item_id));
            let backup_path = backup_path_for(&legacy_path);
            assert!(
                backup_path.exists(),
                "expected legacy backup after migration"
            );

            with_connection("test_reset_migration_marker", |conn| {
                conn.execute(
                    "DELETE FROM gossip_meta WHERE meta_key = ?1",
                    params![MIGRATION_META_KEY],
                )
                .map_err(|err| format!("failed deleting migration marker in test: {err}"))?;
                Ok(())
            })
            .expect("clear marker");
            reset_runtime_for_tests();

            let second_ids = eligible_item_ids(1_700_000_000_000u64)
                .expect("second migration retry from backup");
            assert_eq!(
                second_ids.iter().filter(|value| *value == &item_id).count(),
                1,
                "retry should not duplicate migrated record"
            );

            let item_count: i64 = with_connection("test_count_items", |conn| {
                conn.query_row("SELECT COUNT(*) FROM gossip_items", [], |row| row.get(0))
                    .map_err(|err| format!("failed counting items in test: {err}"))
            })
            .expect("count items");
            assert_eq!(item_count, 1);
        });
        reset_runtime_for_tests();
    }

    #[test]
    fn sqlite_migrates_when_db_file_exists_without_marker() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-existing-db-no-marker");
        fs::create_dir_all(&state_dir).expect("create state dir");
        with_test_state_dir(&state_dir, || {
            let sqlite_path = sqlite_store_path();
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            let (payload, item_id) = build_legacy_payload_and_item_id([11u8; 32], "legacy");
            write_legacy_json(&legacy_path, &payload, &item_id);

            let conn = Connection::open(&sqlite_path).expect("open preexisting sqlite");
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS gossip_items (
                    item_id TEXT PRIMARY KEY NOT NULL,
                    envelope_b64 TEXT NOT NULL,
                    expiry_unix_ms INTEGER NOT NULL,
                    hop_count INTEGER NOT NULL,
                    recorded_at_unix_ms INTEGER NOT NULL
                );
                ",
            )
            .expect("ensure minimal gossip_items schema");
            drop(conn);

            let ids = eligible_item_ids(1_700_000_000_000u64).expect("migration should run");
            assert!(ids.iter().any(|value| value == &item_id));
            assert!(!legacy_path.exists(), "legacy json should be archived");
            assert!(
                backup_path_for(&legacy_path).exists(),
                "expected backup file"
            );
        });
        reset_runtime_for_tests();
    }

    #[test]
    fn sqlite_migration_preserves_existing_backup_file() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-preserve-existing-bak");
        fs::create_dir_all(&state_dir).expect("create state dir");
        with_test_state_dir(&state_dir, || {
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            let default_backup_path = backup_path_for(&legacy_path);
            let default_backup_contents = "preexisting backup must be preserved";
            fs::write(&default_backup_path, default_backup_contents)
                .expect("write existing backup");

            let (payload, item_id) = build_legacy_payload_and_item_id([12u8; 32], "legacy");
            write_legacy_json(&legacy_path, &payload, &item_id);

            let ids = eligible_item_ids(1_700_000_000_000u64).expect("eligible after migration");
            assert!(ids.iter().any(|value| value == &item_id));

            let preserved = fs::read_to_string(&default_backup_path).expect("read existing backup");
            assert_eq!(preserved, default_backup_contents);

            let backup_prefix = format!("{}.bak.", legacy_path.display());
            let archived_backup_candidates = fs::read_dir(&state_dir)
                .expect("read state dir")
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.to_string_lossy().starts_with(&backup_prefix) && path.is_file())
                .collect::<Vec<_>>();
            assert!(
                !archived_backup_candidates.is_empty(),
                "expected archived legacy json path with timestamp suffix"
            );
        });
        reset_runtime_for_tests();
    }

    #[test]
    fn sqlite_migration_imports_from_backup_when_json_missing() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-migrate-from-backup");
        fs::create_dir_all(&state_dir).expect("create state dir");
        with_test_state_dir(&state_dir, || {
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            let backup_path = backup_path_for(&legacy_path);
            let (payload, item_id) = build_legacy_payload_and_item_id([13u8; 32], "legacy");
            write_legacy_json(&backup_path, &payload, &item_id);

            let ids = eligible_item_ids(1_700_000_000_000u64).expect("migration from backup");
            assert!(ids.iter().any(|value| value == &item_id));
            assert!(!legacy_path.exists(), "json file should remain absent");
            assert!(
                backup_path.exists(),
                "backup source should remain available"
            );
        });
        reset_runtime_for_tests();
    }

    #[test]
    fn sqlite_hot_path_does_not_rewrite_legacy_json_store() {
        let _lock = test_env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state_dir = unique_state_dir("aethos-gossip-sqlite-no-legacy-rewrite");
        with_test_state_dir(&state_dir, || {
            fs::create_dir_all(&state_dir).expect("create state dir");
            let now_ms = 1_700_000_000_000u64;
            let item_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
            let payload = crate::aethos_core::protocol::build_envelope_payload_b64_from_utf8(
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "sqlite-only",
                &[3u8; 32],
            )
            .expect("payload");

            record_local_item(item_id, &payload, now_ms + 60_000, 0, now_ms)
                .expect("write sqlite gossip item");

            let sqlite_path = state_dir.join(SQLITE_STORE_FILE_NAME);
            let legacy_path = state_dir.join(LEGACY_JSON_STORE_FILE_NAME);
            assert!(sqlite_path.exists(), "expected sqlite store to exist");
            assert!(
                !legacy_path.exists(),
                "legacy json store should not be rewritten"
            );
        });
    }
}
