//! Machine-level settings, read with the in-crate TOML-subset reader.
//!
//! ```toml
//! [proc-gc]
//! min_hot_minutes      = "60"   # hot this long before a reap
//! cpu_percent          = "80"   # of one core, in every interval
//! max_gap_minutes      = "20"   # a longer gap between samples breaks the streak
//! grace_seconds        = "10"   # SIGTERM → SIGKILL
//! report_after_minutes = "15"   # the session line names a hot orphan after this
//! build_names          = [...]  # executables never reaped, added to the built-ins
//! hold                 = []     # argument substrings never reaped
//! ```
//!
//! Numbers are quoted: the reader supports strings and lists only.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::classify::DEFAULT_BUILD_NAMES;
use crate::toml_subset::Config;

const TABLE: &str = "proc-gc";

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub min_hot: Duration,
    /// Share of one core, 0.8 for 80%.
    pub cores: f64,
    pub max_gap: Duration,
    pub grace: Duration,
    pub report_after: Duration,
    pub build_names: Vec<String>,
    pub holds: Vec<String>,
}

/// `None` when `HOME` is unset or not absolute: state and config paths would
/// be guessed.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.parent().is_some())
}

/// `$PROC_GC_CONFIG`, else `$XDG_CONFIG_HOME/proc-gc/config.toml`, else
/// `~/.config/proc-gc/config.toml`.
pub fn default_path(home: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("PROC_GC_CONFIG") {
        return PathBuf::from(p);
    }
    let base =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    base.join("proc-gc/config.toml")
}

/// `$XDG_STATE_HOME/proc-gc`, else `~/.local/state/proc-gc`.
pub fn state_dir(home: &Path) -> PathBuf {
    let base =
        std::env::var_os("XDG_STATE_HOME").map_or_else(|| home.join(".local/state"), PathBuf::from);
    base.join("proc-gc")
}

impl Settings {
    pub fn defaults() -> Settings {
        Settings {
            min_hot: Duration::from_secs(60 * 60),
            cores: 0.8,
            max_gap: Duration::from_secs(20 * 60),
            grace: Duration::from_secs(10),
            report_after: Duration::from_secs(15 * 60),
            build_names: DEFAULT_BUILD_NAMES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            holds: Vec::new(),
        }
    }

    /// Defaults overlaid with the file at `path`. A missing file means
    /// defaults; a malformed file or an unreadable value is an error.
    pub fn load(path: &Path) -> Result<Settings, String> {
        let config = Config::read(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .unwrap_or_default();
        Settings::from_config(&config)
    }

    pub fn from_config(config: &Config) -> Result<Settings, String> {
        let mut s = Settings::defaults();
        let key = |k: &str| format!("{TABLE}.{k}");
        let number = |k: &str| -> Result<Option<u64>, String> {
            config
                .string(&key(k))
                .map(|v| {
                    v.trim()
                        .parse::<u64>()
                        .map_err(|_| format!("{k}: cannot read {v:?}"))
                })
                .transpose()
        };
        if let Some(m) = number("min_hot_minutes")? {
            s.min_hot = Duration::from_secs(m * 60);
        }
        if let Some(p) = number("cpu_percent")? {
            s.cores = p as f64 / 100.0;
        }
        if let Some(m) = number("max_gap_minutes")? {
            s.max_gap = Duration::from_secs(m * 60);
        }
        if let Some(g) = number("grace_seconds")? {
            s.grace = Duration::from_secs(g);
        }
        if let Some(m) = number("report_after_minutes")? {
            s.report_after = Duration::from_secs(m * 60);
        }
        if let Some(v) = config.list(&key("build_names")) {
            s.build_names
                .extend(v.into_iter().filter(|n| !n.is_empty()));
        }
        if let Some(v) = config.list(&key("hold")) {
            s.holds = v;
        }
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.min_hot < Duration::from_secs(60) {
            return Err("min_hot_minutes must be at least 1".into());
        }
        if !(0.1..=64.0).contains(&self.cores) {
            return Err("cpu_percent must be between 10 and 6400".into());
        }
        if self.max_gap.is_zero() || self.max_gap > self.min_hot {
            return Err("max_gap_minutes must be at least 1 and at most min_hot_minutes".into());
        }
        if self.grace > Duration::from_secs(60) {
            return Err("grace_seconds must be at most 60".into());
        }
        Ok(())
    }
}
