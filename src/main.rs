//! proc-gc — reap processes whose owner is gone and that have burned CPU for
//! an hour. Argument parsing, the run loop and printing; the decisions live in
//! the library.

mod setup;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

use proc_gc::classify::{self, Context, Verdict};
use proc_gc::config::{self, Settings};
use proc_gc::hot::{self, Member, Sample, Store};
use proc_gc::state::{self, Hot, Reaped, Record};
use proc_gc::{human_minutes, install, managed, owner, reap, snapshot, tree, Proc};

struct Args {
    command: String,
    sub: Option<String>,
    config: Option<PathBuf>,
    dir: Option<PathBuf>,
    dry_run: bool,
    quiet: bool,
    json: bool,
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv);
    let Some(home) = config::home() else {
        eprintln!("proc-gc: HOME is unset or not an absolute path; refusing to guess one");
        exit(2);
    };
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| config::default_path(&home));
    match args.command.as_str() {
        "hook" => setup::hook(&config_path, &home),
        "install" => exit(setup::install_cmd(&config_path, &home, args.dry_run)),
        "uninstall" => exit(setup::uninstall_cmd(&home)),
        "skill" => exit(setup::skill_cmd(
            args.sub.as_deref(),
            args.dir.as_deref(),
            &home,
        )),
        "scan" | "run" => {
            let settings = match Settings::load(&config_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("proc-gc: {e}");
                    exit(2);
                }
            };
            exit(if args.command == "scan" {
                scan(&settings, &home, &args)
            } else {
                run(&settings, &home, &args)
            });
        }
        _ => {
            setup::print_help();
            exit(if args.command == "help" { 0 } else { 2 });
        }
    }
}

fn parse_args(argv: &[String]) -> Args {
    let mut args = Args {
        command: "help".into(),
        sub: None,
        config: None,
        dir: None,
        dry_run: false,
        quiet: false,
        json: false,
    };
    let mut positional = 0;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => args.config = it.next().map(PathBuf::from),
            "--dir" => args.dir = it.next().map(PathBuf::from),
            "--dry-run" => args.dry_run = true,
            "--quiet" => args.quiet = true,
            "--json" => args.json = true,
            "-h" | "--help" => {
                args.command = "help".into();
                return args;
            }
            other if !other.starts_with('-') => {
                if positional == 0 {
                    args.command = other.to_string();
                } else {
                    args.sub = Some(other.to_string());
                }
                positional += 1;
            }
            other => {
                eprintln!("proc-gc: unknown flag {other}");
                exit(2);
            }
        }
    }
    args
}

/// One orphan's tree and its verdict.
struct Judged {
    members: Vec<usize>,
    verdict: Verdict,
}

/// One classified view of the machine.
struct Survey {
    now: u64,
    procs: Vec<Proc>,
    orphan_parents: HashSet<u32>,
    judged: Vec<Judged>,
}

impl Survey {
    fn root(&self, j: &Judged) -> &Proc {
        &self.procs[j.members[0]]
    }
}

fn survey(settings: &Settings, home: &Path) -> Result<Survey, String> {
    let now = state::now_secs();
    let procs = snapshot::take(now).ok_or("could not read the process table")?;
    let me: u32 = install::uid()
        .and_then(|u| u.parse().ok())
        .ok_or("could not read the user id")?;
    let orphan_parents = classify::orphan_parents(&procs, me);
    let roots: Vec<usize> = (0..procs.len())
        .filter(|&i| orphan_parents.contains(&procs[i].ppid) && procs[i].pid > 1)
        .filter(|&i| procs[i].uid == me)
        .collect();
    let managed = if snapshot::is_linux() {
        let pids: Vec<u32> = roots.iter().map(|&i| procs[i].pid).collect();
        managed::linux_service_pids(Path::new("/proc"), &pids)
    } else {
        managed::launchd_pids()
            .ok_or("could not read launchctl list, so nothing can be proven unmanaged")?
    };
    let trees: Vec<Vec<usize>> = roots.iter().map(|&r| tree::members(&procs, r)).collect();
    let agent_pids: Vec<u32> = procs
        .iter()
        .filter(|p| owner::is_agent(p))
        .map(|p| p.pid)
        .collect();
    let mut want: HashSet<u32> = agent_pids.iter().copied().collect();
    want.extend(trees.iter().flatten().map(|&i| procs[i].pid));
    let cwds = owner::cwds(&want.into_iter().collect::<Vec<_>>());
    let agents: Vec<(u32, Option<PathBuf>)> = agent_pids
        .iter()
        .map(|pid| (*pid, cwds.get(pid).cloned()))
        .collect();
    let bundles = tree::bundle_counts(&procs);
    let cx = Context {
        me,
        home,
        managed: &managed,
        bundles: &bundles,
        agents: &agents,
        cwds: &cwds,
        build_names: &settings.build_names,
        holds: &settings.holds,
    };
    let judged = trees
        .into_iter()
        .map(|members| Judged {
            verdict: classify::classify(&procs, &members, &cx),
            members,
        })
        .collect();
    Ok(Survey {
        now,
        procs,
        orphan_parents,
        judged,
    })
}

