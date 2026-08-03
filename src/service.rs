use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{AppConfig, AppPaths};
use crate::store::Store;

#[cfg(target_os = "linux")]
#[path = "service/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "service/macos.rs"]
mod platform;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("oura currently supports macOS and Linux");

pub async fn install(
    paths: &AppPaths,
    config: &mut AppConfig,
    key_file: Option<&Path>,
    interval: u16,
) -> Result<()> {
    validate_interval(interval)?;
    paths.ensure_layout()?;

    if let Some(source) = key_file {
        config.configure(paths, Some(source), None, None, None, None)?;
    } else {
        config.save(paths)?;
    }
    Store::open(&config.database_path).await?;

    install_binary(paths)?;
    let service_path = platform::install(paths, interval)?;
    remove_legacy_binary(paths)?;

    println!("Installed binary : {}", paths.installed_binary.display());
    println!("Database         : {}", config.database_path.display());
    println!("Configuration    : {}", paths.config.display());
    println!("Background job   : {}", service_path.display());
    println!("Schedule         : every {interval} minutes while logged in");
    if config.low_battery_threshold_percent == 0 {
        println!("Battery alerts   : off");
    } else {
        println!(
            "Battery alerts   : below {}%",
            config.low_battery_threshold_percent
        );
    }
    if config.load_key().is_ok() {
        println!("Auth key         : configured");
        println!(
            "Next: {} doctor --connect",
            paths.installed_binary.display()
        );
    } else {
        println!("Auth key         : not configured");
        println!(
            "Next: {} configure --key-file /path/to/ring.key",
            paths.installed_binary.display()
        );
    }
    print_path_hint(paths);
    Ok(())
}

pub fn uninstall(paths: &AppPaths) -> Result<()> {
    platform::uninstall()?;
    if paths.installed_binary.exists() {
        fs::remove_file(&paths.installed_binary)
            .with_context(|| format!("removing {}", paths.installed_binary.display()))?;
    }
    println!("Removed the background service and installed binary.");
    println!("Personal data was preserved in {}", paths.root.display());
    Ok(())
}

pub fn start() -> Result<()> {
    platform::start()
}

pub fn stop() -> Result<()> {
    platform::stop()
}

pub fn restart() -> Result<()> {
    platform::restart()
}

pub fn status() -> Result<()> {
    platform::status()
}

pub fn status_text() -> &'static str {
    platform::status_text()
}

pub fn show_logs(paths: &AppPaths, lines: usize) -> Result<()> {
    platform::show_logs(paths, lines)
}

fn install_binary(paths: &AppPaths) -> Result<()> {
    let source = std::env::current_exe().context("finding the current executable")?;
    let parent = paths
        .installed_binary
        .parent()
        .ok_or_else(|| anyhow!("installed binary path has no parent directory"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755))?;
    let same_file = paths.installed_binary.exists()
        && source.canonicalize().ok() == paths.installed_binary.canonicalize().ok();
    if !same_file {
        fs::copy(&source, &paths.installed_binary).with_context(|| {
            format!(
                "copying {} to {}",
                source.display(),
                paths.installed_binary.display()
            )
        })?;
    }
    fs::set_permissions(&paths.installed_binary, fs::Permissions::from_mode(0o755))?;

    #[cfg(target_os = "macos")]
    platform::sign_binary(&paths.installed_binary)?;
    Ok(())
}

fn remove_legacy_binary(paths: &AppPaths) -> Result<()> {
    let legacy_dir = paths.root.join("bin");
    let legacy_binary = legacy_dir.join("oura");
    if legacy_binary.exists() || legacy_binary.is_symlink() {
        fs::remove_file(&legacy_binary)
            .with_context(|| format!("removing legacy binary {}", legacy_binary.display()))?;
    }
    if legacy_dir.exists() {
        match fs::remove_dir(&legacy_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("removing legacy directory {}", legacy_dir.display())
                });
            }
        }
    }
    Ok(())
}

fn print_path_hint(paths: &AppPaths) {
    let Some(parent) = paths.installed_binary.parent() else {
        return;
    };
    let in_path = std::env::var_os("PATH")
        .is_some_and(|value| std::env::split_paths(&value).any(|entry| entry == parent));
    if !in_path {
        println!();
        println!(
            "Add {} to PATH to run oura from any directory:",
            parent.display()
        );
        println!("  export PATH=\"{}:$PATH\"", parent.display());
    }
}

fn validate_interval(interval: u16) -> Result<()> {
    if interval == 0 || interval > 60 || 60 % interval != 0 {
        bail!("interval must be one of 1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30, or 60 minutes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_divisor_interval() {
        assert!(validate_interval(7).is_err());
        assert!(validate_interval(0).is_err());
    }
}
