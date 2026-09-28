//! Signals against real processes. Each target runs under an intermediate
//! shell that stands in for init: it is passed as the orphan parent, and it
//! collects the target when it exits, as launchd would.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use proc_gc::reap::{probe, reap, Outcome};
use proc_gc::snapshot;
use proc_gc::state::now_secs;

struct Target {
    parent: Child,
    pid: u32,
    start: u64,
}

impl Target {
    /// `body` runs in the background of a shell that waits for it.
    fn spawn(body: &str) -> Target {
        let mut parent = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("({body}) & echo $!; wait"))
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut line = String::new();
        BufReader::new(parent.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("read pid");
        let pid: u32 = line.trim().parse().expect("pid");
        let start = snapshot::take(now_secs())
            .expect("snapshot")
            .into_iter()
            .find(|p| p.pid == pid)
            .expect("target in the process table")
            .start;
        Target { parent, pid, start }
    }

    fn reap(&self, orphan_parent: u32, start: u64, hot: bool) -> Outcome {
        reap(
            &[(self.pid, start)],
            &[orphan_parent],
            Duration::from_secs(1),
            &now_secs,
            &|_| hot,
        )
    }

    fn alive(&self) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &self.pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if probe(self.pid, self.start, now_secs()).is_some() {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &self.pid.to_string()])
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.parent.kill();
        let _ = self.parent.wait();
    }
}

#[test]
fn a_hot_orphan_that_honours_sigterm_is_terminated() {
    let t = Target::spawn("exec sleep 60");
    assert_eq!(t.reap(t.parent.id(), t.start, true), Outcome::Terminated);
    assert!(!t.alive());
}

#[test]
fn one_that_ignores_sigterm_is_killed() {
    let t = Target::spawn("trap '' TERM; while :; do sleep 1; done");
    assert_eq!(t.reap(t.parent.id(), t.start, true), Outcome::Killed);
    assert!(!t.alive());
}

#[test]
fn a_tree_that_cooled_is_left_running() {
    let t = Target::spawn("exec sleep 60");
    assert_eq!(t.reap(t.parent.id(), t.start, false), Outcome::Cooled);
    assert!(t.alive());
}

#[test]
fn a_process_that_is_no_longer_an_orphan_is_left_running() {
    let t = Target::spawn("exec sleep 60");
    assert_eq!(t.reap(1, t.start, true), Outcome::Adopted);
    assert!(t.alive());
}

/// The pid names another process than the one judged: a later start time.
#[test]
fn a_reused_pid_is_never_signalled() {
    let t = Target::spawn("exec sleep 60");
    assert_eq!(
        t.reap(t.parent.id(), t.start + 100, true),
        Outcome::Vanished
    );
    assert!(t.alive());
}
