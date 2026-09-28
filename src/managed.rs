//! Processes a supervisor runs on purpose.
//!
//! On macOS every launchd agent, every XPC service and every app launched from
//! the Finder has PPID 1, exactly like a leaked child. Measured on one
//! machine: 470 of 654 user processes had PPID 1. What separates them is that
//! launchd lists the ones it manages, with their PIDs, in `launchctl list`.
//! On Linux a systemd service's processes sit in a `*.service` cgroup; a
//! terminal or app scope is `*.scope`.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bounded;

const LAUNCHCTL: &str = "/bin/launchctl";

/// PIDs in `launchctl list` output (`PID\tStatus\tLabel`, `-` when not
/// running).
pub fn parse_launchctl_list(output: &str) -> HashSet<u32> {
    output
        .lines()
        .skip(1)
        .filter_map(|l| l.split('\t').next())
        .filter_map(|pid| pid.trim().parse().ok())
        .collect()
}

/// `None` when launchctl could not be read: nothing may then be treated as
/// unmanaged.
pub fn launchd_pids() -> Option<HashSet<u32>> {
    let mut cmd = Command::new(LAUNCHCTL);
    cmd.arg("list");
    let out = bounded::run(cmd, Duration::from_secs(20))?;
    let pids = parse_launchctl_list(&out.stdout);
    (out.success && !pids.is_empty()).then_some(pids)
}

/// Linux: true when the process belongs to a systemd service unit.
pub fn cgroup_is_service(cgroup: &str) -> bool {
    cgroup
        .lines()
        .filter_map(|l| l.rsplit(':').next())
        .any(|path| path.split('/').any(|seg| seg.ends_with(".service")))
}

pub fn linux_service_pids(proc_root: &Path, pids: &[u32]) -> HashSet<u32> {
    pids.iter()
        .copied()
        .filter(|pid| {
            fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup"))
                .is_ok_and(|c| cgroup_is_service(&c))
        })
        .collect()
}
