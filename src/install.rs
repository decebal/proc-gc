//! Scheduling `proc-gc run` every five minutes under the platform's own
//! supervisor: launchd on macOS, a systemd user timer on Linux.
//!
//! The supervisor owns and reaps each run. A session hook that started the
//! run itself would leave a child outliving the session that spawned it,
//! which is the kind of leak this tool exists to clean up; the hook only reads
//! the record a scheduled run leaves.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bounded;

pub const LABEL: &str = "dev.proc-gc";
pub const INTERVAL_SECS: u64 = 300;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub path: PathBuf,
    pub body: String,
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn launchd_plist(exe: &Path, state_dir: &Path) -> String {
    let exe = xml_escape(&exe.display().to_string());
    let log = xml_escape(&state_dir.join("scheduled.log").display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>run</string><string>--quiet</string></array>
  <key>StartInterval</key><integer>{INTERVAL_SECS}</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#
    )
}

pub fn systemd_service(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=proc-gc: reap ownerless processes that have burned CPU for an hour\n\n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={} run --quiet\n\
         Nice=10\n",
        exe.display()
    )
}

pub fn systemd_timer() -> String {
    format!(
        "[Unit]\n\
         Description=Run proc-gc every five minutes\n\n\
         [Timer]\n\
         OnBootSec=5min\n\
         OnUnitActiveSec={INTERVAL_SECS}s\n\
         Persistent=true\n\n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

pub fn units(exe: &Path, home: &Path, state_dir: &Path) -> Vec<Unit> {
    if cfg!(target_os = "macos") {
        vec![Unit {
            path: home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
            body: launchd_plist(exe, state_dir),
        }]
    } else {
        let dir = home.join(".config/systemd/user");
        vec![
            Unit {
                path: dir.join("proc-gc.service"),
                body: systemd_service(exe),
            },
            Unit {
                path: dir.join("proc-gc.timer"),
                body: systemd_timer(),
            },
        ]
    }
}

/// The supervisor commands that (re)load the schedule, in order. The first
/// unloads a previous install and is allowed to fail.
pub fn load_commands(units: &[Unit], uid: &str) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        let plist = units[0].path.display().to_string();
        vec![
            s(&["/bin/launchctl", "bootout", &format!("gui/{uid}/{LABEL}")]),
            s(&["/bin/launchctl", "bootstrap", &format!("gui/{uid}"), &plist]),
        ]
    } else {
        vec![
            s(&["systemctl", "--user", "stop", "proc-gc.timer"]),
            s(&["systemctl", "--user", "daemon-reload"]),
            s(&["systemctl", "--user", "enable", "--now", "proc-gc.timer"]),
        ]
    }
}

pub fn unload_commands(uid: &str) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        vec![s(&[
            "/bin/launchctl",
            "bootout",
            &format!("gui/{uid}/{LABEL}"),
        ])]
    } else {
        vec![s(&[
            "systemctl",
            "--user",
            "disable",
            "--now",
            "proc-gc.timer",
        ])]
    }
}

pub fn uid() -> Option<String> {
    let mut cmd = Command::new("/usr/bin/id");
    cmd.arg("-u");
    let out = bounded::run(cmd, TOOL_TIMEOUT)?;
    let uid = out.stdout.trim().to_string();
    (out.success && !uid.is_empty()).then_some(uid)
}

pub fn write_units(units: &[Unit]) -> Result<(), String> {
    for unit in units {
        if let Some(dir) = unit.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(&unit.path, &unit.body).map_err(|e| format!("{}: {e}", unit.path.display()))?;
    }
    Ok(())
}

/// Run each command; every one but the first must succeed when
/// `first_may_fail`.
pub fn run_commands(commands: &[Vec<String>], first_may_fail: bool) -> Result<(), String> {
    for (i, argv) in commands.iter().enumerate() {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        let ok = bounded::run(cmd, TOOL_TIMEOUT).is_some_and(|o| o.success);
        let may_fail = first_may_fail && i == 0;
        if !(ok || may_fail) {
            return Err(format!("`{}` failed", argv.join(" ")));
        }
    }
    Ok(())
}