fn short(args: &str) -> String {
    let s: String = args.chars().take(90).collect();
    if args.chars().count() > 90 {
        format!("{s}…")
    } else {
        s
    }
}

/// Orphans that no supervisor or installed app accounts for, grouped by
/// executable, groups of five or more: the shape of a spawn that leaks a child
/// per call.
fn crowds(s: &Survey) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for j in &s.judged {
        if !matches!(
            j.verdict,
            Verdict::Managed | Verdict::NotMine | Verdict::System | Verdict::App(_)
        ) {
            *counts.entry(s.root(j).exe_name().to_string()).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().filter(|(_, c)| *c >= 5).collect();
    out.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    out
}

fn last_rate(samples: &[Sample]) -> f64 {
    match samples {
        [.., a, b] if b.at > a.at => (b.cpu - a.cpu) / (b.at - a.at) as f64,
        _ => 0.0,
    }
}

fn scan(settings: &Settings, home: &Path, args: &Args) -> i32 {
    let mut s = match survey(settings, home) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("proc-gc: {e}");
            return 2;
        }
    };
    let store = state::read(&config::state_dir(home), state::SAMPLES)
        .map(|v| Store::from_json(&v))
        .unwrap_or_default();
    store.settle_starts(&mut s.procs);
    let hidden = |v: &Verdict| {
        matches!(
            v,
            Verdict::Managed | Verdict::NotMine | Verdict::System | Verdict::App(_)
        )
    };
    let mut rows: Vec<(&Judged, u64, f64)> = s
        .judged
        .iter()
        .map(|j| {
            let samples = store
                .tracks
                .get(&s.root(j).key())
                .map_or(&[][..], Vec::as_slice);
            let hot = hot::hot_for(samples, settings.cores, settings.max_gap.as_secs());
            (j, hot, last_rate(samples))
        })
        .filter(|(j, hot, _)| !hidden(&j.verdict) || *hot > 0)
        .collect();
    rows.sort_by_key(|(j, hot, _)| (j.verdict != Verdict::Candidate, std::cmp::Reverse(*hot)));
    if args.json {
        let items: Vec<_> = rows
            .iter()
            .map(|(j, hot, rate)| {
                let root = s.root(j);
                serde_json::json!({ "pid": root.pid, "tree_size": j.members.len(),
                    "verdict": j.verdict.describe(), "hot_secs": hot, "cores": rate,
                    "command": root.args })
            })
            .collect();
        println!("{}", serde_json::json!({ "orphans": items }));
        return 0;
    }
    let hidden_count = s.judged.len() - rows.len();
    println!(
        "{} processes; {} orphan trees of yours shown ({hidden_count} more are launchd jobs, \
         system components or parts of running apps).",
        s.procs.len(),
        rows.len(),
    );
    for (j, hot, rate) in &rows {
        let heat = if *hot > 0 {
            format!(
                "{:>4.0}% for {}",
                rate * 100.0,
                human_minutes(Duration::from_secs(*hot))
            )
        } else {
            "    -".to_string()
        };
        let root = s.root(j);
        println!(
            "  {:>7} +{:<3} {heat:<16}  {:<36}  {}",
            root.pid,
            j.members.len() - 1,
            j.verdict.describe(),
            short(&root.args)
        );
    }
    0
}

