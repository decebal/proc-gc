//! Reaping processes whose owner is gone and that keep burning CPU.
//!
//! A process may be signalled only when every one of these holds, each pinned
//! by a test:
//!
//! 1. **It is an orphan.** Its parent died and it was reparented to init (or,
//!    on Linux, to a `systemd --user` subreaper).
//! 2. **Nobody manages it.** launchd lists it as a job (every app and agent on
//!    macOS also has PPID 1), systemd runs it as a service, or it belongs to
//!    another user: any of these means it is someone's on purpose.
//! 3. **It is not a build or a test.** `cargo`, `rustc`, linkers, and anything
//!    executing from a Cargo build directory are hot by nature.
//! 4. **No live agent session owns it.** A killing rule that ignored this
//!    killed six pieces of Claude Code background work in one week elsewhere:
//!    pytest runs and scratch scripts whose wrapper shell had exited. A process
//!    whose working directory is a live agent session's project, or whose
//!    arguments name a live session's temp directory, is held.
//! 5. **It has burned CPU for an hour.** At least `cpu_percent` of one core in
//!    every interval between samples, samples taken by the scheduled runs and
//!    kept between them, for `min_hot_minutes` with no gap longer than
//!    `max_gap_minutes`. One sample is not evidence: a container read at 161%
//!    once sat at 0.12% minutes later.
//!
//! Then it is re-checked (same start time, still an orphan, still hot over a
//! fresh three-second sample) and sent SIGTERM, alone: never its process
//! group, which once took out the terminal the sessions ran in.

use std::time::Duration;

pub mod bounded;
pub mod classify;
pub mod config;
pub mod hot;
pub mod install;
pub mod managed;
pub mod owner;
pub mod reap;
pub mod skill;
pub mod snapshot;
pub mod state;
pub mod toml_subset;
pub mod tree;

/// A start time derived as `now − etime`, both whole seconds, moves by a
/// second between readings. Two readings this close name the same process.
pub const START_TOLERANCE: u64 = 2;

/// One process as a snapshot sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    /// Start time, whole seconds since the Unix epoch. With the pid it names
    /// one process for its whole life: a reused pid has a later start.
    pub start: u64,
    /// Cumulative user + system CPU time, seconds.
    pub cpu: f64,
    /// Full command line; the first word is the executable.
    pub args: String,
}

impl Proc {
    /// The executable's file name.
    pub fn exe_name(&self) -> &str {
        let exe = self.args.split_whitespace().next().unwrap_or("");
        exe.rsplit('/').next().unwrap_or(exe)
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.pid, self.start)
    }
}

pub fn human_minutes(d: Duration) -> String {
    let m = d.as_secs() / 60;
    if m >= 120 {
        format!("{}h", m / 60)
    } else {
        format!("{m} min")
    }
}
