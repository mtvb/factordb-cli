//! Persistent CLI configuration: the RPC endpoint, the account API token and a default timeout
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_URL: &str = "http://127.0.0.1:4059/rpc";
pub const DEFAULT_TIMEOUT: f64 = 120.0;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}

pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("FDB_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    base.join("fdb").join("config.toml")
}

pub fn load() -> Result<Config> {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("bad config file {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", p.display())),
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let s = toml::to_string_pretty(cfg).context("cannot serialize config")?;
    // Write a private temp file next to the target and rename it into place: the token never sits in
    // a file with loose permissions, and a crash mid-write cannot leave a truncated config behind.
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(s.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &p)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("cannot write {}", p.display()));
    }
    Ok(())
}

/// Validate a timeout in seconds: finite, non-negative (0 = none) and small enough for Duration.
pub fn check_timeout(t: f64) -> std::result::Result<f64, String> {
    if !t.is_finite() {
        Err("timeout must be a finite number of seconds".into())
    } else if t < 0.0 {
        Err("timeout must be 0 (none) or a positive number of seconds".into())
    } else if t > 1.0e9 {
        Err("timeout is too large (at most 1e9 seconds; 0 = none)".into())
    } else {
        Ok(t)
    }
}

/// A validated timeout as a Duration; 0 means no timeout.
pub fn timeout_duration(t: f64) -> Option<Duration> {
    if t > 0.0 { Duration::try_from_secs_f64(t).ok() } else { None }
}

/// A token shortened for display: first and last four characters.
pub fn mask(token: &str) -> String {
    let n = token.chars().count();
    if n <= 10 {
        return "*".repeat(n);
    }
    let head: String = token.chars().take(4).collect();
    let tail: String = token.chars().skip(n - 4).collect();
    format!("{head} {tail} ({n} chars)")
}