fn run(settings: &Settings, home: &Path, args: &Args) -> i32 {
    let state_dir = config::state_dir(home);
    let Some(_lock) = run_lock(&state_dir) else {
        return 0;
    };
    let mut s = match survey(settings, home) {
        Ok(s) => s,
        Err(e) => return fail(&state_dir, &e),
    };
    let mut store = state::read(&state_dir, state::SAMPLES)
        .map(|v| Store::from_json(&v))
        .unwrap_or_default();
    store.settle_starts(&mut s.procs);
    let tracked: Vec<&Judged> = s
        .judged
        .iter()
        .filter(|j| !matches!(j.verdict, Verdict::Managed | Verdict::NotMine))
        .collect();
    for j in &tracked {
        let members: Vec<Member> = j
            .members
            .iter()
            .map(|&i| Member {
                key: s.procs[i].key(),
                start: s.procs[i].start,
                cpu: s.procs[i].cpu,
            })
            .collect();
        store.observe(&s.root(j).key(), s.now, &members);
    }
    let keep = (settings.min_hot + settings.max_gap * 2).as_secs();
    let alive: Vec<String> = tracked.iter().map(|j| s.root(j).key()).collect();
    store.prune(&alive, s.now, keep);
    let candidates = tracked
        .iter()
        .filter(|j| j.verdict == Verdict::Candidate)
        .count();

    let previous = state::read(&state_dir, state::RECORD).and_then(|v| Record::from_json(&v));
    let mut reaped: Vec<Reaped> = previous
        .map(|r| r.reaped)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| s.now.saturating_sub(r.at) <= state::REAP_NEWS.as_secs())
        .collect();
    let mut hot_list = Vec::new();
    let parents: Vec<u32> = s.orphan_parents.iter().copied().collect();
    for j in &tracked {
        let p = s.root(j);
        let samples = store.tracks.get(&p.key()).map_or(&[][..], Vec::as_slice);
        let hot_secs = hot::hot_for(samples, settings.cores, settings.max_gap.as_secs());
        if hot_secs == 0 {
            continue;
        }
        let held = (j.verdict != Verdict::Candidate).then(|| j.verdict.describe());
        hot_list.push(Hot {
            pid: p.pid,
            command: p.exe_name().to_string(),
            cores: last_rate(samples),
            hot_secs,
            held: held.clone(),
        });
        if held.is_some() {
            continue;
        }
        let due = hot_secs >= settings.min_hot.as_secs();
        if args.dry_run && due {
            println!(
                "would reap {} and {} descendant(s) ({}): hot for {}",
                p.pid,
                j.members.len() - 1,
                short(&p.args),
                human_minutes(Duration::from_secs(hot_secs))
            );
        }
        if !due || args.dry_run {
            continue;
        }
        let members: Vec<(u32, u64)> = j
            .members
            .iter()
            .map(|&i| (s.procs[i].pid, s.procs[i].start))
            .collect();
        let outcome = reap::reap(
            &members,
            &parents,
            settings.grace,
            &state::now_secs,
            &|rate| rate >= settings.cores,
        );
        println!("{:?}  {} ({})", outcome, p.pid, short(&p.args));
        if !matches!(
            outcome,
            reap::Outcome::Terminated | reap::Outcome::Killed | reap::Outcome::Failed(_)
        ) {
            continue;
        }
        reaped.push(Reaped {
            pid: p.pid,
            command: p.exe_name().to_string(),
            outcome: format!("{outcome:?}"),
            at: state::now_secs(),
        });
    }
    let record = Record {
        finished_at: state::now_secs(),
        error: None,
        hot: hot_list,
        reaped,
        crowds: crowds(&s),
    };
    if !args.dry_run {
        let saved = state::write(&state_dir, state::SAMPLES, &store.to_json())
            .and_then(|()| state::write(&state_dir, state::RECORD, &record.to_json()));
        if let Err(e) = saved {
            eprintln!("proc-gc: could not write state: {e}");
        }
    }
    if args.json {
        println!("{}", record.to_json());
    } else if !args.quiet {
        println!(
            "proc-gc: {} candidates, {} hot, {} reaped in the last 2h{}",
            candidates,
            record.hot.len(),
            record.reaped.len(),
            if args.dry_run {
                " (dry run: nothing recorded or signalled)"
            } else {
                ""
            }
        );
    }
    0
}

fn run_lock(state_dir: &Path) -> Option<File> {
    fs::create_dir_all(state_dir).ok()?;
    let file = File::create(state_dir.join("run.lock")).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn fail(state_dir: &Path, error: &str) -> i32 {
    eprintln!("proc-gc: {error}");
    let record = Record {
        finished_at: state::now_secs(),
        error: Some(error.to_string()),
        ..Record::default()
    };
    let _ = state::write(state_dir, state::RECORD, &record.to_json());
    2
}
