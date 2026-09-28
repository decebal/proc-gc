---
name: proc-gc
description: Diagnose and stop CPU outages caused by leaked processes (orphans whose parent died) without killing a build, a service, an app, or an agent session's background work. Use when the machine is slow or hot with nothing obvious running, when load is far above the core count, when asked to find or kill CPU hogs or runaway/orphaned processes, and before running pkill or kill -9 on anything you did not start.
---

# CPU outages from leaked processes

Use the `proc-gc` binary. Do not `pkill` by name or `kill -9` a process you did
not start: the process that looks like a leak is often an agent session's
background test run, a build, or the user's own app, and a name match cannot
tell them apart. `proc-gc` checks who owns a process before anything is
signalled.

If `proc-gc` is not installed: `cargo install proc-gc` (or
`cargo binstall proc-gc`).

## Triage before blaming anything

A single reading is not evidence. A container once read 161% CPU and minutes
later sat at 0.12%, with nothing stopped.

1. Load against cores: `uptime` and `sysctl -n hw.ncpu` (Linux: `nproc`). Load
   far above cores with `%idle` near zero means something is spinning; high load
   with high idle is history, not a hog.
2. Before saying "memory-bound", check pressure, not free RAM: on macOS
   `sysctl kern.memorystatus_vm_pressure_level` (1 is normal) and pageouts in
   `vm_stat`. Near-zero free memory is file cache.
3. Count processes, never grep matches, and read `ps` unfiltered: an
   output-filtering shell proxy once reported 31 processes against a real 1,242
   and truncated the arguments that identified the leak.
4. Say "correlated" unless removing the cause moved the load.

## Steps

1. **See every orphan of yours and what holds it.**

   ```sh
   proc-gc scan
   ```

   Each line is a process whose parent died, judged with every descendant it
   has (`+N`), and the reason it is kept: `build or test (…)`, `owned by live
   agent session pid N`, `an agent session`, `matches hold pattern …`,
   `working directory unreadable, its own or an agent's` (ownership cannot be
   ruled out), or `orphan, no owner found`, the only kind
   that can be reaped. launchd/systemd jobs, system components and parts of
   running apps are counted in the header, not listed. Where samples exist it
   shows the CPU share and for how long it has been hot.

2. **Preview.**

   ```sh
   proc-gc run --dry-run
   ```

3. **Let the schedule act, or run it.** A process is reaped only after it has
   used at least `cpu_percent` of a core in every sample for `min_hot_minutes`
   (default 80% for 60 minutes), so a first run records samples and reaps
   nothing.

   ```sh
   proc-gc run
   ```

   A reap is SIGTERM to the tree's root PID alone, never a process group. After
   `grace_seconds`, each process of the tree still running gets SIGTERM, then
   SIGKILL. The CPU heat is summed over the tree, because a leaked daemon's
   children are what burn.

4. **Report from the record**, `~/.local/state/proc-gc/last-run.json`: hot
   orphans with their CPU share and duration, what was stopped in the last two
   hours and how it ended (`Terminated`, `Killed`, or `Failed`), and crowds of
   five or more orphans of one executable. A run that finds the tree cooled,
   re-parented or already gone at the last check prints `Cooled`, `Adopted` or
   `Vanished` and records nothing.

## When something is hot but held

`scan` names why. Tell the user which process, how hot, for how long, and what
holds it. Do not kill it yourself: a held process is held because it may be
someone's work.

A **crowd** (many short-lived orphans of one executable) is a spawn that leaks
a child per call. Reaping cannot fix it; the program spawning them needs to own
and wait for its children. Report the executable and the count.

## Settings

`~/.config/proc-gc/config.toml` (numbers are quoted strings):

| Key | Default | Meaning |
|---|---|---|
| `min_hot_minutes` | `"60"` | Hot this long before a reap |
| `cpu_percent` | `"80"` | Share of one core required in every interval |
| `max_gap_minutes` | `"20"` | A longer gap between samples restarts the streak |
| `grace_seconds` | `"10"` | SIGTERM to SIGKILL |
| `report_after_minutes` | `"15"` | The session line names a hot orphan after this |
| `build_names` | `[]` | Extra executables never reaped, added to the built-ins (cargo, rustc, cc, ld, make, …) |
| `hold` | `[]` | Argument substrings never reaped |

## The schedule

`proc-gc install` runs `proc-gc run --quiet` every five minutes (launchd
`dev.proc-gc`, or a systemd user timer). `proc-gc hook` prints one line at
session start only when something was stopped recently, an orphan has been hot
past `report_after_minutes`, there is a crowd, or the schedule stopped.

## Limits to state when reporting

- Ownership is inferred: a live agent session in the same project, or an agent
  temp directory in the arguments. An orphan left by a session that has ended,
  in a project where another session is working, is held.
- A short-lived leak (a child that runs seconds per call) is never hot for an
  hour; it shows up only as a crowd.
- macOS and Linux only.
