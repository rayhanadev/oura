mod cli;
mod config;
mod notification;
mod service;
mod store;
mod sync;

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Local, TimeZone, Utc};
use clap::Parser;
use oura_link::OuraClient;
use oura_link::ble::{self, BleTransport};

use crate::cli::{Cli, Command, NotificationCommand, ServiceCommand};
use crate::config::{AppConfig, AppPaths, write_private};
use crate::store::Store;

#[tokio::main]
async fn main() -> Result<()> {
    // Health data, logs, config, and Turso sidecar files should never inherit a
    // permissive interactive-shell umask.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,oura=info,btleplug=off".into()),
        )
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();
    let paths = AppPaths::resolve(cli.home.as_deref())?;
    let mut config = AppConfig::load_or_default(&paths)?;
    config.apply_overrides(&cli);

    match cli.command {
        Command::Install { key_file, interval } => {
            service::install(&paths, &mut config, key_file.as_deref(), interval).await
        }
        Command::Uninstall => service::uninstall(&paths),
        Command::Configure {
            key_file,
            name,
            address,
            scan_timeout,
            battery_threshold,
        } => {
            config.configure(
                &paths,
                key_file.as_deref(),
                name,
                address,
                scan_timeout,
                battery_threshold,
            )?;
            println!("Configuration saved to {}", paths.config.display());
            Ok(())
        }
        Command::Scan => cmd_scan(&config).await,
        Command::Pair { confirm_reset } => cmd_pair(&paths, &mut config, confirm_reset).await,
        Command::Info { refresh, verbose } => cmd_info(&paths, &config, refresh, verbose).await,
        Command::Sync {
            sync_time,
            background,
        } => sync::run(&paths, &config, sync_time, background).await,
        Command::Status { verbose } => cmd_status(&paths, &config, verbose).await,
        Command::Doctor { connect } => cmd_doctor(&paths, &config, connect).await,
        Command::Logs { lines } => service::show_logs(&paths, lines),
        Command::Notifications { command } => match command {
            NotificationCommand::Status => {
                if config.low_battery_threshold_percent == 0 {
                    println!("Battery notifications are off.");
                } else {
                    println!(
                        "Battery notifications are on below {}%.",
                        config.low_battery_threshold_percent
                    );
                }
                println!("Backend: {}", notification::backend_name());
                Ok(())
            }
            NotificationCommand::Test => {
                notification::low_battery(9)?;
                println!("Test notification sent.");
                Ok(())
            }
        },
        Command::Service { command } => match command {
            ServiceCommand::Start => service::start(),
            ServiceCommand::Stop => service::stop(),
            ServiceCommand::Restart => service::restart(),
            ServiceCommand::Status => service::status(),
        },
    }
}

async fn cmd_scan(config: &AppConfig) -> Result<()> {
    let rings = ble::scan(
        &config.name,
        Duration::from_secs(config.scan_timeout_seconds),
    )
    .await
    .context("scanning for Oura rings")?;

    if rings.is_empty() {
        println!("No matching Oura rings found.");
    } else {
        for ring in rings {
            println!("{:>5} dBm  {}  ({})", ring.rssi, ring.name, ring.id);
        }
    }
    Ok(())
}

