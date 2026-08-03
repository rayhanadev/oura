use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use oura_protocol::events::RingEvent;
use tokio::sync::mpsc::UnboundedReceiver;
use turso::{Connection, Database, Value, params};

pub const DECODER_REVISION: &str = "c5106bd5674dd98f07954b96dff9e16c8fe26f06";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version        INTEGER PRIMARY KEY,
    applied_unix   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
    serial         TEXT PRIMARY KEY,
    hardware_id    TEXT,
    firmware       TEXT,
    api_version    TEXT,
    mac             TEXT,
    updated_unix    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_state (
    serial          TEXT PRIMARY KEY,
    next_cursor     INTEGER NOT NULL,
    last_sync_unix  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    event_key        TEXT PRIMARY KEY,
    serial           TEXT NOT NULL,
    tag              INTEGER NOT NULL,
    name             TEXT NOT NULL,
    ring_timestamp   INTEGER NOT NULL,
    body             BLOB NOT NULL,
    decoded_json     TEXT,
    decoder_revision TEXT NOT NULL,
    captured_unix    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_serial_time
    ON events(serial, ring_timestamp);
CREATE INDEX IF NOT EXISTS idx_events_serial_tag
    ON events(serial, tag);

CREATE TABLE IF NOT EXISTS readings (
    reading_key    TEXT PRIMARY KEY,
    serial         TEXT NOT NULL,
    kind           TEXT NOT NULL,
    value          REAL NOT NULL,
    unit           TEXT NOT NULL,
    captured_unix  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_readings_serial_kind_time
    ON readings(serial, kind, captured_unix);

CREATE TABLE IF NOT EXISTS sync_runs (
    run_id          TEXT PRIMARY KEY,
    serial          TEXT NOT NULL,
    started_unix    INTEGER NOT NULL,
    finished_unix   INTEGER,
    start_cursor    INTEGER NOT NULL,
    end_cursor      INTEGER NOT NULL,
    received        INTEGER NOT NULL DEFAULT 0,
    inserted        INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL,
    error           TEXT
);
CREATE INDEX IF NOT EXISTS idx_sync_runs_started
    ON sync_runs(started_unix DESC);

CREATE TABLE IF NOT EXISTS notification_state (
    serial              TEXT NOT NULL,
    kind                TEXT NOT NULL,
    active              INTEGER NOT NULL DEFAULT 0,
    last_value          REAL,
    last_notified_unix  INTEGER,
    updated_unix        INTEGER NOT NULL,
    PRIMARY KEY(serial, kind)
);

INSERT OR IGNORE INTO schema_migrations(version, applied_unix)
VALUES (1, unixepoch());
INSERT OR IGNORE INTO schema_migrations(version, applied_unix)
VALUES (2, unixepoch());
"#;

#[derive(Clone)]
pub struct Store {
    database: Database,
}

#[derive(Debug)]
pub enum BatchMessage {
    Event(RingEvent),
    Commit(u32),
}

#[derive(Debug, Default)]
pub struct WriteSummary {
    pub received: u32,
    pub inserted: u32,
    pub end_cursor: u32,
}

#[derive(Debug)]
pub struct StatusSummary {
    pub device: Option<DeviceSummary>,
    pub event_count: i64,
    pub event_counts: Vec<EventCount>,
    pub latest_battery: Option<LatestReading>,
    pub last_run: Option<LastRun>,
}

#[derive(Debug)]
pub struct DeviceSummary {
    pub serial: String,
    pub hardware_id: Option<String>,
    pub firmware: Option<String>,
    pub api_version: Option<String>,
    pub mac: Option<String>,
    pub updated_unix: i64,
}

#[derive(Debug)]
pub struct EventCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug)]
pub struct LatestReading {
    pub value: f64,
    pub unit: String,
    pub captured_unix: i64,
}

