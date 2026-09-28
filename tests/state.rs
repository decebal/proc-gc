//! The session line: silent when nothing is wrong, and never silent when the
//! schedule itself has stopped.

use std::time::Duration;

use proc_gc::state::{hook_line, Hot, Reaped, Record, REAP_NEWS, STALE_AFTER};

const NOW: u64 = 1_800_000_000;
const REPORT_AFTER: Duration = Duration::from_secs(15 * 60);

fn fresh() -> Record {
    Record {
        finished_at: NOW - 60,
        ..Record::default()
    }
}

#[test]
fn a_quiet_machine_says_nothing() {
    assert_eq!(hook_line(Some(&fresh()), REPORT_AFTER, NOW), None);
}

#[test]
fn a_missing_errored_or_stale_record_always_speaks() {
    let never = hook_line(None, REPORT_AFTER, NOW).expect("line");
    assert!(never.contains("proc-gc install"), "{never}");

    let errored = Record {
        error: Some("could not read launchctl list".into()),
        ..fresh()
    };
    let line = hook_line(Some(&errored), REPORT_AFTER, NOW).expect("line");
    assert!(line.contains("could not read launchctl list"), "{line}");

    let stale = Record {
        finished_at: NOW - STALE_AFTER.as_secs() - 60,
        ..Record::default()
    };
    let line = hook_line(Some(&stale), REPORT_AFTER, NOW).expect("line");
    assert!(line.contains("not running"), "{line}");
}

#[test]
fn stops_are_news_for_two_hours_and_failures_are_named_apart() {
    let r = Record {
        reaped: vec![
            Reaped {
                pid: 900,
                command: "agent-browser".into(),
                outcome: "Terminated".into(),
                at: NOW - 600,
            },
            Reaped {
                pid: 901,
                command: "spin".into(),
                outcome: "Failed(\"EPERM\")".into(),
                at: NOW - 600,
            },
            Reaped {
                pid: 902,
                command: "old".into(),
                outcome: "Killed".into(),
                at: NOW - REAP_NEWS.as_secs() - 1,
            },
        ],
        ..fresh()
    };
    let line = hook_line(Some(&r), REPORT_AFTER, NOW).expect("line");
    assert!(line.contains("stopped 900 (agent-browser)"), "{line}");
    assert!(line.contains("could not stop 901 (spin)"), "{line}");
    assert!(
        !line.contains("902"),
        "a reap older than the news window is not news: {line}"
    );
}

#[test]
fn a_hot_orphan_is_named_only_after_report_after() {
    let hot = |secs| Record {
        hot: vec![Hot {
            pid: 54120,
            command: "llama-server".into(),
            cores: 1.02,
            hot_secs: secs,
            held: None,
        }],
        ..fresh()
    };
    assert_eq!(hook_line(Some(&hot(300)), REPORT_AFTER, NOW), None);
    let line = hook_line(Some(&hot(20 * 60)), REPORT_AFTER, NOW).expect("line");
    assert!(
        line.contains("pid 54120 (llama-server)") && line.contains("102%"),
        "{line}"
    );
}

#[test]
fn a_hot_orphan_that_is_held_is_named_with_the_reason() {
    let r = Record {
        hot: vec![Hot {
            pid: 810,
            command: "python3".into(),
            cores: 0.95,
            hot_secs: 90 * 60,
            held: Some("owned by live agent session pid 700".into()),
        }],
        ..fresh()
    };
    let line = hook_line(Some(&r), REPORT_AFTER, NOW).expect("line");
    assert!(
        line.contains("not stopped: owned by live agent session pid 700"),
        "{line}"
    );
}

#[test]
fn crowds_are_named() {
    let r = Record {
        crowds: vec![("cn".into(), 9)],
        ..fresh()
    };
    let line = hook_line(Some(&r), REPORT_AFTER, NOW).expect("line");
    assert!(line.contains("9× cn"), "{line}");
}

#[test]
fn the_record_round_trips() {
    let r = Record {
        finished_at: NOW,
        error: None,
        hot: vec![Hot {
            pid: 1,
            command: "a".into(),
            cores: 0.9,
            hot_secs: 60,
            held: Some("build or test (cargo)".into()),
        }],
        reaped: vec![Reaped {
            pid: 2,
            command: "b".into(),
            outcome: "Killed".into(),
            at: NOW,
        }],
        crowds: vec![("c".into(), 5)],
    };
    assert_eq!(Record::from_json(&r.to_json()), Some(r));
}