async fn cmd_pair(paths: &AppPaths, config: &mut AppConfig, confirm_reset: bool) -> Result<()> {
    use oura_protocol::protocol::{feature, feature_mode};

    if !confirm_reset {
        bail!(
            "pairing installs a new auth key; factory-reset the ring, then rerun with --confirm-reset"
        );
    }

    paths.ensure_layout()?;
    let _lock = sync::acquire_lock(paths)?;

    if config.address.is_none() {
        let rings = ble::scan(
            &config.name,
            Duration::from_secs(config.scan_timeout_seconds),
        )
        .await
        .context("scanning for the factory-reset ring")?;
        match rings.as_slice() {
            [] => bail!("no factory-reset Oura ring found"),
            [ring] => {
                config.address = Some(ring.id.clone());
                println!("Selected {} ({})", ring.name, ring.id);
            }
            _ => bail!(
                "multiple Oura rings found; rerun with --address followed by the intended device ID"
            ),
        }
    }

    let (key, generated) = if config.key_path.exists() {
        (config.load_key()?, false)
    } else {
        let key = generate_auth_key()?;
        write_private(
            &config.key_path,
            format!("{}\n", hex::encode(key)).as_bytes(),
        )?;
        (key, true)
    };

    // Persist both the key and selected device before installing the key on the
    // ring. A retry can then authenticate even if the process stops mid-pair.
    config.save(paths)?;

    let client = connect(config).await?;
    let result = async {
        let serial = client.serial().await.unwrap_or_else(|_| "unknown".into());
        if client.authenticate(&key).await.is_ok() {
            println!("The saved auth key is already active on {serial}.");
        } else {
            client.set_auth_key(&key).await.context(
                "installing the auth key (the ring must be factory-reset and disconnected from the Oura app)",
            )?;
            client
                .authenticate(&key)
                .await
                .context("verifying the new auth key")?;
            println!("Paired with {serial} using the saved auth key.");
        }

        client.sync_time().await.context("setting ring time")?;
        client
            .set_feature_mode(feature::DAYTIME_HR, feature_mode::AUTOMATIC)
            .await
            .context("enabling daytime heart-rate measurements")?;
        client
            .set_feature_mode(feature::SPO2, feature_mode::AUTOMATIC)
            .await
            .context("enabling SpO2 measurements")?;

        println!("Enabled automatic daytime HR and SpO2 measurements.");
        if let Ok(battery) = client.battery().await {
            println!("Battery: {}%", battery.percent);
        }
        println!("Auth key: {}", config.key_path.display());
        if generated {
            println!("A new auth key was generated and stored before pairing.");
        }
        Ok(())
    }
    .await;
    let _ = client.transport().disconnect().await;
    result
}

fn generate_auth_key() -> Result<[u8; 16]> {
    let mut random = std::fs::File::open("/dev/urandom").context("opening OS random source")?;
    let mut key = [0_u8; 16];
    random.read_exact(&mut key).context("generating auth key")?;
    Ok(key)
}

async fn connect(config: &AppConfig) -> Result<OuraClient<BleTransport>> {
    let transport = BleTransport::connect(
        &config.name,
        config.address.as_deref(),
        Duration::from_secs(config.scan_timeout_seconds),
    )
    .await
    .with_context(|| {
        format!(
            "connecting to Oura ring (waited up to {} seconds; keep it close to the Mac and disconnected from the official app)",
            config.scan_timeout_seconds
        )
    })?;
    Ok(OuraClient::new(transport))
}

#[derive(Debug)]
struct LiveDetails {
    bt_stack: String,
    capabilities: Option<String>,
}

