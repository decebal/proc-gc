//! The skill, the plugin and the binary must describe the same tool.
//!
//! The skill tells an agent which commands to run. A command it names that the
//! binary does not have is an instruction that fails in front of the user, so
//! every `proc-gc <command>` and every `--flag` in the skill's code spans is
//! checked against the real binary's help.

use std::path::Path;
use std::process::Command;

use proc_gc::skill::{install, SKILL_MD};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_proc-gc");
const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn help() -> String {
    let out = Command::new(BIN).arg("help").output().expect("run binary");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Text inside fenced blocks and inline backticks.
fn code_spans(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, block) in md.split("```").enumerate() {
        if i % 2 == 1 {
            out.push(block.to_string());
            continue;
        }
        for (j, span) in block.split('`').enumerate() {
            if j % 2 == 1 {
                out.push(span.to_string());
            }
        }
    }
    out
}

#[test]
fn every_command_the_skill_names_exists() {
    let help = help();
    let mut checked = 0;
    for span in code_spans(SKILL_MD) {
        let words: Vec<&str> = span.split_whitespace().collect();
        for pair in words.windows(2) {
            if pair[0] == "proc-gc" && pair[1].chars().all(|c| c.is_ascii_lowercase()) {
                let usage = format!("proc-gc {}", pair[1]);
                assert!(
                    help.contains(&usage),
                    "skill names `{usage}`, help has no such command"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 3,
        "found only {checked} commands in the skill; is the parse broken?"
    );
}

#[test]
fn every_flag_the_skill_names_exists() {
    let help = help();
    let flags: Vec<String> = code_spans(SKILL_MD)
        .iter()
        .flat_map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|w| w.starts_with("--"))
        .map(|w| {
            w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .collect();
    assert!(
        !flags.is_empty(),
        "no flags found in the skill; is the parse broken?"
    );
    for flag in flags {
        assert!(help.contains(&flag), "skill names {flag}, help does not");
    }
}

#[test]
fn skill_install_writes_the_file_the_plugin_ships() {
    let dir = std::env::temp_dir().join(format!("proc-gc-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let written = install(&dir).expect("install skill");
    let shipped = Path::new(ROOT).join("skills/proc-gc/SKILL.md");
    assert_eq!(
        std::fs::read(&written).expect("read written"),
        std::fs::read(&shipped).expect("read shipped")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn json(rel: &str) -> Value {
    let text = std::fs::read_to_string(Path::new(ROOT).join(rel)).expect(rel);
    serde_json::from_str(&text).expect(rel)
}

/// The plugin, its marketplace entry and the crate are released together; a
/// version skew means an installed skill describes a different binary.
#[test]
fn plugin_marketplace_and_crate_versions_agree() {
    let crate_version = env!("CARGO_PKG_VERSION");
    let plugin = json(".claude-plugin/plugin.json");
    let market = json(".claude-plugin/marketplace.json");
    assert_eq!(plugin["name"], "proc-gc");
    assert_eq!(plugin["version"], crate_version);
    assert_eq!(market["plugins"][0]["name"], "proc-gc");
    assert_eq!(market["plugins"][0]["source"], "./");
    assert_eq!(market["plugins"][0]["version"], crate_version);
}

#[test]
fn the_plugin_hook_runs_the_session_line() {
    let hooks = json("hooks/hooks.json");
    let command = hooks["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .expect("command");
    assert!(
        command.contains("\" hook"),
        "hook does not call `proc-gc hook`: {command}"
    );
    assert!(
        command.contains("cargo install proc-gc"),
        "no install hint: {command}"
    );
}
