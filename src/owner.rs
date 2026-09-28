//! Which live agent sessions might still own a process.
//!
//! Claude Code runs a background command under a wrapper shell whose parent is
//! the live `claude` process. If that shell exits first, the command is
//! reparented to launchd and looks exactly like a leak by PPID and CPU. A
//! killing rule that ignored this killed six pieces of background work in one
//! week elsewhere. So an orphan is treated as owned while any live agent
//! session works in the same project (one working directory inside the
//! other), or while its arguments name an agent temp directory and an agent is
//! running at all.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{bounded, Proc};

const LSOF: &str = "/usr/sbin/lsof";

/// Markers in an argument list that identify an agent CLI.
const AGENT_MARKERS: [&str; 3] = [
    "@anthropic-ai/claude-code",
    "claude-code/cli",
    "@openai/codex",
];
const AGENT_EXES: [&str; 2] = ["claude", "codex"];

/// Paths agents create per session for scratch files and background output.
const AGENT_TEMP_MARKERS: [&str; 3] = ["/tmp/claude-", "/private/tmp/claude-", "/.codex/"];

pub fn is_agent(p: &Proc) -> bool {
    AGENT_EXES.contains(&p.exe_name()) || AGENT_MARKERS.iter().any(|m| p.args.contains(m))
}

pub fn mentions_agent_temp(p: &Proc) -> bool {
    AGENT_TEMP_MARKERS.iter().any(|m| p.args.contains(m))
}

/// One directory inside the other, and neither is too broad to mean a
/// project: `/` and the home directory contain everything.
pub fn same_project(a: &Path, b: &Path, home: &Path) -> bool {
    let broad = |p: &Path| p == Path::new("/") || p == home;
    if broad(a) || broad(b) {
        return false;
    }
    a.starts_with(b) || b.starts_with(a)
}

/// Parse `lsof -F pn -d cwd` output into pid → cwd.
pub fn parse_lsof_cwd(output: &str) -> HashMap<u32, PathBuf> {
    let mut out = HashMap::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(v) = line.strip_prefix('p') {
            pid = v.parse().ok();
        } else if let (Some(v), Some(p)) = (line.strip_prefix('n'), pid) {
            if v.starts_with('/') {
                out.insert(p, PathBuf::from(v));
            }
        }
    }
    out
}

/// Working directories for `pids`. A pid missing from the result has no
/// readable cwd; callers treat that as "cannot tell", never as "no owner".
pub fn cwds(pids: &[u32]) -> HashMap<u32, PathBuf> {
    if pids.is_empty() {
        return HashMap::new();
    }
    if Path::new("/proc/self/cwd").exists() {
        return pids
            .iter()
            .filter_map(|pid| {
                fs::read_link(format!("/proc/{pid}/cwd"))
                    .ok()
                    .map(|c| (*pid, c))
            })
            .collect();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut cmd = Command::new(LSOF);
    cmd.current_dir("/")
        .args(["-w", "-n", "-P", "-a", "-d", "cwd", "-F", "pn", "-p", &list]);
    bounded::run(cmd, Duration::from_secs(30))
        .map(|o| parse_lsof_cwd(&o.stdout))
        .unwrap_or_default()
}
