//! Who may be reaped, built from the incidents this tool exists for and the
//! false kills a simpler rule made elsewhere.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use proc_gc::classify::{classify, orphan_parents, Context, Verdict, DEFAULT_BUILD_NAMES};
use proc_gc::tree::{bundle_counts, members};
use proc_gc::Proc;

const ME: u32 = 501;

fn p(pid: u32, ppid: u32, args: &str) -> Proc {
    Proc {
        pid,
        ppid,
        uid: ME,
        start: 1_000,
        cpu: 0.0,
        args: args.to_string(),
    }
}

struct World {
    procs: Vec<Proc>,
    managed: HashSet<u32>,
    cwds: HashMap<u32, PathBuf>,
    holds: Vec<String>,
}

impl World {
    fn new(procs: Vec<Proc>) -> World {
        World {
            procs,
            managed: HashSet::new(),
            cwds: HashMap::new(),
            holds: Vec::new(),
        }
    }

    fn cwd(mut self, pid: u32, dir: &str) -> World {
        self.cwds.insert(pid, PathBuf::from(dir));
        self
    }

    fn judge(&self, root_pid: u32) -> Verdict {
        let agents: Vec<(u32, Option<PathBuf>)> = self
            .procs
            .iter()
            .filter(|p| proc_gc::owner::is_agent(p))
            .map(|p| (p.pid, self.cwds.get(&p.pid).cloned()))
            .collect();
        let bundles = bundle_counts(&self.procs);
        let build_names: Vec<String> = DEFAULT_BUILD_NAMES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let cx = Context {
            me: ME,
            home: Path::new("/Users/me"),
            managed: &self.managed,
            bundles: &bundles,
            agents: &agents,
            cwds: &self.cwds,
            build_names: &build_names,
            holds: &self.holds,
        };
        let root = self
            .procs
            .iter()
            .position(|p| p.pid == root_pid)
            .expect("root");
        classify(&self.procs, &members(&self.procs, root), &cx)
    }
}

/// The 2026-09-18 incident: a browser-automation daemon outlived its session;
/// its headless Chrome children, not orphans themselves, burned the CPU.
#[test]
fn a_leaked_daemon_and_its_browser_tree_is_a_candidate() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(900, 1, "/Users/me/.hermes/node_modules/agent-browser/bin/agent-browser-darwin-arm64 daemon"),
        p(901, 900, "/Users/me/.cache/agent-browser/Chrome for Testing.app/Contents/MacOS/Chrome for Testing --headless=new"),
        p(902, 901, "/Users/me/.cache/agent-browser/Chrome for Testing.app/Contents/Frameworks/Helper --type=gpu-process"),
    ])
    .cwd(900, "/");
    assert_eq!(w.judge(900), Verdict::Candidate);
}

/// The false kills elsewhere: background test runs whose wrapper shell had
/// exited, while the agent session that started them was still working.
#[test]
fn an_agent_sessions_reparented_background_work_is_held() {
    let pytest = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(700, 650, "/Users/me/.local/bin/claude"),
        p(810, 1, "/usr/local/bin/python3 -m pytest tests/"),
    ])
    .cwd(700, "/Users/me/code/app")
    .cwd(810, "/Users/me/code/app");
    assert!(matches!(pytest.judge(810), Verdict::Build(_)));

    let script = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(700, 650, "/Users/me/.local/bin/claude"),
        p(811, 1, "node /Users/me/code/app/scripts/crawl.mjs"),
    ])
    .cwd(700, "/Users/me/code/app")
    .cwd(811, "/Users/me/code/app/scripts");
    assert_eq!(script.judge(811), Verdict::AgentOwned(700));

    let scratch = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(700, 650, "/Users/me/.local/bin/claude"),
        p(
            812,
            1,
            "node /private/tmp/claude-501/app/session/scratchpad/job.mjs",
        ),
    ])
    .cwd(700, "/Users/me/code/app")
    .cwd(812, "/");
    assert_eq!(scratch.judge(812), Verdict::AgentOwned(700));
}

