use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn is_supported() -> bool {
    cfg!(target_os = "macos") || (cfg!(target_os = "linux") && command_exists("notify-send"))
}

pub fn backend_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Notification Center"
    } else if cfg!(target_os = "linux") && command_exists("notify-send") {
        "notify-send"
    } else {
        "unavailable"
    }
}

#[cfg(target_os = "macos")]
pub fn low_battery(percent: u8) -> Result<()> {
    let message = format!("Your Oura Ring battery is at {percent}%. Charge it soon.");
    let output = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"Oura CLI\"",
            "-e",
            "end run",
            "--",
        ])
        .arg(&message)
        .output()
        .context("running the macOS notification service")?;
    if !output.status.success() {
        bail!(
            "macOS notification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn low_battery(percent: u8) -> Result<()> {
    let message = format!("Your Oura Ring battery is at {percent}%. Charge it soon.");
    let output = Command::new("notify-send")
        .args(["--app-name=Oura CLI", "Oura Ring battery low", &message])
        .output()
        .context("running notify-send")?;
    if !output.status.success() {
        bail!(
            "notify-send failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn low_battery(_percent: u8) -> Result<()> {
    bail!("desktop notifications are not supported on this platform")
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_has_a_builtin_notification_backend() {
        #[cfg(target_os = "macos")]
        assert!(is_supported());
    }
}
