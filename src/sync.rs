use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use oura_link::OuraClient;
use oura_link::ble::BleTransport;
use tokio::sync::mpsc;

use crate::config::{AppConfig, AppPaths};
use crate::notification;
use crate::store::{BatchMessage, Store, WriteSummary, write_batches};

pub async fn run(
    paths: &AppPaths,
    config: &AppConfig,
    sync_time: bool,
    background: bool,
) -> Result<()> {
    paths.ensure_layout()?;
    let _lock = match acquire_lock(paths) {
        Ok(lock) => lock,
        Err(error) if background => {
            println!("Sync skipped: {error}");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let key = match config.load_key() {
        Ok(key) => key,
        Err(error) if background => {
            println!("Sync skipped: auth key is not configured ({error})");
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let store = Store::open(&config.database_path).await?;
    let transport = match BleTransport::connect(
        &config.name,
        config.address.as_deref(),
        Duration::from_secs(config.scan_timeout_seconds),
    )
    .await
    {
        Ok(transport) => transport,
        Err(error) if background => {
            println!("Sync skipped: ring unavailable ({error})");
            return Ok(());
        }
        Err(error) => return Err(error).context("connecting to ring"),
    };
    let client = OuraClient::new(transport);

    let result = sync_connected(&client, &store, config, &key, sync_time).await;
    let _ = client.transport().disconnect().await;
    result
}

async fn sync_connected(
    client: &OuraClient<BleTransport>,
    store: &Store,
    config: &AppConfig,
    key: &[u8; 16],
    sync_time: bool,
) -> Result<()> {
    client.authenticate(key).await.context("authenticating")?;
    if sync_time {
        client.sync_time().await.context("syncing ring time")?;
    }

    let serial = client.serial().await.context("reading ring serial")?;
    let hardware = client.hardware_id().await.ok();
    let info = client.firmware().await.ok();
    store
        .upsert_device(
            &serial,
            hardware.as_deref(),
            info.as_ref().map(|value| value.firmware_version.as_str()),
            info.as_ref().map(|value| value.api_version.as_str()),
            info.as_ref().map(|value| value.mac.as_str()),
        )
        .await?;

    if let Ok(battery) = client.battery().await {
        store
            .insert_reading(&serial, "battery_percent", battery.percent as f64, "%")
            .await?;
        if store
            .low_battery_notification_due(
                &serial,
                battery.percent,
                config.low_battery_threshold_percent,
            )
            .await?
        {
            match notification::low_battery(battery.percent) {
                Ok(()) => {
                    store
                        .mark_low_battery_notified(&serial, battery.percent)
                        .await?;
                    println!("Low-battery notification sent ({}%).", battery.percent);
                }
                Err(error) => tracing::warn!("could not send low-battery notification: {error:#}"),
            }
        }
    }

    let cursor = store.cursor(&serial).await?;
    let run_id = store.begin_run(&serial, cursor).await?;
    println!("Syncing {serial} from cursor {cursor} ...");

    let (sender, receiver) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write_batches(
        store.database(),
        serial.clone(),
        cursor,
        receiver,
    ));
    let event_sender = sender.clone();
    let cursor_sender = sender.clone();
    let drain_result = client
        .drain_events(
            cursor,
            move |event| {
                let _ = event_sender.send(BatchMessage::Event(event.clone()));
            },
            move |next_cursor| {
                let _ = cursor_sender.send(BatchMessage::Commit(next_cursor));
            },
        )
        .await;
    drop(sender);

    let writer_result = writer.await.context("event writer task failed")?;
    match (drain_result, writer_result) {
        (Ok(outcome), Ok(mut summary)) => {
            // A fully drained ring with no events does not emit a batch commit.
            summary.end_cursor = summary.end_cursor.max(outcome.next_cursor);
            store.finish_run(&run_id, &summary, "success", None).await?;
            println!(
                "Done: {} received, {} inserted, cursor {}.",
                summary.received, summary.inserted, summary.end_cursor
            );
            Ok(())
        }
        (drain, writer) => {
            let persisted_cursor = store.cursor(&serial).await.unwrap_or(cursor);
            let (summary, writer_error) = match writer {
                Ok(summary) => (summary, None),
                Err(error) => (
                    WriteSummary {
                        end_cursor: persisted_cursor,
                        ..WriteSummary::default()
                    },
                    Some(error.to_string()),
                ),
            };
            let mut errors = Vec::new();
            if let Err(error) = drain {
                errors.push(format!("ring sync failed: {error}"));
            }
            if let Some(error) = writer_error {
                errors.push(format!("database writer failed: {error}"));
            }
            let error = errors.join("; ");
            store
                .finish_run(&run_id, &summary, "failed", Some(&error))
                .await?;
            Err(anyhow!(error))
        }
    }
}

pub(crate) fn acquire_lock(paths: &AppPaths) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .truncate(false)
        .open(paths.lock_file())?;
    file.try_lock_exclusive()
        .map_err(|_| anyhow!("another sync is already running"))?;
    Ok(file)
}
