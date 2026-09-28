//! An hour of heat, measured across runs, never inferred from one sample.

use proc_gc::hot::{hot_for, Member, Sample, Store};
use proc_gc::Proc;

const MIN: u64 = 60;

/// A start derived from elapsed time moves a second between runs; the
/// process must keep its key or its history is pruned as gone.
#[test]
fn a_start_that_wobbles_keeps_the_key_on_record() {
    let mut store = Store::default();
    store.observe(
        "900:1000",
        10_000,
        &[m("900:1000", 1_000, 5.0), m("901:1500", 1_500, 1.0)],
    );
    let proc = |pid, start| Proc {
        pid,
        ppid: 1,
        uid: 501,
        start,
        cpu: 0.0,
        args: String::new(),
    };
    let mut procs = vec![
        proc(900, 1_001),
        proc(901, 1_499),
        proc(902, 1_000),
        proc(900, 5_000),
    ];
    store.settle_starts(&mut procs);
    let starts: Vec<u64> = procs.iter().map(|p| p.start).collect();
    assert_eq!(
        starts,
        vec![1_000, 1_500, 1_000, 5_000],
        "902 is another pid; 900 at 5000 is a reused pid"
    );
}

fn steady(cores: f64, minutes: u64, every: u64) -> Vec<Sample> {
    (0..=minutes / every)
        .map(|i| {
            let at = 10_000 + i * every * MIN;
            Sample {
                at,
                cpu: cores * (at - 10_000) as f64,
            }
        })
        .collect()
}

#[test]
fn an_hour_at_a_full_core_is_an_hour_hot() {
    assert_eq!(hot_for(&steady(1.0, 60, 5), 0.8, 20 * MIN), 60 * MIN);
}

/// A container read 161% once and sat at 0.12% minutes later.
#[test]
fn one_sample_is_no_heat_at_all() {
    let one = [Sample {
        at: 10_000,
        cpu: 500.0,
    }];
    assert_eq!(hot_for(&one, 0.8, 20 * MIN), 0);
}

#[test]
fn a_cool_interval_ends_the_streak() {
    let mut s = steady(1.0, 60, 5);
    let last = *s.last().expect("samples");
    s.push(Sample {
        at: last.at + 5 * MIN,
        cpu: last.cpu + 10.0,
    });
    assert_eq!(
        hot_for(&s, 0.8, 20 * MIN),
        0,
        "the newest interval was cool"
    );
}

#[test]
fn a_gap_longer_than_allowed_starts_the_streak_over() {
    let mut s = steady(1.0, 30, 5);
    let last = *s.last().expect("samples");
    s.push(Sample {
        at: last.at + 45 * MIN,
        cpu: last.cpu + 45.0 * 60.0,
    });
    s.push(Sample {
        at: last.at + 50 * MIN,
        cpu: last.cpu + 50.0 * 60.0,
    });
    assert_eq!(
        hot_for(&s, 0.8, 20 * MIN),
        5 * MIN,
        "nothing is known across the gap"
    );
}

#[test]
fn a_tree_whose_cpu_falls_is_not_hot() {
    let s = [
        Sample {
            at: 10_000,
            cpu: 900.0,
        },
        Sample {
            at: 10_300,
            cpu: 400.0,
        },
    ];
    assert_eq!(hot_for(&s, 0.8, 20 * MIN), 0);
}

#[test]
fn the_store_round_trips_and_forgets_the_dead() {
    let mut store = Store::default();
    store.record(
        "10:100",
        Sample {
            at: 1_000,
            cpu: 1.0,
        },
    );
    store.record(
        "10:100",
        Sample {
            at: 9_000,
            cpu: 2.0,
        },
    );
    store.record(
        "11:200",
        Sample {
            at: 9_000,
            cpu: 3.0,
        },
    );
    let back = Store::from_json(&store.to_json());
    assert_eq!(back, store);

    store.prune(&["10:100".to_string()], 9_000, 3_600);
    assert!(
        !store.tracks.contains_key("11:200"),
        "a gone process keeps no samples"
    );
    assert_eq!(
        store.tracks["10:100"],
        vec![Sample {
            at: 9_000,
            cpu: 2.0
        }]
    );
}

fn m(key: &str, start: u64, cpu: f64) -> Member {
    Member {
        key: key.to_string(),
        start,
        cpu,
    }
}

/// Headless Chrome replaces renderers as it works. A hot child exiting must
/// not read as the tree cooling.
#[test]
fn a_tree_that_replaces_its_hot_workers_stays_hot() {
    let mut store = Store::default();
    store.observe(
        "900:1",
        10_000,
        &[m("900:1", 1, 5.0), m("901:9000", 9_000, 900.0)],
    );
    store.observe(
        "900:1",
        10_300,
        &[m("900:1", 1, 5.0), m("901:9000", 9_000, 1_200.0)],
    );
    store.observe(
        "900:1",
        10_600,
        &[m("900:1", 1, 5.0), m("902:10400", 10_400, 190.0)],
    );
    let samples = &store.tracks["900:1"];
    assert_eq!(samples.len(), 3);
    assert!((samples[2].cpu - samples[1].cpu - 190.0).abs() < 1e-9);
    assert_eq!(hot_for(samples, 0.5, 20 * MIN), 600);
}

/// A process already running that newly shows up in the tree brings no past
/// work with it.
#[test]
fn a_member_older_than_the_last_sample_counts_from_its_first_sighting() {
    let mut store = Store::default();
    store.observe("900:1", 10_000, &[m("900:1", 1, 5.0)]);
    store.observe(
        "900:1",
        10_300,
        &[m("900:1", 1, 5.0), m("950:2", 2, 50_000.0)],
    );
    assert_eq!(store.tracks["900:1"][1].cpu, 0.0);
    store.observe(
        "900:1",
        10_600,
        &[m("900:1", 1, 5.0), m("950:2", 2, 50_300.0)],
    );
    assert!((store.tracks["900:1"][2].cpu - 300.0).abs() < 1e-9);
}

#[test]
fn members_survive_a_round_trip_and_are_pruned_with_their_tree() {
    let mut store = Store::default();
    store.observe("900:1", 10_000, &[m("900:1", 1, 5.0)]);
    store.observe("800:1", 10_000, &[m("800:1", 1, 5.0)]);
    assert_eq!(Store::from_json(&store.to_json()), store);
    store.prune(&["900:1".to_string()], 10_000, 3_600);
    assert!(!store.members.contains_key("800:1"));
    assert!(store.members.contains_key("900:1"));
}
