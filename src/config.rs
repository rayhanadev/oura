use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::absolute;
use crate::cli::Cli;
use crate::load_key_file;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
    pub database: PathBuf,
    pub config: PathBuf,
    pub installed_binary: PathBuf,
    pub secrets_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub run_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve(override_root: Option<&Path>) -> Result<Self> {
        let base_dirs =
            BaseDirs::new().ok_or_else(|| anyhow!("could not determine your home directory"))?;
        let root = match override_root {
            Some(path) => absolute(path.to_path_buf())?,
            None => base_dirs.home_dir().join(".oura"),
        };
        Ok(Self {
            database: root.join("oura.db"),
            config: root.join("config.toml"),
            installed_binary: base_dirs.home_dir().join(".local/bin/oura"),
            secrets_dir: root.join("secrets"),
            logs_dir: root.join("logs"),
            run_dir: root.join("run"),
            root,
        })
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for path in [&self.root, &self.secrets_dir, &self.logs_dir, &self.run_dir] {
            fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub fn lock_file(&self) -> PathBuf {
        self.run_dir.join("sync.lock")
    }

    #[cfg(target_os = "macos")]
    pub fn stdout_log(&self) -> PathBuf {
        self.logs_dir.join("agent.log")
    }

    #[cfg(target_os = "macos")]
    pub fn stderr_log(&self) -> PathBuf {
        self.logs_dir.join("agent-error.log")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub name: String,
    pub address: Option<String>,
    pub scan_timeout_seconds: u64,
    pub database_path: PathBuf,
    pub key_path: PathBuf,
    #[serde(default = "default_battery_threshold")]
    pub low_battery_threshold_percent: u8,
}

fn default_battery_threshold() -> u8 {
    10
}

impl AppConfig {
    pub fn defaults(paths: &AppPaths) -> Self {
        Self {
            name: "Oura".to_owned(),
            address: None,
            scan_timeout_seconds: 60,
            database_path: paths.database.clone(),
            key_path: paths.secrets_dir.join("ring.key"),
            low_battery_threshold_percent: default_battery_threshold(),
        }
    }

    pub fn load_or_default(paths: &AppPaths) -> Result<Self> {
        if !paths.config.exists() {
            return Ok(Self::defaults(paths));
        }
        let text = fs::read_to_string(&paths.config)
            .with_context(|| format!("reading {}", paths.config.display()))?;
        toml::from_str(&text).context("parsing Oura configuration")
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure_layout()?;
        let encoded = toml::to_string_pretty(self).context("serializing configuration")?;
        write_private(&paths.config, encoded.as_bytes())
    }

    pub fn apply_overrides(&mut self, cli: &Cli) {
        if let Some(path) = &cli.db {
            self.database_path = path.clone();
        }
        if let Some(path) = &cli.key_file {
            self.key_path = path.clone();
        }
        if let Some(name) = &cli.name {
            self.name.clone_from(name);
        }
        if let Some(address) = &cli.address {
            self.address = Some(address.clone());
        }
    }

    pub fn configure(
        &mut self,
        paths: &AppPaths,
        key_file: Option<&Path>,
        name: Option<String>,
        address: Option<String>,
        scan_timeout: Option<u64>,
        battery_threshold: Option<u8>,
    ) -> Result<()> {
        paths.ensure_layout()?;
        if let Some(name) = name {
            self.name = name;
        }
        if let Some(address) = address {
            self.address = Some(address);
        }
        if let Some(scan_timeout) = scan_timeout {
            if !(5..=300).contains(&scan_timeout) {
                bail!("scan timeout must be between 5 and 300 seconds");
            }
            self.scan_timeout_seconds = scan_timeout;
        }
        if let Some(threshold) = battery_threshold {
            if threshold > 100 {
                bail!("battery threshold must be between 0 and 100 percent");
            }
            self.low_battery_threshold_percent = threshold;
        }
        if let Some(source) = key_file {
            let key = load_key_file(source)?;
            write_private(&self.key_path, format!("{}\n", hex::encode(key)).as_bytes())?;
        }
        self.save(paths)
    }

    pub fn load_key(&self) -> Result<[u8; 16]> {
        load_key_file(&self.key_path)
    }
}

pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(contents)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_stay_inside_selected_home() {
        let paths = AppPaths::resolve(Some(Path::new("/tmp/oura-test-home"))).unwrap();
        let config = AppConfig::defaults(&paths);
        assert_eq!(
            config.database_path,
            Path::new("/tmp/oura-test-home/oura.db")
        );
        assert_eq!(
            config.key_path,
            Path::new("/tmp/oura-test-home/secrets/ring.key")
        );
        assert_eq!(config.low_battery_threshold_percent, 10);
    }
}
