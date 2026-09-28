//! An orphan is judged together with everything it started.
//!
//! In the incident this was built for, the orphan was a browser-automation
//! daemon using almost no CPU; its nine headless Chrome trees, children of the
//! daemon rather than orphans themselves, burned eight cores for three days. A
//! rule that looked only at processes with PPID 1 would never have seen them.
//! So heat is summed over the orphan's whole tree, and every member of the
//! tree is checked against the build, agent and hold rules.

use std::collections::{HashMap, HashSet};

use crate::Proc;

/// Indices of `root` and all its descendants, root first.
pub fn members(procs: &[Proc], root: usize) -> Vec<usize> {
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, p) in procs.iter().enumerate() {
        children.entry(p.ppid).or_default().push(i);
    }
    let mut out = vec![root];
    let mut seen: HashSet<u32> = HashSet::from([procs[root].pid]);
    let mut next = 0;
    while next < out.len() {
        let pid = procs[out[next]].pid;
        for &c in children.get(&pid).map_or(&[][..], Vec::as_slice) {
            if seen.insert(procs[c].pid) {
                out.push(c);
            }
        }
        next += 1;
    }
    out
}

/// The `.app` bundle an executable lives in, e.g. `/Applications/Foo.app/`.
pub fn app_bundle(args: &str) -> Option<&str> {
    if !args.starts_with('/') {
        return None;
    }
    let exe = args.split(" -").next().unwrap_or(args);
    let end = exe.find(".app/")? + ".app/".len();
    Some(&exe[..end])
}

/// How many live processes run from each app bundle.
pub fn bundle_counts(procs: &[Proc]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for p in procs {
        if let Some(b) = app_bundle(&p.args) {
            *out.entry(b.to_string()).or_default() += 1;
        }
    }
    out
}
