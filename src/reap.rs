//! Stopping one orphan's tree, after proving it is still the tree that was
//! judged.
//!
//! Every signal goes to one pid, never a process group: a group signal once
//! took out the terminal the agent sessions ran in. The root is asked first;
//! members still running after the grace period are asked one by one, then
//! killed. Whether it worked is read back from the process table, not from
//! `kill`'s exit status.

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::snapshot::parse_clock;
use crate::{bounded, START_TOLERANCE};

const PS: &str = "/bin/ps";
const KILL: &str = "/bin/kill";
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub ppid: u32,
    pub start: u64,
    pub cpu: f64,
    /// Exited, waiting for its parent to collect it. Counts as gone.
    pub zombie: bool,
}

/// Parse `ps -o ppid=,etime=,time=,stat= -p <pid>`.
pub fn parse_probe(output: &str, now: u64) -> Option<Probe> {
    let mut it = output.split_whitespace();
    let ppid = it.next()?.parse().ok()?;
    let elapsed = parse_clock(it.next()?)?;
    let cpu = parse_clock(it.next()?)?;
    let zombie = it.next().is_some_and(|s| s.starts_with('Z'));
    Some(Probe {
        ppid,
        start: now.saturating_sub(elapsed as u64),
        cpu,
        zombie,
    })
}

/// The process now, if it is still the one that started at `start`.
pub fn probe(pid: u32, start: u64, now: u64) -> Option<Probe> {
    let mut cmd = Command::new(PS);
    cmd.args(["-o", "ppid=,etime=,time=,stat=", "-p", &pid.to_string()]);
    let out = bounded::run(cmd, TOOL_TIMEOUT)?;
    if !out.success {
        return None;
    }
    parse_probe(&out.stdout, now).filter(|p| p.start.abs_diff(start) <= START_TOLERANCE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Everything gone after SIGTERM.
    Terminated,
    /// Some members ignored SIGTERM; gone after SIGKILL.
    Killed,
    /// The root exited, or its pid now belongs to another process, first.
    Vanished,
    /// Not hot over the fresh sample; left alone.
    Cooled,
    /// The root is no longer an orphan.
    Adopted,
    /// Members still running after SIGKILL, or a signal could not be sent.
    Failed(String),
}

fn signal(sig: &str, pid: u32) -> bool {
    let mut cmd = Command::new(KILL);
    cmd.args([sig, &pid.to_string()]);
    bounded::run(cmd, TOOL_TIMEOUT).is_some_and(|o| o.success)
}

fn alive(members: &[(u32, u64)], now: u64) -> Vec<(u32, u64)> {
    members
        .iter()
        .copied()
        .filter(|&(pid, start)| probe(pid, start, now).is_some_and(|p| !p.zombie))
        .collect()
}

fn tree_cpu(members: &[(u32, u64)], now: u64) -> f64 {
    members
        .iter()
        .filter_map(|&(pid, start)| probe(pid, start, now))
        .map(|p| p.cpu)
        .sum()
}

fn wait_gone(members: &[(u32, u64)], grace: Duration, now: &dyn Fn() -> u64) -> Vec<(u32, u64)> {
    let deadline = Instant::now() + grace;
    loop {
        let left = alive(members, now());
        if left.is_empty() || Instant::now() >= deadline {
            return left;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// `members` is the tree as judged, root first, as (pid, start). `hot` says
/// whether a CPU rate in cores is still over the threshold.
pub fn reap(
    members: &[(u32, u64)],
    orphan_parents: &[u32],
    grace: Duration,
    now: &dyn Fn() -> u64,
    hot: &dyn Fn(f64) -> bool,
) -> Outcome {
    let Some(&(root, root_start)) = members.first() else {
        return Outcome::Vanished;
    };
    let Some(first) = probe(root, root_start, now()) else {
        return Outcome::Vanished;
    };
    if !orphan_parents.contains(&first.ppid) {
        return Outcome::Adopted;
    }
    let (t0, cpu0) = (Instant::now(), tree_cpu(members, now()));
    thread::sleep(Duration::from_secs(3));
    let rate = (tree_cpu(members, now()) - cpu0) / t0.elapsed().as_secs_f64();
    if !hot(rate) {
        return Outcome::Cooled;
    }
    if !signal("-TERM", root) {
        return Outcome::Failed("SIGTERM to the root could not be sent".into());
    }
    let mut left = wait_gone(members, grace, now);
    for &(pid, _) in &left {
        signal("-TERM", pid);
    }
    left = wait_gone(&left, grace, now);
    if left.is_empty() {
        return Outcome::Terminated;
    }
    for &(pid, _) in &left {
        signal("-KILL", pid);
    }
    let left = wait_gone(&left, Duration::from_secs(2), now);
    if left.is_empty() {
        Outcome::Killed
    } else {
        Outcome::Failed(format!(
            "{} member(s) still running after SIGKILL",
            left.len()
        ))
    }
}
