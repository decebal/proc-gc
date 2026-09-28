//! The process table, read once per sample.
//!
//! macOS: one `ps` call. Linux: `/proc/<pid>/stat`, `status` and `cmdline`.
//! `ps` is invoked by absolute path: an output-filtering shell proxy once
//! rewrote a plain `ps` and reported 31 processes against a real 1,242, with
//! argv truncated.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::{bounded, Proc};

const PS: &str = "/bin/ps";
const PS_TIMEOUT: Duration = Duration::from_secs(20);

/// `[[dd-]hh:]mm:ss[.cc]` → seconds. Both `etime` and `time` use it; `time`
/// lets the minutes run past 60 (`116:43.61`).
pub fn parse_clock(raw: &str) -> Option<f64> {
    let (days, clock) = match raw.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, raw),
    };
    let mut secs = 0.0;
    for part in clock.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + secs)
}

/// Parse `ps -Axo pid=,ppid=,uid=,etime=,time=,args=`. Rows that do not parse
/// are skipped: a guessed field could name the wrong pid.
pub fn parse_ps(output: &str, now: u64) -> Vec<Proc> {
    let mut out = Vec::new();
    for line in output.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(uid), Some(etime), Some(time)) =
            (it.next(), it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(uid), Some(elapsed), Some(cpu)) = (
            pid.parse::<u32>(),
            ppid.parse::<u32>(),
            uid.parse::<u32>(),
            parse_clock(etime),
            parse_clock(time),
        ) else {
            continue;
        };
        let args: Vec<&str> = it.collect();
        out.push(Proc {
            pid,
            ppid,
            uid,
            start: now.saturating_sub(elapsed as u64),
            cpu,
            args: args.join(" "),
        });
    }
    out
}

/// Parse one `/proc/<pid>/stat`. The command sits in parentheses and may
/// contain spaces or parentheses itself, so fields are counted from the LAST
/// `)`. Returns (ppid, utime + stime ticks, starttime ticks).
pub fn parse_proc_stat(stat: &str) -> Option<(u32, u64, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After the command: state(0) ppid(1) … utime(11) stime(12) … starttime(19).
    let ppid = f.get(1)?.parse().ok()?;
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let start: u64 = f.get(19)?.parse().ok()?;
    Some((ppid, utime + stime, start))
}

pub fn parse_status_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|u| u.parse().ok())
}

/// Linux: every readable `/proc/<pid>`. `boot` is the boot time in epoch
/// seconds and `ticks` the clock-tick rate, both read once by the caller.
pub fn read_proc(root: &Path, boot: u64, ticks: f64) -> Vec<Proc> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let dir = entry.path();
        let (Ok(stat), Ok(status)) = (
            fs::read_to_string(dir.join("stat")),
            fs::read_to_string(dir.join("status")),
        ) else {
            continue;
        };
        let (Some((ppid, cpu_ticks, start_ticks)), Some(uid)) =
            (parse_proc_stat(&stat), parse_status_uid(&status))
        else {
            continue;
        };
        let args = fs::read(dir.join("cmdline"))
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        out.push(Proc {
            pid,
            ppid,
            uid,
            start: boot + (start_ticks as f64 / ticks) as u64,
            cpu: cpu_ticks as f64 / ticks,
            args,
        });
    }
    out
}

pub fn is_linux() -> bool {
    Path::new("/proc/self/stat").exists()
}

/// The whole process table. `None` when it cannot be read; nothing may be
/// signalled on a partial or failed read.
pub fn take(now: u64) -> Option<Vec<Proc>> {
    let procs = if is_linux() {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let boot: u64 = stat
            .lines()
            .find_map(|l| l.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()?;
        read_proc(Path::new("/proc"), boot, clock_ticks())
    } else {
        let mut cmd = Command::new(PS);
        cmd.args(["-Axo", "pid=,ppid=,uid=,etime=,time=,args=", "-ww"]);
        let out = bounded::run(cmd, PS_TIMEOUT)?;
        if !out.success {
            return None;
        }
        parse_ps(&out.stdout, now)
    };
    (!procs.is_empty()).then_some(procs)
}

fn clock_ticks() -> f64 {
    let mut cmd = Command::new("getconf");
    cmd.arg("CLK_TCK");
    bounded::run(cmd, Duration::from_secs(5))
        .and_then(|o| o.stdout.trim().parse().ok())
        .unwrap_or(100.0)
}
