//! The record each run leaves, and the one line the session hook derives
//! from it. A schedule that stops running is silent, so the hook also speaks
//! when the record is missing or stale.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::human_minutes;

pub const RECORD: &str = "last-run.json";
pub const SAMPLES: &str = "samples.json";

/// The schedule runs every five minutes; six missed runs means it stopped.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
/// A reap is still news to a session started within this long of it.
pub const REAP_NEWS: Duration = Duration::from_secs(2 * 3600);

#[derive(Debug, Clone, PartialEq)]
pub struct Hot {
    pub pid: u32,
    pub command: String,
    pub cores: f64,
    pub hot_secs: u64,
    /// Why it is not stopped; `None` for a candidate.
    pub held: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reaped {
    pub pid: u32,
    pub command: String,
    pub outcome: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    pub finished_at: u64,
    pub error: Option<String>,
    pub hot: Vec<Hot>,
    pub reaped: Vec<Reaped>,
    /// Orphans grouped by executable, only groups of five or more.
    pub crowds: Vec<(String, usize)>,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Record {
    pub fn to_json(&self) -> Value {
        json!({
            "finished_at": self.finished_at,
            "error": self.error,
            "hot": self.hot.iter().map(|h| json!({
                "pid": h.pid, "command": h.command, "cores": h.cores, "hot_secs": h.hot_secs,
                "held": h.held,
            })).collect::<Vec<_>>(),
            "reaped": self.reaped.iter().map(|r| json!({
                "pid": r.pid, "command": r.command, "outcome": r.outcome, "at": r.at,
            })).collect::<Vec<_>>(),
            "crowds": self.crowds.iter().map(|(n, c)| json!({ "command": n, "count": c }))
                .collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<Record> {
        let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
        Some(Record {
            finished_at: v["finished_at"].as_u64()?,
            error: v["error"].as_str().map(str::to_string),
            hot: v["hot"]
                .as_array()?
                .iter()
                .filter_map(|h| {
                    Some(Hot {
                        pid: u32::try_from(h["pid"].as_u64()?).ok()?,
                        command: text(h, "command"),
                        cores: h["cores"].as_f64()?,
                        hot_secs: h["hot_secs"].as_u64()?,
                        held: h["held"].as_str().map(str::to_string),
                    })
                })
                .collect(),
            reaped: v["reaped"]
                .as_array()?
                .iter()
                .filter_map(|r| {
                    Some(Reaped {
                        pid: u32::try_from(r["pid"].as_u64()?).ok()?,
                        command: text(r, "command"),
                        outcome: text(r, "outcome"),
                        at: r["at"].as_u64()?,
                    })
                })
                .collect(),
            crowds: v["crowds"]
                .as_array()?
                .iter()
                .filter_map(|c| {
                    Some((
                        text(c, "command"),
                        usize::try_from(c["count"].as_u64()?).ok()?,
                    ))
                })
                .collect(),
        })
    }
}

pub fn write(dir: &Path, name: &str, value: &Value) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!("{name}.tmp"));
    let body = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, dir.join(name)).map_err(|e| e.to_string())
}

pub fn read(dir: &Path, name: &str) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(dir.join(name)).ok()?).ok()
}

/// What a session should be told, or `None`. Pure.
pub fn hook_line(record: Option<&Record>, report_after: Duration, now: u64) -> Option<String> {
    let Some(r) = record else {
        return Some(
            "proc-gc has never completed a run here. Schedule it with `proc-gc install`.".into(),
        );
    };
    let age = Duration::from_secs(now.saturating_sub(r.finished_at));
    if let Some(e) = &r.error {
        return Some(format!(
            "proc-gc: the last run ({} ago) could not complete: {e}",
            human_minutes(age)
        ));
    }
    if age > STALE_AFTER {
        return Some(format!(
            "proc-gc: no run for {}. The schedule is not running; `proc-gc install` reinstalls it.",
            human_minutes(age)
        ));
    }
    let mut parts = Vec::new();
    let recent: Vec<&Reaped> = r
        .reaped
        .iter()
        .filter(|x| now.saturating_sub(x.at) <= REAP_NEWS.as_secs())
        .collect();
    let (failed, stopped): (Vec<&Reaped>, Vec<&Reaped>) = recent
        .into_iter()
        .partition(|x| x.outcome.starts_with("Failed"));
    let list = |xs: &[&Reaped]| -> String {
        xs.iter()
            .map(|x| format!("{} ({})", x.pid, x.command))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !stopped.is_empty() {
        parts.push(format!("stopped {}", list(&stopped)));
    }
    if !failed.is_empty() {
        parts.push(format!("could not stop {}", list(&failed)));
    }
    let hot: Vec<String> = r
        .hot
        .iter()
        .filter(|h| h.hot_secs >= report_after.as_secs())
        .map(|h| {
            let kept = h
                .held
                .as_ref()
                .map_or(String::new(), |why| format!(", not stopped: {why}"));
            format!(
                "pid {} ({}) orphaned at {:.0}% CPU for {}{kept}",
                h.pid,
                h.command,
                h.cores * 100.0,
                human_minutes(Duration::from_secs(h.hot_secs))
            )
        })
        .collect();
    if !hot.is_empty() {
        parts.push(hot.join("; "));
    }
    if !r.crowds.is_empty() {
        let crowds: Vec<String> = r.crowds.iter().map(|(n, c)| format!("{c}× {n}")).collect();
        parts.push(format!("orphan crowds: {}", crowds.join(", ")));
    }
    (!parts.is_empty()).then(|| {
        format!(
            "proc-gc: {}. `proc-gc scan` explains each.",
            parts.join(". ")
        )
    })
}
