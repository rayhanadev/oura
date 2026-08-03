use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "oura",
    version,
    about = "Sync an Oura Ring into a private local Turso database"
)]
pub struct Cli {
    /// Override the application home (default: ~/.oura).
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,

    /// Override the local Turso database path.
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Override the ring auth-key file.
    #[arg(long, global = true)]
    pub key_file: Option<PathBuf>,

    /// Override the Bluetooth name filter.
    #[arg(long, global = true)]
    pub name: Option<String>,

    /// Connect only to this platform-specific Bluetooth device ID.
    #[arg(long, global = true)]
    pub address: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Install the binary and periodic per-user background service.
    Install {
        /// Copy and configure an existing 16-byte ring auth key.
        #[arg(long)]
        key_file: Option<PathBuf>,

        /// Sync interval in minutes; must divide evenly into 60.
        #[arg(long, default_value_t = 30)]
        interval: u16,
    },

    /// Remove the background service and binary; personal data is preserved.
    Uninstall,

    /// Save ring selection and auth-key configuration.
    Configure {
        /// Copy an existing 16-byte ring auth key into private storage.
        #[arg(long)]
        key_file: Option<PathBuf>,
        /// Bluetooth advertisement name to match.
        #[arg(long)]
        name: Option<String>,
        /// Save one platform-specific Bluetooth device ID.
        #[arg(long)]
        address: Option<String>,
        /// Seconds to wait for an intermittent ring advertisement.
        #[arg(long)]
        scan_timeout: Option<u64>,

        /// Notify below this battery percentage; 0 disables notifications.
        #[arg(long, value_name = "PERCENT")]
        battery_threshold: Option<u8>,
    },

    /// List nearby Oura rings.
    Scan,

    /// Pair with a factory-reset ring and enable normal measurements.
    Pair {
        /// Confirm that the ring has been factory-reset and may receive a new auth key.
        #[arg(long)]
        confirm_reset: bool,
    },

    /// Show cached ring identity, firmware, and battery.
    Info {
        /// Attempt a live Bluetooth refresh before displaying information.
        #[arg(long)]
        refresh: bool,
        /// Include low-level protocol and hardware details.
        #[arg(long)]
        verbose: bool,
    },

    /// Incrementally drain ring history into the local database.
    Sync {
        /// Align ring time to host UTC before syncing (state-changing).
        #[arg(long)]
        sync_time: bool,

        /// Treat missing configuration/ring as a scheduled skip.
        #[arg(long, hide = true)]
        background: bool,
    },

    /// Show a short cached sync overview without connecting to the ring.
    Status {
        /// Include storage and record-count details.
        #[arg(long)]
        verbose: bool,
    },

    /// Validate local configuration and optionally connect to the ring.
    Doctor {
        #[arg(long)]
        connect: bool,
    },

    /// Print recent background-agent logs.
    Logs {
        #[arg(long, default_value_t = 100)]
        lines: usize,
    },

    /// Inspect or test low-battery notifications.
    Notifications {
        #[command(subcommand)]
        command: NotificationCommand,
    },

    /// Control the installed background service.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum NotificationCommand {
    /// Show the configured threshold and desktop backend.
    Status,
    /// Send a test desktop notification.
    Test,
}

#[derive(Subcommand, Debug)]
pub enum ServiceCommand {
    /// Enable background sync and run once now.
    Start,
    /// Disable background sync.
    Stop,
    /// Reload background sync and run once now.
    Restart,
    /// Show raw background-service state.
    Status,
}
