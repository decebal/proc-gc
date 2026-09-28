//! The Claude Code skill that ships with the binary.
//!
//! One file, `skills/proc-gc/SKILL.md`, serves both install paths: the plugin
//! reads it from the repository, and `proc-gc skill install` writes the copy
//! compiled into this binary, so the instructions on disk always describe the
//! binary that wrote them.

use std::fs;
use std::path::{Path, PathBuf};

pub const SKILL_MD: &str = include_str!("../skills/proc-gc/SKILL.md");
pub const SKILL_NAME: &str = "proc-gc";

/// `~/.claude/skills/proc-gc`, where Claude Code discovers personal skills.
pub fn default_dir(home: &Path) -> PathBuf {
    home.join(".claude/skills").join(SKILL_NAME)
}

/// Write `SKILL.md` into `dir`, creating it. Returns the file written.
pub fn install(dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("SKILL.md");
    fs::write(&path, SKILL_MD).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}