#[derive(Debug, Default)]
pub struct TodaySummary {
    pub heart_rate_median: Option<f64>,
    pub heart_rate_samples: usize,
    pub temperature_records: usize,
    pub motion_periods: usize,
    pub motion_seconds: u64,
    pub activity_samples: usize,
    pub activity_met_average: Option<f64>,
    pub spo2_percent_samples: usize,
    pub spo2_signal_records: usize,
}

#[derive(Debug)]
pub struct LastRun {
    pub started_unix: i64,
    pub finished_unix: Option<i64>,
    pub inserted: i64,
    pub status: String,
    pub error: Option<String>,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating database directory {}", parent.display()))?;
        }
        let path_text = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("database path is not valid UTF-8"))?;
        let database = turso::Builder::new_local(path_text)
            .build()
            .await
            .with_context(|| format!("opening Turso database {}", path.display()))?;
        let connection = database.connect()?;
        connection
            .execute_batch(SCHEMA)
            .await
            .context("applying database schema")?;
        harden_database_files(path)?;
        Ok(Self { database })
    }

    pub fn database(&self) -> Database {
        self.database.clone()
    }

    pub async fn upsert_device(
        &self,
        serial: &str,
        hardware_id: Option<&str>,
        firmware: Option<&str>,
        api_version: Option<&str>,
        mac: Option<&str>,
    ) -> Result<()> {
        let connection = self.database.connect()?;
        connection
            .execute(
                "INSERT INTO devices
                   (serial, hardware_id, firmware, api_version, mac, updated_unix)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(serial) DO UPDATE SET
                   hardware_id=COALESCE(excluded.hardware_id, devices.hardware_id),
                   firmware=COALESCE(excluded.firmware, devices.firmware),
                   api_version=COALESCE(excluded.api_version, devices.api_version),
                   mac=COALESCE(excluded.mac, devices.mac),
                   updated_unix=excluded.updated_unix",
                params![serial, hardware_id, firmware, api_version, mac, now_unix(),],
            )
            .await?;
        Ok(())
    }

    pub async fn cursor(&self, serial: &str) -> Result<u32> {
        let connection = self.database.connect()?;
        let mut rows = connection
            .query(
                "SELECT next_cursor FROM sync_state WHERE serial = ?1",
                [serial],
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(0);
        };
        let cursor: i64 = row.get(0)?;
        Ok(cursor.try_into().unwrap_or(0))
    }

    pub async fn insert_reading(
        &self,
        serial: &str,
        kind: &str,
        value: f64,
        unit: &str,
    ) -> Result<()> {
        let captured = now_unix();
        let mut hasher = blake3::Hasher::new();
        hasher.update(serial.as_bytes());
        hasher.update(kind.as_bytes());
        hasher.update(&captured.to_le_bytes());
        hasher.update(&value.to_le_bytes());
        let key = hasher.finalize().to_hex().to_string();
        let connection = self.database.connect()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO readings
                   (reading_key, serial, kind, value, unit, captured_unix)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key, serial, kind, value, unit, captured],
            )
            .await?;
        Ok(())
    }

    pub async fn low_battery_notification_due(
        &self,
        serial: &str,
        percent: u8,
        threshold: u8,
    ) -> Result<bool> {
        if threshold == 0 {
            return Ok(false);
        }
        let connection = self.database.connect()?;
        let mut rows = connection
            .query(
                "SELECT active FROM notification_state
                 WHERE serial = ?1 AND kind = 'low_battery'",
                [serial],
            )
            .await?;
        let active = rows
            .next()
            .await?
            .map(|row| row.get::<i64>(0).map(|value| value != 0))
            .transpose()?
            .unwrap_or(false);

        if percent >= threshold {
            connection
                .execute(
                    "INSERT INTO notification_state
                       (serial, kind, active, last_value, updated_unix)
                     VALUES (?1, 'low_battery', 0, ?2, ?3)
                     ON CONFLICT(serial, kind) DO UPDATE SET
                       active=0, last_value=excluded.last_value,
                       updated_unix=excluded.updated_unix",
                    params![serial, percent, now_unix()],
                )
                .await?;
            return Ok(false);
        }

        connection
            .execute(
                "INSERT INTO notification_state
                   (serial, kind, active, last_value, updated_unix)
                 VALUES (?1, 'low_battery', 0, ?2, ?3)
                 ON CONFLICT(serial, kind) DO UPDATE SET
                   last_value=excluded.last_value, updated_unix=excluded.updated_unix",
                params![serial, percent, now_unix()],
            )
            .await?;
        Ok(!active)
    }

    pub async fn mark_low_battery_notified(&self, serial: &str, percent: u8) -> Result<()> {
        let connection = self.database.connect()?;
        connection
            .execute(
                "INSERT INTO notification_state
                   (serial, kind, active, last_value, last_notified_unix, updated_unix)
                 VALUES (?1, 'low_battery', 1, ?2, ?3, ?3)
                 ON CONFLICT(serial, kind) DO UPDATE SET
                   active=1, last_value=excluded.last_value,
                   last_notified_unix=excluded.last_notified_unix,
                   updated_unix=excluded.updated_unix",
                params![serial, percent, now_unix()],
            )
            .await?;
        Ok(())
    }

    pub async fn begin_run(&self, serial: &str, start_cursor: u32) -> Result<String> {
        let started = now_unix();
        let run_id = format!("{serial}-{started}-{}", std::process::id());
        let connection = self.database.connect()?;
        connection
            .execute(
                "INSERT INTO sync_runs
                   (run_id, serial, started_unix, start_cursor, end_cursor, status)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'running')",
                params![run_id.as_str(), serial, started, start_cursor],
            )
            .await?;
        Ok(run_id)
    }

    pub async fn finish_run(
        &self,
        run_id: &str,
        summary: &WriteSummary,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let connection = self.database.connect()?;
        connection
            .execute(
                "UPDATE sync_runs SET
                   finished_unix=?1,
                   end_cursor=?2,
                   received=?3,
                   inserted=?4,
                   status=?5,
                   error=?6
                 WHERE run_id=?7",
                params![
                    now_unix(),
                    summary.end_cursor,
                    summary.received,
                    summary.inserted,
                    status,
                    error,
                    run_id,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn status(&self) -> Result<StatusSummary> {
        let connection = self.database.connect()?;
        let event_count = scalar_i64(&connection, "SELECT COUNT(*) FROM events").await?;

        let mut device_rows = connection
            .query(
                "SELECT serial, hardware_id, firmware, api_version, mac, updated_unix
                 FROM devices ORDER BY updated_unix DESC LIMIT 1",
                (),
            )
            .await?;
        let device = if let Some(row) = device_rows.next().await? {
            Some(DeviceSummary {
                serial: row.get(0)?,
                hardware_id: optional_text(row.get_value(1)?),
                firmware: optional_text(row.get_value(2)?),
                api_version: optional_text(row.get_value(3)?),
                mac: optional_text(row.get_value(4)?),
                updated_unix: row.get(5)?,
            })
        } else {
            None
        };

        let mut count_rows = connection
            .query(
                "SELECT name, COUNT(*) AS count
                 FROM events GROUP BY name ORDER BY count DESC, name",
                (),
            )
            .await?;
        let mut event_counts = Vec::new();
        while let Some(row) = count_rows.next().await? {
            event_counts.push(EventCount {
                name: row.get(0)?,
                count: row.get(1)?,
            });
        }

        let mut battery_rows = connection
            .query(
                "SELECT value, unit, captured_unix
                 FROM readings WHERE kind = 'battery_percent'
                 ORDER BY captured_unix DESC LIMIT 1",
                (),
            )
            .await?;
        let latest_battery = if let Some(row) = battery_rows.next().await? {
            Some(LatestReading {
                value: row.get(0)?,
                unit: row.get(1)?,
                captured_unix: row.get(2)?,
            })
        } else {
            None
        };

        let mut rows = connection
            .query(
                "SELECT started_unix, finished_unix, inserted, status, error
                 FROM sync_runs ORDER BY started_unix DESC LIMIT 1",
                (),
            )
            .await?;
        let last_run = if let Some(row) = rows.next().await? {
            let finished_unix = match row.get_value(1)? {
                Value::Null => None,
                Value::Integer(value) => Some(value),
                _ => None,
            };
            let error = match row.get_value(4)? {
                Value::Null => None,
                Value::Text(value) => Some(value),
                other => Some(format!("{other:?}")),
            };
            Some(LastRun {
                started_unix: row.get(0)?,
                finished_unix,
                inserted: row.get(2)?,
                status: row.get(3)?,
                error,
            })
        } else {
            None
        };

        Ok(StatusSummary {
            device,
            event_count,
            event_counts,
            latest_battery,
            last_run,
        })
    }

    pub async fn today(&self, start_unix: i64, end_unix: i64) -> Result<TodaySummary> {
        let connection = self.database.connect()?;
        let mut sync_rows = connection
            .query(
                "SELECT ring_timestamp, decoded_json
                 FROM events
                 WHERE name = 'time_sync' AND decoded_json IS NOT NULL
                 ORDER BY ring_timestamp DESC LIMIT 1",
                (),
            )
            .await?;
        let Some(sync_row) = sync_rows.next().await? else {
            return Ok(TodaySummary::default());
        };
        let sync_tick: i64 = sync_row.get(0)?;
        let sync_json: String = sync_row.get(1)?;
        let sync_value: serde_json::Value = serde_json::from_str(&sync_json)?;
        let sync_unix = sync_value
            .get("unix_time")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("time-sync event has no Unix timestamp"))?;

        let start_tick = (sync_tick + (start_unix - sync_unix) * 10).max(0);
        let end_tick = (sync_tick + (end_unix - sync_unix) * 10).max(start_tick);
        let mut rows = connection
            .query(
                "SELECT name, decoded_json
                 FROM events
                 WHERE ring_timestamp >= ?1 AND ring_timestamp < ?2
                   AND decoded_json IS NOT NULL
                   AND name IN (
                     'green_ibi_quality_event', 'ibi_and_amplitude_event',
                     'hrv_event', 'aohr_event', 'temp_event', 'temp_period',
                     'sleep_temp', 'motion_event', 'motion_period',
                     'activity_information', 'spo2_event', 'spo2_r_pi_event',
                     'spo2_smoothed_event', 'spo2_ibi_and_amplitude_event'
                   )",
                params![start_tick, end_tick],
            )
            .await?;

        let mut summary = TodaySummary::default();
        let mut heart_rates = Vec::new();
        let mut activity_met_total = 0.0;
        while let Some(row) = rows.next().await? {
            let name: String = row.get(0)?;
            let decoded: String = row.get(1)?;
            let value: serde_json::Value = serde_json::from_str(&decoded)?;

            for key in ["hr_bpm", "bpm"] {
                if let Some(samples) = value.get(key).and_then(serde_json::Value::as_array) {
                    heart_rates.extend(
                        samples
                            .iter()
                            .filter_map(serde_json::Value::as_f64)
                            .filter(|sample| (25.0..=240.0).contains(sample)),
                    );
                }
            }

            if matches!(name.as_str(), "temp_event" | "temp_period" | "sleep_temp") {
                summary.temperature_records += 1;
            }
            if name == "motion_event" || name == "motion_period" {
                summary.motion_periods += 1;
                summary.motion_seconds += value
                    .get("motion_seconds")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
            }
            if name == "activity_information"
                && let Some(samples) = value.get("met").and_then(serde_json::Value::as_array)
            {
                for sample in samples.iter().filter_map(serde_json::Value::as_f64) {
                    summary.activity_samples += 1;
                    activity_met_total += sample;
                }
            }
            if name.contains("spo2") {
                summary.spo2_signal_records += 1;
                summary.spo2_percent_samples += value
                    .get("spo2_percent")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
            }
        }

        heart_rates.sort_by(f64::total_cmp);
        summary.heart_rate_samples = heart_rates.len();
        summary.heart_rate_median = median(&heart_rates);
        if summary.activity_samples > 0 {
            summary.activity_met_average =
                Some(activity_met_total / summary.activity_samples as f64);
        }
        Ok(summary)
    }
}

fn median(values: &[f64]) -> Option<f64> {
    let midpoint = values.len() / 2;
    match values.len() {
        0 => None,
        length if length.is_multiple_of(2) => Some((values[midpoint - 1] + values[midpoint]) / 2.0),
        _ => Some(values[midpoint]),
    }
}

fn optional_text(value: Value) -> Option<String> {
    match value {
        Value::Text(value) => Some(value),
        _ => None,
    }
}

pub async fn write_batches(
    database: Database,
    serial: String,
    start_cursor: u32,
    mut receiver: UnboundedReceiver<BatchMessage>,
) -> Result<WriteSummary> {
    let mut connection = database.connect()?;
    let mut pending = Vec::new();
    let mut summary = WriteSummary {
        end_cursor: start_cursor,
        ..WriteSummary::default()
    };

    while let Some(message) = receiver.recv().await {
        match message {
            BatchMessage::Event(event) => {
                summary.received += 1;
                pending.push(event);
            }
            BatchMessage::Commit(cursor) => {
                let captured = now_unix();
                let transaction = connection.transaction().await?;
                for event in pending.drain(..) {
                    let key = event_key(&serial, &event);
                    let decoded = event
                        .decoded
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?;
                    let changed = transaction
                        .execute(
                            "INSERT OR IGNORE INTO events
                               (event_key, serial, tag, name, ring_timestamp, body,
                                decoded_json, decoder_revision, captured_unix)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                            params![
                                key,
                                serial.as_str(),
                                event.tag,
                                event.name,
                                event.timestamp,
                                event.body,
                                decoded,
                                DECODER_REVISION,
                                captured,
                            ],
                        )
                        .await?;
                    summary.inserted += changed as u32;
                }
                transaction
                    .execute(
                        "INSERT INTO sync_state(serial, next_cursor, last_sync_unix)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT(serial) DO UPDATE SET
                           next_cursor=excluded.next_cursor,
                           last_sync_unix=excluded.last_sync_unix",
                        params![serial.as_str(), cursor, captured],
                    )
                    .await?;
                transaction.commit().await?;
                summary.end_cursor = cursor;
            }
        }
    }

    if !pending.is_empty() {
        anyhow::bail!("event writer ended with an uncommitted partial batch");
    }
    Ok(summary)
}

fn event_key(serial: &str, event: &RingEvent) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(serial.len() as u64).to_le_bytes());
    hasher.update(serial.as_bytes());
    hasher.update(&[event.tag]);
    hasher.update(&event.timestamp.to_le_bytes());
    hasher.update(&(event.body.len() as u64).to_le_bytes());
    hasher.update(&event.body);
    hasher.finalize().to_hex().to_string()
}