#[test]
fn builds_anywhere_in_the_tree_are_held() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(500, 1, "/bin/sh -c make release"),
        p(501, 500, "/Users/me/.cargo/bin/cargo build --release"),
    ])
    .cwd(500, "/Users/me/code/x");
    assert!(matches!(w.judge(500), Verdict::Build(_)));
}

/// launchd starts XPC services and extensions without listing them in
/// `launchctl list`; 128 of them looked like candidates before this rule.
#[test]
fn system_components_are_held_even_when_launchctl_does_not_list_them() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(12503, 1, "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent"),
        p(73540, 1, "/Users/me/Library/Application Support/Setapp/LaunchAgents/Setapp.app/Contents/MacOS/Agent"),
    ]);
    assert_eq!(w.judge(12503), Verdict::System);
    assert_eq!(w.judge(73540), Verdict::System);
}

#[test]
fn a_browser_downloaded_into_library_caches_is_not_a_system_component() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(840, 1, "/Users/me/Library/Caches/ms-playwright/chromium-1140/chrome-mac/Chromium.app/Contents/MacOS/Chromium --headless"),
    ])
    .cwd(840, "/");
    assert_eq!(w.judge(840), Verdict::Candidate);
}

#[test]
fn a_shell_or_interpreter_from_the_system_bin_dirs_is_not_a_system_component() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(830, 1, "/bin/sh -c while :; do :; done"),
        p(831, 1, "/usr/bin/python3 spin.py"),
    ])
    .cwd(830, "/Users/me/tmp")
    .cwd(831, "/Users/me/tmp");
    assert_eq!(w.judge(830), Verdict::Candidate);
    assert_eq!(w.judge(831), Verdict::Candidate);
}

#[test]
fn an_app_helper_is_held_while_its_app_runs_and_not_after() {
    let running = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(475, 1, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        p(1290, 1, "/Applications/Google Chrome.app/Contents/Frameworks/Helpers/chrome_crashpad_handler --database=x"),
    ])
    .cwd(1290, "/");
    assert_eq!(
        running.judge(1290),
        Verdict::App("/Applications/Google Chrome.app/".into())
    );

    let gone = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(
            54120,
            1,
            "/Applications/Longhand.app/Contents/MacOS/llama-server --port 8087",
        ),
    ])
    .cwd(54120, "/");
    assert_eq!(gone.judge(54120), Verdict::Candidate);
}

#[test]
fn managed_foreign_held_and_unknown_are_never_candidates() {
    let mut w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(20, 1, "/opt/homebrew/bin/some-agent"),
        Proc {
            uid: 0,
            ..p(21, 1, "/opt/homebrew/bin/rootd")
        },
        p(22, 1, "/opt/homebrew/bin/keepme --serve"),
        p(23, 1, "/opt/homebrew/bin/mystery"),
    ]);
    w.managed.insert(20);
    w.holds.push("keepme".into());
    w.cwds.insert(22, PathBuf::from("/"));
    assert_eq!(w.judge(20), Verdict::Managed);
    assert_eq!(w.judge(21), Verdict::NotMine);
    assert_eq!(w.judge(22), Verdict::Held("keepme".into()));
    assert_eq!(
        w.judge(23),
        Verdict::Unknown,
        "no cwd must not read as no owner"
    );
}

#[test]
fn home_and_root_cwds_are_too_broad_to_prove_ownership() {
    let w = World::new(vec![
        p(1, 0, "/sbin/launchd"),
        p(700, 650, "/Users/me/.local/bin/claude"),
        p(820, 1, "/opt/homebrew/bin/spinner"),
    ])
    .cwd(700, "/Users/me")
    .cwd(820, "/Users/me/elsewhere");
    assert_eq!(w.judge(820), Verdict::Candidate);
}

#[test]
fn a_user_systemd_is_an_orphan_parent_on_linux() {
    let procs = vec![
        p(1, 0, "/sbin/init"),
        p(1200, 1, "/usr/lib/systemd/systemd --user"),
        Proc {
            uid: 0,
            ..p(1300, 1, "/usr/lib/systemd/systemd --user")
        },
    ];
    let parents = orphan_parents(&procs, ME);
    assert!(parents.contains(&1) && parents.contains(&1200));
    assert!(
        !parents.contains(&1300),
        "another user's subreaper is not ours"
    );
}
