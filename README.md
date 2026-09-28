# proc-gc

Stop orphaned processes that keep burning CPU after the session that started
them is gone, without touching builds, services, apps, or an agent's
background work.

The case it exists for: a browser-automation daemon outlived the agent session
that started it. Nine headless Chrome trees, 92 processes, took ~810% CPU (8 of
12 cores) for up to three days while every task list said nothing was running.
The hot processes were **children** of the leaked daemon, not orphans
themselves, so a rule that only looks at PPID-1 processes misses them. proc-gc
judges each orphan together with its whole tree.

```
cargo install proc-gc
proc-gc scan          # every orphan of yours and what holds it; changes nothing
proc-gc install       # run every five minutes (launchd on macOS, systemd on Linux)
```

For Claude Code, the plugin adds the skill and a session-start line:

```
/plugin marketplace add decebal/proc-gc
/plugin install proc-gc@proc-gc
```

Without the plugin, `proc-gc skill install` writes the skill to
`~/.claude/skills/proc-gc/`.

## When a process is stopped

Every condition must hold, and each is pinned by a test:

1. **It is an orphan.** Its parent died and it was reparented to init, or on
   Linux to a `systemd --user` subreaper.
2. **Nobody manages it.** launchd lists it, systemd runs it as a service, it
   belongs to another user, it runs from a system location (`/System/`,
   `/Library/`, `/usr/libexec/`, `/usr/lib/`, `/usr/sbin/`, `/sbin/`,
   `~/Library/` except `~/Library/Caches/`, where Playwright keeps its
   browsers), or it is part of an `.app` that still has another process
   running. On macOS every app, agent and XPC service also has PPID 1, and
   `launchctl list` does not list XPC services or app helpers; without the last
   two rules 128 of them looked like leaks on one machine.
3. **It is not a build or a test.** Compilers, linkers, `cargo`, `make`,
   test runners, and anything running from a Cargo `target/` directory,
   anywhere in the tree. Your own names go in `build_names`.
4. **No live agent session owns it.** A Claude Code or Codex process whose
   working directory contains the tree's, or a tree whose arguments name a live
   session's temp directory, holds it. If a working directory cannot be read,
   ownership cannot be ruled out and the tree is held.
5. **It has burned CPU for an hour.** At least `cpu_percent` of one core in
   **every** interval between samples, for `min_hot_minutes`, with no gap longer
   than `max_gap_minutes`. Samples are taken by the scheduled runs and kept
   between them, keyed by PID and start time. One sample is never enough. The
   tree's work only grows: CPU gained by members seen at the last sample, plus
   all CPU of members started since, so a daemon that replaces its hot workers
   does not read as cooling.

Then it is checked again (same start time, still an orphan, still hot over a
fresh three-second sample) and its root is sent SIGTERM, alone. After
`grace_seconds`, each process still alive in the tree gets SIGTERM, then
SIGKILL. No signal ever goes to a process group. Whether it worked is read back
from the process table, not from `kill`'s exit status.

Every orphan tree of yours is sampled except launchd and systemd jobs. One that
stays hot while something holds it is reported, in the record and the session
line, with the reason it was not stopped.

## Commands

| Command | Does |
|---|---|
| `proc-gc scan [--json]` | Classify every orphan tree of yours; show heat from recorded samples. Read-only. |
| `proc-gc run [--dry-run] [--quiet] [--json]` | Sample CPU, record it, stop what has been hot long enough. `--dry-run` records nothing and signals nothing. |
| `proc-gc hook` | One line for a session start, only when something was stopped, is hot, crowds up, or the schedule stopped running. |
| `proc-gc install [--dry-run]` / `uninstall` | Add or remove the five-minute schedule. |
| `proc-gc skill [install] [--dir <d>]` | Print the Claude Code skill, or write it. |

Exit codes: 0 done, 2 could not run. State lives in
`$XDG_STATE_HOME/proc-gc/` (default `~/.local/state/proc-gc/`): `last-run.json`
and `samples.json`.

## Settings

`~/.config/proc-gc/config.toml` (or `$PROC_GC_CONFIG`, or `--config`).
[`config.example.toml`](config.example.toml) is the file `install` writes, and it
equals the defaults.

| Key | Default | Meaning |
|---|---|---|
| `min_hot_minutes` | 60 | Hot this long, across runs, before a stop |
| `cpu_percent` | 80 | Of one core, in every interval |
| `max_gap_minutes` | 20 | A longer gap between samples restarts the count |
| `grace_seconds` | 10 | SIGTERM to SIGKILL, at most 60 |
| `report_after_minutes` | 15 | The session line names a hot orphan after this |
| `build_names` | built-ins | Extra executable names that count as builds; added to the built-ins, never replacing them |
| `hold` | `[]` | Command-line substrings never stopped |

## What it does not do

- **Short-lived leaks.** A spawn that leaks a child per call, where each child
  lives seconds, never reaches an hour of heat. proc-gc reports these as
  crowds: five or more orphans of one executable. It does not stop them.
- **Memory pressure.** It reads CPU only. On Linux, earlyoom and nohang handle
  memory.
- **Other users' processes, root's processes, or anything under a supervisor.**
- **Windows.** macOS and Linux only.

## Prior art

- **[cc-reaper](https://github.com/theQuert/cc-reaper)** reaped PPID-1
  processes at 80% CPU for three minutes. All six kills in one published log
  were pytest runs and scratch scripts, background work that Claude Code
  reparents to launchd when its wrapper shell exits, and the pass was removed.
  A group signal in the same project once took out a desktop app and the
  terminal. proc-gc takes three things from it: hold anything a live session
  owns, require an hour rather than minutes, and signal one PID.
- **[proc-janitor](https://github.com/jhlee0409/proc-janitor)** cleans PPID-1
  processes by pattern, with a start-time identity check and a dead-session
  test. It has no CPU criterion; it answers "which orphans match my list", and
  proc-gc answers "which orphan is burning the machine".

## License

MIT