async fn refresh_ring_info(config: &AppConfig, store: &Store) -> Result<LiveDetails> {
    let client = connect(config).await?;
    let result = async {
        let info = client.firmware().await.context("reading firmware")?;
        let serial = client.serial().await.context("reading serial")?;
        let hardware = client.hardware_id().await.ok();
        store
            .upsert_device(
                &serial,
                hardware.as_deref(),
                Some(&info.firmware_version),
                Some(&info.api_version),
                Some(&info.mac),
            )
            .await?;

        if let Ok(key) = config.load_key() {
            client.authenticate(&key).await.context("authenticating")?;
            if let Ok(battery) = client.battery().await {
                store
                    .insert_reading(&serial, "battery_percent", battery.percent as f64, "%")
                    .await?;
            }
        }

        let capabilities = client.capabilities().await.ok().and_then(|caps| {
            (!caps.is_empty()).then(|| {
                caps.iter()
                    .map(|cap| format!("{}:{}", cap.feature, cap.value))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        });
        Ok(LiveDetails {
            bt_stack: info.bt_stack_version,
            capabilities,
        })
    }
    .await;

    let _ = client.transport().disconnect().await;
    result
}

async fn cmd_info(
    paths: &AppPaths,
    config: &AppConfig,
    refresh: bool,
    verbose: bool,
) -> Result<()> {
    paths.ensure_layout()?;
    let store = Store::open(&config.database_path).await?;
    let (live, refresh_error) = if refresh {
        match refresh_ring_info(config, &store).await {
            Ok(details) => (Some(details), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    let status = store.status().await?;
    let device = status.device.as_ref().ok_or_else(|| {
        anyhow!("no cached ring information; run `oura sync` or `oura info --refresh`")
    })?;

    println!("{}", ring_model(device.hardware_id.as_deref()));
    println!("Serial   : {}", device.serial);
    if let Some(firmware) = &device.firmware {
        println!("Firmware : {firmware}");
    }
    if let Some(battery) = &status.latest_battery {
        println!(
            "Battery  : {:.0}{} ({})",
            battery.value,
            battery.unit,
            format_age(battery.captured_unix)
        );
    }
    println!("Updated  : {}", format_age(device.updated_unix));

    if verbose {
        if let Some(hardware) = &device.hardware_id {
            println!("Hardware : {hardware}");
        }
        if let Some(api) = &device.api_version {
            println!("API      : {api}");
        }
        if let Some(mac) = &device.mac {
            println!("MAC      : {mac}");
        }
        if let Some(live) = &live {
            println!("BT stack : {}", live.bt_stack);
            if let Some(capabilities) = &live.capabilities {
                println!("Caps     : {capabilities}");
            }
        }
    }

    if let Some(error) = refresh_error {
        eprintln!("Live refresh unavailable; showing cached information.");
        eprintln!("Reason: {error:#}");
    }
    Ok(())
}

async fn cmd_status(paths: &AppPaths, config: &AppConfig, verbose: bool) -> Result<()> {
    paths.ensure_layout()?;
    let store = Store::open(&config.database_path).await?;
    let status = store.status().await?;
    let (today_start, tomorrow_start) = local_day_bounds()?;
    let today = store.today(today_start, tomorrow_start).await?;

    let model = status
        .device
        .as_ref()
        .map(|device| ring_model(device.hardware_id.as_deref()))
        .unwrap_or("Oura Ring");
    println!("Ring       : {model}");
    if let Some(battery) = &status.latest_battery {
        println!(
            "Battery    : {:.0}{} ({})",
            battery.value,
            battery.unit,
            format_age(battery.captured_unix)
        );
    } else {
        println!("Battery    : unknown");
    }

    if let Some(run) = &status.last_run {
        let synced_at = run.finished_unix.unwrap_or(run.started_unix);
        println!(
            "Last sync  : {} ({})",
            status_label(&run.status),
            format_age(synced_at)
        );
        if let Some(error) = &run.error {
            println!("Problem    : {error}");
        }
    } else {
        println!("Last sync  : never");
    }

    println!(
        "Background : {}",
        if service::status_text() == "loaded" {
            "on"
        } else {
            "off"
        }
    );

    let diagnostic_count: i64 = status
        .event_counts
        .iter()
        .filter(|entry| is_diagnostic_event(&entry.name))
        .map(|entry| entry.count)
        .sum();
    println!();
    println!("Today");
    match today.heart_rate_median {
        Some(median) => println!(
            "  Heart rate : {median:.0} bpm median · {} samples",
            today.heart_rate_samples
        ),
        None => println!("  Heart rate : no samples yet"),
    }
    println!(
        "  Temperature: {} sensor record{}",
        today.temperature_records,
        if today.temperature_records == 1 {
            ""
        } else {
            "s"
        }
    );
    if today.motion_periods == 0 {
        println!("  Movement   : no samples yet");
    } else {
        println!(
            "  Movement   : {} · {} period{}",
            format_motion_time(today.motion_seconds),
            today.motion_periods,
            if today.motion_periods == 1 { "" } else { "s" }
        );
    }
    if let Some(average) = today.activity_met_average {
        println!(
            "  Activity   : {average:.1} MET average · {} samples",
            today.activity_samples
        );
    }
    if today.spo2_percent_samples > 0 {
        println!("  SpO₂       : {} samples", today.spo2_percent_samples);
    } else if today.spo2_signal_records > 0 {
        println!(
            "  SpO₂       : {} signal record{}",
            today.spo2_signal_records,
            if today.spo2_signal_records == 1 {
                ""
            } else {
                "s"
            }
        );
    } else {
        println!("  SpO₂       : no samples yet");
    }

    if verbose {
        println!();
        if let Some(device) = &status.device {
            println!("Serial      : {}", device.serial);
        }
        if let Some(run) = &status.last_run {
            println!("Last added  : {} records", run.inserted);
        }
        println!("Stored      : {} records", status.event_count);
        println!("Diagnostics : {diagnostic_count} records");
        println!("Database    : {}", config.database_path.display());
    }
    Ok(())
}

fn format_age(timestamp: i64) -> String {
    let seconds = Utc::now().timestamp() - timestamp;
    match seconds {
        ..=-1 => format_local_time(timestamp),
        0..=59 => "just now".to_owned(),
        60..=119 => "1 minute ago".to_owned(),
        120..=3_599 => format!("{} minutes ago", seconds / 60),
        3_600..=7_199 => "1 hour ago".to_owned(),
        7_200..=86_399 => format!("{} hours ago", seconds / 3_600),
        86_400..=172_799 => "1 day ago".to_owned(),
        172_800..=604_799 => format!("{} days ago", seconds / 86_400),
        _ => format_local_time(timestamp),
    }
}

fn local_day_bounds() -> Result<(i64, i64)> {
    let today = Local::now().date_naive();
    let tomorrow = today
        .succ_opt()
        .ok_or_else(|| anyhow!("could not determine tomorrow's date"))?;
    let start = today
        .and_hms_opt(0, 0, 0)
        .and_then(|value| value.and_local_timezone(Local).earliest())
        .ok_or_else(|| anyhow!("could not determine local start of day"))?;
    let end = tomorrow
        .and_hms_opt(0, 0, 0)
        .and_then(|value| value.and_local_timezone(Local).earliest())
        .ok_or_else(|| anyhow!("could not determine local end of day"))?;
    Ok((start.timestamp(), end.timestamp()))
}

fn format_motion_time(seconds: u64) -> String {
    match seconds {
        0 => "movement recorded".to_owned(),
        1..=59 => "<1 min detected".to_owned(),
        _ => format!("{} min detected", (seconds + 30) / 60),
    }
}

fn format_local_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|| "unknown time".to_owned())
}

fn ring_model(hardware_id: Option<&str>) -> &'static str {
    match hardware_id {
        Some(value) if value.starts_with("ORE_") || value.starts_with("JAD_") => "Oura Ring 4",
        _ => "Oura Ring",
    }
}

fn status_label(status: &str) -> &str {
    match status {
        "success" => "successful",
        "failed" => "failed",
        "running" => "running",
        other => other,
    }
}

fn is_diagnostic_event(name: &str) -> bool {
    matches!(
        name,
        "debug_event"
            | "debug_data"
            | "ble_connection"
            | "time_sync"
            | "ring_start"
            | "self_test_data_event"
            | "state_change"
            | "feature_session"
            | "wear_event"
            | "scan_start"
            | "scan_end"
    )
}

async fn cmd_doctor(paths: &AppPaths, config: &AppConfig, connect_ring: bool) -> Result<()> {
    paths.ensure_layout()?;
    let mut failures = 0_u8;
    println!("Oura diagnostics");
    println!("✓ Configuration");

    let key = match config.load_key() {
        Ok(key) => {
            println!("✓ Auth key");
            Some(key)
        }
        Err(error) => {
            println!("✗ Auth key — {error}");
            failures += 1;
            None
        }
    };

    match Store::open(&config.database_path).await {
        Ok(_) => println!("✓ Local database"),
        Err(error) => {
            println!("✗ Local database — {error:#}");
            failures += 1;
        }
    }

    if service::status_text() == "loaded" {
        println!("✓ Background sync");
    } else {
        println!("✗ Background sync — the user service is not loaded");
        failures += 1;
    }

    if config.low_battery_threshold_percent == 0 {
        println!("○ Battery notifications disabled");
    } else if notification::is_supported() {
        println!(
            "✓ Battery notifications — below {}%",
            config.low_battery_threshold_percent
        );
    } else {
        println!("✗ Battery notifications — no supported desktop notifier found");
        failures += 1;
    }

    if connect_ring {
        if let Some(key) = key {
            match connect(config).await {
                Ok(client) => {
                    let check = async {
                        client.authenticate(&key).await.context("authenticating")?;
                        client.battery().await.context("reading battery")
                    }
                    .await;
                    let _ = client.transport().disconnect().await;
                    match check {
                        Ok(battery) => println!("✓ Ring connection — battery {}%", battery.percent),
                        Err(error) => {
                            println!("✗ Ring connection — {error:#}");
                            failures += 1;
                        }
                    }
                }
                Err(error) => {
                    println!("✗ Ring connection — {error:#}");
                    failures += 1;
                }
            }
        } else {
            println!("✗ Ring connection — auth key is unavailable");
        }
    } else {
        println!("○ Ring connection not tested (use `oura doctor --connect`)");
    }

    if failures > 0 {
        bail!("{failures} diagnostic check(s) failed");
    }
    Ok(())
}

pub(crate) fn load_key_file(path: &std::path::Path) -> Result<[u8; 16]> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading auth key {}", path.display()))?;
    let bytes = hex::decode(text.trim()).context("auth key is not valid hexadecimal")?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("auth key must be exactly 16 bytes (32 hex characters)"))
}

pub(crate) fn absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
