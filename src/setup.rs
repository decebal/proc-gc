//! The commands around a run: the session hook, installing and removing the
//! schedule, the Claude Code skill, and help.

use std::fs;
use std::path::{Path, PathBuf};

use proc_gc::config::{self, Settings};
use proc_gc::state::{self, Record};
use proc_gc::{install, skill};

const CONFIG_TEMPLATE: &str = include_str!("../config.example.toml");

pub fn hook(config_path: &Path, home: &Path) {
    let settings = match Settings::load(config_path) {
        Ok(s) => s,
        Err(e) => {
            println!("proc-gc: config error, scheduled runs will fail: {e}");
            return;
        }
    };
    let record =
        state::read(&config::state_dir(home), state::RECORD).and_then(|v| Record::from_json(&v));
    if let Some(line) = state::hook_line(record.as_ref(), settings.report_after, state::now_secs())
    {
        println!("{line}");
    }
}

pub fn install_cmd(config_path: &Path, home: &Path, dry_run: bool) -> i32 {
    let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
    else {
        eprintln!("proc-gc: cannot resolve my own path");
        return 2;
    };
    let in_build_dir = exe.ancestors().any(|a| {
        fs::read_to_string(a.join("CACHEDIR.TAG")).is_ok_and(|t| t.contains("created by cargo"))
    });
    if in_build_dir {
        eprintln!(
            "proc-gc: {} is inside a Cargo build directory, which a build-directory cleaner \
             may delete. Install it first: cargo install proc-gc",
            exe.display()
        );
        return 2;
    }
    let state_dir = config::state_dir(home);
    let units = install::units(&exe, home, &state_dir);
    let Some(uid) = install::uid() else {
        eprintln!("proc-gc: could not read the user id");
        return 2;
    };
    let commands = install::load_commands(&units, &uid);
    if dry_run {
        for u in &units {
            println!("--- {}\n{}", u.path.display(), u.body);
        }
        for c in &commands {
            println!("$ {}", c.join(" "));
        }
        return 0;
    }
    if !config_path.exists() {
        let written = config_path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(config_path, CONFIG_TEMPLATE));
        if let Err(e) = written {
            eprintln!("proc-gc: {}: {e}", config_path.display());
            return 2;
        }
        println!("wrote {}", config_path.display());
    }
    let result = fs::create_dir_all(&state_dir)
        .map_err(|e| e.to_string())
        .and_then(|()| install::write_units(&units))
        .and_then(|()| install::run_commands(&commands, true));
    match result {
        Ok(()) => {
            for u in &units {
                println!("installed {}", u.path.display());
            }
            println!(
                "Runs every five minutes. Nothing is reaped until a process has been an \
                 ownerless orphan burning CPU for min_hot_minutes; {} records each run.\n\
                 For Claude Code: `/plugin marketplace add decebal/proc-gc` then \
                 `/plugin install proc-gc@proc-gc` adds the skill and the session hook.\n\
                 Without the plugin: `proc-gc skill install`, and add to \
                 ~/.claude/settings.json under SessionStart:\n  \
                 {{ \"type\": \"command\", \"command\": \"{} hook\", \"timeout\": 10 }}",
                state_dir.join(state::RECORD).display(),
                exe.display()
            );
            0
        }
        Err(e) => {
            eprintln!("proc-gc: {e}");
            2
        }
    }
}

pub fn uninstall_cmd(home: &Path) -> i32 {
    let Some(uid) = install::uid() else {
        eprintln!("proc-gc: could not read the user id");
        return 2;
    };
    let _ = install::run_commands(&install::unload_commands(&uid), true);
    let exe = PathBuf::from("proc-gc");
    for u in install::units(&exe, home, &config::state_dir(home)) {
        if fs::remove_file(&u.path).is_ok() {
            println!("removed {}", u.path.display());
        }
    }
    0
}

/// `proc-gc skill` prints the skill; `proc-gc skill install` writes it to
/// `~/.claude/skills/proc-gc/` (or `--dir`).
pub fn skill_cmd(sub: Option<&str>, dir: Option<&Path>, home: &Path) -> i32 {
    match sub {
        None | Some("print") => {
            print!("{}", skill::SKILL_MD);
            0
        }
        Some("install") => {
            let dir = dir.map_or_else(|| skill::default_dir(home), Path::to_path_buf);
            match skill::install(&dir) {
                Ok(path) => {
                    println!("wrote {}", path.display());
                    0
                }
                Err(e) => {
                    eprintln!("proc-gc: {e}");
                    2
                }
            }
        }
        Some(other) => {
            eprintln!("proc-gc: unknown skill command {other:?}; use `print` or `install`");
            2
        }
    }
}

pub fn print_help() {
    println!(
        "proc-gc — reap processes whose owner is gone and that have burned CPU for an hour.

USAGE:
    proc-gc scan [--json]                 every orphan of yours and what holds it
    proc-gc run [--dry-run] [--quiet] [--json]
                                          sample CPU, reap what has been hot long enough
    proc-gc hook                          one line for a session, only when something is wrong
    proc-gc install [--dry-run]           schedule `run` every five minutes (launchd / systemd)
    proc-gc uninstall                     remove the schedule
    proc-gc skill [install] [--dir <d>]   print the Claude Code skill, or write it to
                                          ~/.claude/skills/proc-gc/

    --config <path>   settings file (default: $XDG_CONFIG_HOME/proc-gc/config.toml)

A process is reaped only when it is an orphan, not run by launchd/systemd,
yours, not a build or test, owned by no live agent session, and has used at
least cpu_percent of a core in every sample for min_hot_minutes. It is re-checked,
then its root is sent SIGTERM alone (never its group); after grace_seconds each
survivor in its tree gets SIGTERM, then SIGKILL.

Exit codes: 0 done; 2 could not run."
    );
}
