//! Running a helper (`lsof`, `df`, `launchctl`) under a deadline.
//!
//! The caller owns the child for its whole life: it polls, and on the deadline
//! it kills AND waits. A reader thread holds only the stdout pipe, so the pipe
//! cannot fill and stall the child, and the thread never owns the process —
//! a thread blocked in `output()` cannot enforce a deadline
//! (rules/process-ownership.md).

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct Finished {
    pub success: bool,
    pub stdout: String,
}

/// `None` when the command could not start or ran past `timeout`.
pub fn run(mut command: Command, timeout: Duration) -> Option<Finished> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut pipe = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().unwrap_or_default();
    let status = status?;
    Some(Finished {
        success: status.success(),
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
    })
}
