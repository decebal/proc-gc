//! Is this orphan's tree a candidate, and if not, what holds it? Pure: the
//! caller gathers the process table, the managed set, the agent sessions and
//! the working directories, and every rule is testable from fixtures.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::owner::{is_agent, mentions_agent_temp, same_project};
use crate::tree::app_bundle;
use crate::Proc;

/// Why an orphan is left alone, or `Candidate` when nothing holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Another user's (or root's) process.
    NotMine,
    /// launchd lists it, or systemd runs it as a service.
    Managed,
    /// An operating-system or support component: launchd starts XPC services,
    /// extensions and agents from these locations without listing them in
    /// `launchctl list`.
    System,
    /// Part of an installed app that is still running (a crash handler, a
    /// helper). Once no other process from the bundle is alive, it is not
    /// held on this ground.
    App(String),
    /// An agent CLI itself.
    Agent,
    /// A build, a compiler, a test runner, or a binary run from a build dir,
    /// anywhere in the tree.
    Build(String),
    /// A live agent session works in the tree's project, or the tree names an
    /// agent temp directory while an agent is running.
    AgentOwned(u32),
    /// Matches a configured hold pattern, anywhere in the tree.
    Held(String),
    /// The root's working directory could not be read, so ownership cannot be
    /// ruled out.
    Unknown,
    Candidate,
}

impl Verdict {
    pub fn describe(&self) -> String {
        match self {
            Verdict::NotMine => "another user's process".into(),
            Verdict::Managed => "run by launchd/systemd".into(),
            Verdict::System => "system or support component".into(),
            Verdict::App(b) => format!("part of running app {b}"),
            Verdict::Agent => "an agent session".into(),
            Verdict::Build(why) => format!("build or test ({why})"),
            Verdict::AgentOwned(pid) => format!("owned by live agent session pid {pid}"),
            Verdict::Held(p) => format!("matches hold pattern {p:?}"),
            Verdict::Unknown => "working directory unreadable".into(),
            Verdict::Candidate => "orphan, no owner found".into(),
        }
    }
}

/// Where operating-system and support components live. launchd starts these
/// as their own group leaders with PPID 1, indistinguishable by lineage from
/// a leak. `/bin/` and `/usr/bin/` are absent on purpose: shells and
/// interpreters live there, and a leaked loop usually runs through one.
const SYSTEM_ROOTS: [&str; 6] = [
    "/System/",
    "/Library/",
    "/usr/libexec/",
    "/usr/lib/",
    "/usr/sbin/",
    "/sbin/",
];

/// Argument fragments that mark a build or a test run.
const BUILD_ARG_MARKERS: [&str; 7] = [
    "/target/debug/",
    "/target/release/",
    "/deps/",
    "pytest",
    "vitest",
    "jest",
    "nextest",
];

pub struct Context<'a> {
    pub me: u32,
    pub home: &'a Path,
    pub managed: &'a HashSet<u32>,
    /// Live processes per `.app` bundle.
    pub bundles: &'a HashMap<String, usize>,
    /// Live agent sessions and their working directories.
    pub agents: &'a [(u32, Option<PathBuf>)],
    pub cwds: &'a HashMap<u32, PathBuf>,
    pub build_names: &'a [String],
    pub holds: &'a [String],
}

/// `~/Library/Caches/` is excluded: tools download browsers there (Playwright
/// on macOS), and a leaked headless browser is exactly what this is for.
fn is_system(p: &Proc, home: &Path) -> bool {
    let user_library = format!("{}/Library/", home.display());
    let user_caches = format!("{}/Library/Caches/", home.display());
    SYSTEM_ROOTS.iter().any(|r| p.args.starts_with(r))
        || (p.args.starts_with(&user_library) && !p.args.starts_with(&user_caches))
}

fn build_marker(p: &Proc, build_names: &[String]) -> Option<String> {
    if let Some(n) = build_names.iter().find(|n| p.exe_name() == n.as_str()) {
        return Some(n.clone());
    }
    BUILD_ARG_MARKERS
        .iter()
        .find(|m| p.args.contains(*m))
        .map(|m| (*m).to_string())
}

/// Judge the tree rooted at `procs[members[0]]`.
pub fn classify(procs: &[Proc], members: &[usize], cx: &Context<'_>) -> Verdict {
    let root = &procs[members[0]];
    if root.uid != cx.me {
        return Verdict::NotMine;
    }
    if cx.managed.contains(&root.pid) {
        return Verdict::Managed;
    }
    if is_system(root, cx.home) {
        return Verdict::System;
    }
    if let Some(bundle) = app_bundle(&root.args) {
        if cx.bundles.get(bundle).copied().unwrap_or(0) > 1 {
            return Verdict::App(bundle.to_string());
        }
    }
    let tree: Vec<&Proc> = members.iter().map(|&i| &procs[i]).collect();
    if tree.iter().any(|p| is_agent(p)) {
        return Verdict::Agent;
    }
    if let Some(why) = tree.iter().find_map(|p| build_marker(p, cx.build_names)) {
        return Verdict::Build(why);
    }
    if let Some(pattern) = cx
        .holds
        .iter()
        .find(|h| !h.is_empty() && tree.iter().any(|p| p.args.contains(h.as_str())))
    {
        return Verdict::Held(pattern.clone());
    }
    if tree.iter().any(|p| mentions_agent_temp(p)) {
        if let Some((agent, _)) = cx.agents.first() {
            return Verdict::AgentOwned(*agent);
        }
    }
    let Some(root_cwd) = cx.cwds.get(&root.pid) else {
        return Verdict::Unknown;
    };
    let member_cwds = tree.iter().filter_map(|p| cx.cwds.get(&p.pid));
    for cwd in std::iter::once(root_cwd).chain(member_cwds) {
        if let Some((agent, _)) = cx
            .agents
            .iter()
            .find(|(_, acwd)| acwd.as_ref().is_some_and(|a| same_project(cwd, a, cx.home)))
        {
            return Verdict::AgentOwned(*agent);
        }
    }
    Verdict::Candidate
}

/// PID 1 and, on Linux, the user's `systemd --user` processes: the parents an
/// orphan is reparented to.
pub fn orphan_parents(procs: &[Proc], me: u32) -> HashSet<u32> {
    let mut out: HashSet<u32> = HashSet::from([1]);
    out.extend(
        procs
            .iter()
            .filter(|p| p.uid == me && p.exe_name() == "systemd" && p.args.contains("--user"))
            .map(|p| p.pid),
    );
    out
}

pub const DEFAULT_BUILD_NAMES: [&str; 22] = [
    "cargo",
    "rustc",
    "rustdoc",
    "clippy-driver",
    "rust-analyzer",
    "sccache",
    "cargo-nextest",
    "cc",
    "c++",
    "clang",
    "clang++",
    "gcc",
    "g++",
    "ld",
    "ld64",
    "lld",
    "mold",
    "wild",
    "make",
    "ninja",
    "cmake",
    "xcodebuild",
];
