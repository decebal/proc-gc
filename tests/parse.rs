//! Reading the process table, launchd's job list, lsof and /proc.

use std::path::{Path, PathBuf};

use proc_gc::managed::{cgroup_is_service, parse_launchctl_list};
use proc_gc::owner::{parse_lsof_cwd, same_project};
use proc_gc::reap::parse_probe;
use proc_gc::snapshot::{parse_clock, parse_proc_stat, parse_ps, parse_status_uid};
use proc_gc::tree::{app_bundle, members};

#[test]
fn clock_formats_parse_including_minutes_past_sixty_and_days() {
    assert_eq!(parse_clock("116:43.61"), Some(116.0 * 60.0 + 43.61));
    assert_eq!(
        parse_clock("13-19:37:43"),
        Some(13.0 * 86_400.0 + 19.0 * 3600.0 + 37.0 * 60.0 + 43.0)
    );
    assert_eq!(parse_clock("00:05"), Some(5.0));
    assert_eq!(parse_clock("x"), None);
}

#[test]
fn ps_rows_parse_with_spaces_in_arguments() {
    let out = "    1     0     0 13-19:37:43 116:43.61 /sbin/launchd\n\
               54120     1   501 02-19:20:01   2:58.18 /Applications/Longhand.app/Contents/MacOS/llama-server --port 8087\n\
               garbage row\n";
    let procs = parse_ps(out, 2_000_000);
    assert_eq!(procs.len(), 2);
    let l = &procs[1];
    assert_eq!((l.pid, l.ppid, l.uid), (54120, 1, 501));
    assert_eq!(l.exe_name(), "llama-server");
    assert!((l.cpu - 178.18).abs() < 1e-6);
    assert_eq!(l.start, 2_000_000 - (2 * 86_400 + 19 * 3600 + 20 * 60 + 1));
}

#[test]
fn proc_stat_counts_fields_from_the_last_parenthesis() {
    let stat = "4242 (my (weird) proc) S 1 4242 4242 0 -1 4194560 100 0 0 0 150 50 0 0 20 0 1 0 9000 1000 200";
    assert_eq!(parse_proc_stat(stat), Some((1, 200, 9000)));
    assert_eq!(
        parse_status_uid("Name:\tx\nUid:\t1000\t1000\t1000\t1000\n"),
        Some(1000)
    );
}

#[test]
fn launchctl_list_yields_running_pids_only() {
    let out =
        "PID\tStatus\tLabel\n-\t0\tcom.apple.x\n511\t0\tcom.apple.Finder\n2529\t0\txyz.service\n";
    let pids = parse_launchctl_list(out);
    assert!(pids.contains(&511) && pids.contains(&2529));
    assert_eq!(pids.len(), 2);
}

#[test]
fn a_systemd_service_cgroup_is_managed_and_a_scope_is_not() {
    assert!(cgroup_is_service(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/foo.service\n"
    ));
    assert!(!cgroup_is_service(
        "0::/user.slice/user-1000.slice/session-3.scope\n"
    ));
}

#[test]
fn lsof_cwd_output_maps_pids_to_directories() {
    let out = "p700\nfcwd\nn/Users/me/code/app\np810\nfcwd\nn/Users/me/code/app/sub\n";
    let cwds = parse_lsof_cwd(out);
    assert_eq!(cwds[&700], PathBuf::from("/Users/me/code/app"));
    assert_eq!(cwds[&810], PathBuf::from("/Users/me/code/app/sub"));
}

#[test]
fn same_project_is_containment_but_never_home_or_root() {
    let home = Path::new("/Users/me");
    assert!(same_project(
        Path::new("/Users/me/a/b"),
        Path::new("/Users/me/a"),
        home
    ));
    assert!(!same_project(
        Path::new("/Users/me/a"),
        Path::new("/Users/me/ab"),
        home
    ));
    assert!(!same_project(
        Path::new("/Users/me"),
        Path::new("/Users/me/a"),
        home
    ));
    assert!(!same_project(Path::new("/"), Path::new("/x"), home));
}

#[test]
fn a_probe_reads_parent_start_and_cpu() {
    let p = parse_probe("    1   01:00  0:30.50 R\n", 10_000).expect("probe");
    assert_eq!((p.ppid, p.start, p.zombie), (1, 10_000 - 60, false));
    assert!((p.cpu - 30.5).abs() < 1e-6);
    let z = parse_probe("  900   01:00  0:30.50 Z+\n", 10_000).expect("probe");
    assert!(z.zombie, "an exited, uncollected process counts as gone");
}

#[test]
fn trees_and_bundles() {
    let out = "1 0 0 01:00 0:00 /sbin/launchd\n\
               900 1 501 01:00 0:00 /x/daemon\n\
               901 900 501 01:00 0:00 /x/child\n\
               902 901 501 01:00 0:00 /x/grandchild\n\
               903 1 501 01:00 0:00 /x/other\n";
    let procs = parse_ps(out, 100_000);
    let tree: Vec<u32> = members(&procs, 1).iter().map(|&i| procs[i].pid).collect();
    assert_eq!(tree, vec![900, 901, 902]);
    assert_eq!(
        app_bundle("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --flag"),
        Some("/Applications/Google Chrome.app/")
    );
    assert_eq!(
        app_bundle("node /x/y.app/z"),
        None,
        "only an absolute executable path counts"
    );
}
