use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;

use crate::config::{AppPaths, write_private};

const SERVICE: &str = "oura-cli-sync.service";
const TIMER: &str = "oura-cli-sync.timer";
const LEGACY_UNITS: &[(&str, &str)] = &[("oura-sync.service", "oura-sync.timer")];

pub fn install(paths: &AppPaths, interval: u16) -> Result<PathBuf> {
    remove_legacy_units()?;
    let directory = unit_directory()?;
    fs::create_dir_all(&directory)?;
    let service_path = directory.join(SERVICE);
    let timer_path = directory.join(TIMER);
    write_private(&service_path, render_service(paths).as_bytes())?;
    write_private(&timer_path, render_timer(interval).as_bytes())?;
    checked_systemctl(&["--user", "daemon-reload"])?;
    checked_systemctl(&["--user", "enable", "--now", TIMER])?;
    Ok(timer_path)
}

pub fn uninstall() -> Result<()> {
    let directory = unit_directory()?;
    remove_unit_pair(&directory, SERVICE, TIMER)?;
    for (service, timer) in LEGACY_UNITS {
        remove_unit_pair(&directory, service, timer)?;
    }
    checked_systemctl(&["--user", "daemon-reload"])
}

pub fn start() -> Result<()> {
    checked_systemctl(&["--user", "enable", "--now", TIMER])?;
    checked_systemctl(&["--user", "start", SERVICE])?;
    println!("Background sync is on; a sync has started.");
    Ok(())
}

pub fn stop() -> Result<()> {
    checked_systemctl(&["--user", "disable", "--now", TIMER])?;
    let _ = systemctl(&["--user", "stop", SERVICE]);
    println!("Background sync is off.");
    Ok(())
}

pub fn restart() -> Result<()> {
    checked_systemctl(&["--user", "enable", "--now", TIMER])?;
    checked_systemctl(&["--user", "restart", SERVICE])?;
    println!("Background sync restarted.");
    Ok(())
}

pub fn status() -> Result<()> {
    let output = systemctl(&["--user", "status", TIMER, "--no-pager"])?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        bail!("Oura systemd user timer is not installed or active")
    }
}

pub fn status_text() -> &'static str {
    match systemctl(&["--user", "is-active", TIMER]) {
        Ok(output) if output.status.success() => "loaded",
        _ => "not loaded",
    }
}

pub fn show_logs(_paths: &AppPaths, lines: usize) -> Result<()> {
    let output = Command::new("journalctl")
        .args(["--user", "--unit", SERVICE, "--no-pager", "--lines"])
        .arg(lines.to_string())
        .output()
        .context("running journalctl")?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "journalctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn render_service(paths: &AppPaths) -> String {
    format!(
        "[Unit]\nDescription=Sync an Oura Ring to a private local database\n\n\
         [Service]\nType=oneshot\nExecStart={} --home {} sync --background\n\
         WorkingDirectory={}\nNice=10\nUMask=0077\n",
        systemd_quote(&paths.installed_binary.to_string_lossy()),
        systemd_quote(&paths.root.to_string_lossy()),
        systemd_quote(&paths.root.to_string_lossy()),
    )
}

fn render_timer(interval: u16) -> String {
    format!(
        "[Unit]\nDescription=Periodically sync an Oura Ring\n\n\
         [Timer]\nOnBootSec=2min\nOnUnitActiveSec={interval}min\nPersistent=true\nUnit={SERVICE}\n\n\
         [Install]\nWantedBy=timers.target\n"
    )
}

fn remove_legacy_units() -> Result<()> {
    let directory = unit_directory()?;
    for (service, timer) in LEGACY_UNITS {
        remove_unit_pair(&directory, service, timer)?;
    }
    Ok(())
}

fn remove_unit_pair(directory: &std::path::Path, service: &str, timer: &str) -> Result<()> {
    let _ = systemctl(&["--user", "disable", "--now", timer]);
    let _ = systemctl(&["--user", "stop", service]);
    for name in [service, timer] {
        let path = directory.join(name);
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn unit_directory() -> Result<PathBuf> {
    Ok(BaseDirs::new()
        .ok_or_else(|| anyhow!("could not determine the user configuration directory"))?
        .config_dir()
        .join("systemd/user"))
}

fn systemctl(arguments: &[&str]) -> Result<Output> {
    Command::new("systemctl")
        .args(arguments)
        .output()
        .context("running systemctl")
}

fn checked_systemctl(arguments: &[&str]) -> Result<()> {
    let output = systemctl(arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "systemctl failed: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn units_quote_paths_and_include_interval() {
        let paths = AppPaths::resolve(Some(Path::new("/tmp/Oura Ring"))).unwrap();
        assert!(render_service(&paths).contains("\"/tmp/Oura Ring\""));
        assert!(render_timer(30).contains("OnUnitActiveSec=30min"));
        assert!(render_timer(30).contains("Unit=oura-cli-sync.service"));
        assert!(!render_timer(30).contains("Unit=oura-sync.service"));
    }
}