async fn scalar_i64(connection: &Connection, sql: &str) -> Result<i64> {
    let mut rows = connection.query(sql, ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| anyhow::anyhow!("query returned no rows: {sql}"))?;
    Ok(row.get(0)?)
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn harden_database_files(path: &Path) -> Result<()> {
    let base = path.to_string_lossy();
    for candidate in [
        path.to_path_buf(),
        format!("{base}-wal").into(),
        format!("{base}-shm").into(),
        format!("{base}.tshm").into(),
    ] {
        if candidate.exists() {
            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("securing {}", candidate.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn event_batches_are_atomic_and_idempotent() {
        let temp = tempdir().unwrap();
        let store = Store::open(&temp.path().join("oura.db")).await.unwrap();
        let event = RingEvent {
            tag: 0x43,
            name: "debug_event",
            timestamp: 42,
            body: vec![1, 2, 3],
            decoded: None,
        };

        for expected_inserted in [1, 0] {
            let (sender, receiver) = mpsc::unbounded_channel();
            sender.send(BatchMessage::Event(event.clone())).unwrap();
            sender.send(BatchMessage::Commit(43)).unwrap();
            drop(sender);
            let summary = write_batches(store.database(), "S1".into(), 0, receiver)
                .await
                .unwrap();
            assert_eq!(summary.inserted, expected_inserted);
            assert_eq!(summary.end_cursor, 43);
        }

        assert_eq!(store.cursor("S1").await.unwrap(), 43);
        assert_eq!(store.status().await.unwrap().event_count, 1);
    }

    #[tokio::test]
    async fn today_converts_ring_ticks_and_summarizes_measurements() {
        let temp = tempdir().unwrap();
        let store = Store::open(&temp.path().join("oura.db")).await.unwrap();
        let events = [
            RingEvent {
                tag: 1,
                name: "time_sync",
                timestamp: 100,
                body: vec![1],
                decoded: Some(serde_json::json!({"unix_time": 1_000})),
            },
            RingEvent {
                tag: 2,
                name: "green_ibi_quality_event",
                timestamp: 200,
                body: vec![2],
                decoded: Some(serde_json::json!({"hr_bpm": [60, 80]})),
            },
            RingEvent {
                tag: 3,
                name: "temp_event",
                timestamp: 201,
                body: vec![3],
                decoded: Some(serde_json::json!({"temps_c": [30.0, 31.0, 25.0]})),
            },
            RingEvent {
                tag: 4,
                name: "motion_event",
                timestamp: 202,
                body: vec![4],
                decoded: Some(serde_json::json!({"motion_seconds": 35})),
            },
            RingEvent {
                tag: 5,
                name: "activity_information",
                timestamp: 203,
                body: vec![5],
                decoded: Some(serde_json::json!({"met": [1.0, 2.0]})),
            },
            RingEvent {
                tag: 6,
                name: "spo2_event",
                timestamp: 204,
                body: vec![6],
                decoded: Some(serde_json::json!({"spo2_percent": [97, 98]})),
            },
        ];
        let (sender, receiver) = mpsc::unbounded_channel();
        for event in events {
            sender.send(BatchMessage::Event(event)).unwrap();
        }
        sender.send(BatchMessage::Commit(205)).unwrap();
        drop(sender);
        write_batches(store.database(), "S1".into(), 0, receiver)
            .await
            .unwrap();

        let today = store.today(1_000, 2_000).await.unwrap();
        assert_eq!(today.heart_rate_median, Some(70.0));
        assert_eq!(today.heart_rate_samples, 2);
        assert_eq!(today.temperature_records, 1);
        assert_eq!(today.motion_periods, 1);
        assert_eq!(today.motion_seconds, 35);
        assert_eq!(today.activity_met_average, Some(1.5));
        assert_eq!(today.spo2_percent_samples, 2);
    }

    #[tokio::test]
    async fn low_battery_notification_is_sent_once_until_recharged() {
        let temp = tempdir().unwrap();
        let store = Store::open(&temp.path().join("oura.db")).await.unwrap();

        assert!(
            store
                .low_battery_notification_due("S1", 9, 10)
                .await
                .unwrap()
        );
        store.mark_low_battery_notified("S1", 9).await.unwrap();
        assert!(
            !store
                .low_battery_notification_due("S1", 8, 10)
                .await
                .unwrap()
        );
        assert!(
            !store
                .low_battery_notification_due("S1", 10, 10)
                .await
                .unwrap()
        );
        assert!(
            store
                .low_battery_notification_due("S1", 9, 10)
                .await
                .unwrap()
        );
        assert!(
            !store
                .low_battery_notification_due("S1", 1, 0)
                .await
                .unwrap()
        );
    }
}
