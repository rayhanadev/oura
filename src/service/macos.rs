use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;

use crate::config::{AppPaths, write_private};

const LABEL: &str = "com.rayhanadev.oura-cli-sync";
const LEGACY_LABELS: &[&str] = &["dev.oura.sync"];

pub fn install(paths: &AppPaths, interval: u16) -> Result<PathBuf> {
    remove_legacy_agents()?;
    let agent_path = launch_agent_path()?;
    if let Some(parent) = agent_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(&agent_path, render_plist(paths, interval).as_bytes())?;
    validate_plist(&agent_path)?;
    load(&agent_path)?;
    Ok(agent_path)
}

pub fn uninstall() -> Result<()> {
    remove_agent(LABEL)?;
    remove_legacy_agents()
}

fn remove_legacy_agents() -> Result<()> {
    for label in LEGACY_LABELS {
        remove_agent(label)?;
    }
    Ok(())
}

fn remove_agent(label: &str) -> Result<()> {
    let _ = launchctl(&["bootout", &service_target_for(label)?]);
    let agent_path = launch_agent_path_for(label)?;
    if agent_path.exists() {
        fs::remove_file(&agent_path)
            .with_context(|| format!("removing {}", agent_path.display()))?;
    }
    Ok(())
}

pub fn start() -> Result<()> {
    let target = service_target()?;
    if !matches!(launchctl(&["print", &target]), Ok(output) if output.status.success()) {
        load(&launch_agent_path()?)?;
    }
    checked_launchctl(&["kickstart", "-k", &target])?;
    println!("Background sync is on; a sync has started.");
    Ok(())
}

pub fn stop() -> Result<()> {
    let output = launchctl(&["bootout", &service_target()?])?;
    if output.status.success() {
        println!("Background sync is off.");
    } else {
        println!("Background sync was already off.");
    }
    Ok(())
}

pub fn restart() -> Result<()> {
    let target = service_target()?;
    let _ = launchctl(&["bootout", &target]);
    load(&launch_agent_path()?)?;
    checked_launchctl(&["kickstart", "-k", &target])?;
    println!("Background sync restarted.");
    Ok(())
}

pub fn status() -> Result<()> {
    let output = launchctl(&["print", &service_target()?])?;
    if output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
        Ok(())
    } else {
        bail!("Oura LaunchAgent is not installed or loaded")
    }
}

pub fn status_text() -> &'static str {
    let Ok(target) = service_target() else {
        return "unknown";
    };
    match launchctl(&["print", &target]) {
        Ok(output) if output.status.success() => "loaded",
        _ => "not loaded",
    }
}

pub fn show_logs(paths: &AppPaths, lines: usize) -> Result<()> {
    for (label, path) in [
        ("agent", paths.stdout_log()),
        ("errors", paths.stderr_log()),
    ] {
        println!("== {label}: {} ==", path.display());
        if !path.exists() {
            println!("<no log yet>");
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let collected = text.lines().collect::<Vec<_>>();
        let start = collected.len().saturating_sub(lines);
        for line in &collected[start..] {
            println!("{line}");
        }
    }
    Ok(())
}

pub fn sign_binary(binary: &Path) -> Result<()> {
    let identity = std::env::var("OURA_CODESIGN_IDENTITY").unwrap_or_else(|_| "-".to_owned());
    let output = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", &identity, "--identifier", LABEL])
        .arg(binary)
        .output()
        .context("signing the installed executable")?;
    if !output.status.success() {
        bail!(
            "codesign failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn load(agent_path: &Path) -> Result<()> {
    if !agent_path.exists() {
        bail!("background service is not installed; run oura install");
    }
    let target = service_target()?;
    let domain = domain_target()?;
    let _ = launchctl(&["bootout", &target]);
    let agent_text = agent_path
        .to_str()
        .ok_or_else(|| anyhow!("LaunchAgent path is not valid UTF-8"))?;
    checked_launchctl(&["bootstrap", &domain, agent_text])
}

fn render_plist(paths: &AppPaths, interval: u16) -> String {
    let executable = xml_escape(&paths.installed_binary.to_string_lossy());
    let home = xml_escape(&paths.root.to_string_lossy());
    let working_directory = xml_escape(&paths.root.to_string_lossy());
    let stdout = xml_escape(&paths.stdout_log().to_string_lossy());
    let stderr = xml_escape(&paths.stderr_log().to_string_lossy());
    let mut calendar = String::new();
    for minute in (0..60).step_by(interval as usize) {
        calendar.push_str(&format!(
            "        <dict><key>Minute</key><integer>{minute}</integer></dict>\n"
        ));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{executable}</string>
        <string>--home</string>
        <string>{home}</string>
        <string>sync</string>
        <string>--background</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{working_directory}</string>
    <key>StartCalendarInterval</key>
    <array>
{calendar}    </array>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>Umask</key>
    <integer>63</integer>
    <key>ExitTimeOut</key>
    <integer>30</integer>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#
    )
}

fn validate_plist(path: &Path) -> Result<()> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-lint"])
        .arg(path)
        .output()
        .context("validating LaunchAgent property list")?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "invalid LaunchAgent property list: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn launch_agent_path() -> Result<PathBuf> {
    launch_agent_path_for(LABEL)
}

fn launch_agent_path_for(label: &str) -> Result<PathBuf> {
    Ok(BaseDirs::new()
        .ok_or_else(|| anyhow!("could not determine your home directory"))?
        .home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist")))
}

fn service_target() -> Result<String> {
    service_target_for(LABEL)
}

fn service_target_for(label: &str) -> Result<String> {
    Ok(format!("{}/{label}", domain_target()?))
}

fn domain_target() -> Result<String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("determining user id")?;
    if !output.status.success() {
        bail!("could not determine user id");
    }
    let uid = String::from_utf8(output.stdout)?.trim().to_owned();
    Ok(format!("gui/{uid}"))
}

fn launchctl(arguments: &[&str]) -> Result<Output> {
    Command::new("/bin/launchctl")
        .args(arguments)
        .output()
        .context("running launchctl")
}

fn checked_launchctl(arguments: &[&str]) -> Result<()> {
    let output = launchctl(arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("launchctl failed: {}{}", stdout.trim(), stderr.trim())
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_has_expected_half_hour_entries() {
        let paths = AppPaths::resolve(Some(Path::new("/tmp/Oura & Me"))).unwrap();
        let plist = render_plist(&paths, 30);
        assert!(plist.contains("<integer>0</integer>"));
        assert!(plist.contains("<integer>30</integer>"));
        assert!(plist.contains("com.rayhanadev.oura-cli-sync"));
        assert!(!plist.contains("dev.oura.sync"));
        assert!(plist.contains(&paths.installed_binary.to_string_lossy().to_string()));
        assert!(!plist.contains("/tmp/Oura &amp; Me/bin/oura"));
    }
}
