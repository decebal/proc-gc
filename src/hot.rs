//! "Burned CPU for an hour", measured across runs.
//!
//! Each scheduled run records how much CPU every tracked tree has done since
//! it was first seen. A tree is hot for as long as every interval between
//! consecutive samples, walking back from the newest, used at least `cores`
//! of CPU and no interval is longer than `max_gap`: a gap means the machine
//! slept or the schedule stopped, and nothing is known about that stretch.
//! Samples are keyed by the root's pid and start time, so a reused pid starts
//! from nothing.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::{Proc, START_TOLERANCE};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: u64,
    /// CPU seconds the tree has done since it was first sampled. Never falls.
    pub cpu: f64,
}

/// One tree member as a run sees it: its own key, start time and cumulative
/// CPU seconds.
pub struct Member {
    pub key: String,
    pub start: u64,
    pub cpu: f64,
}

/// Seconds of unbroken hotness ending at the newest sample; 0 when the last
/// interval was not hot.
pub fn hot_for(samples: &[Sample], cores: f64, max_gap: u64) -> u64 {
    let Some(last) = samples.last() else {
        return 0;
    };
    let mut start = last.at;
    for pair in samples.windows(2).rev() {
        let (a, b) = (pair[0], pair[1]);
        let wall = b.at.saturating_sub(a.at);
        if wall == 0 || wall > max_gap {
            break;
        }
        if (b.cpu - a.cpu) / wall as f64 >= cores {
            start = a.at;
        } else {
            break;
        }
    }
    last.at - start
}

/// Samples per tree, and each tree's members at its last sample, persisted
/// between runs.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Store {
    pub tracks: HashMap<String, Vec<Sample>>,
    pub members: HashMap<String, HashMap<String, f64>>,
}

impl Store {
    pub fn record(&mut self, key: &str, sample: Sample) {
        self.tracks.entry(key.to_string()).or_default().push(sample);
    }

    /// Add a sample for `tree` from its members now. The work since the last
    /// sample is the CPU gained by members seen then, plus all the CPU of
    /// members started since. Summing live members instead would read a hot
    /// child's exit as the tree cooling, and a tree that replaces its workers
    /// would never stay hot. A member that joined the tree but started before
    /// the last sample counts from now.
    pub fn observe(&mut self, tree: &str, at: u64, members: &[Member]) {
        let prev = self.tracks.get(tree).and_then(|v| v.last()).copied();
        let known = self.members.get(tree);
        let work = prev.map_or(0.0, |p| {
            p.cpu
                + members
                    .iter()
                    .map(|m| match known.and_then(|k| k.get(&m.key)) {
                        Some(before) => (m.cpu - before).max(0.0),
                        None if m.start >= p.at => m.cpu,
                        None => 0.0,
                    })
                    .sum::<f64>()
        });
        self.record(tree, Sample { at, cpu: work });
        self.members.insert(
            tree.to_string(),
            members.iter().map(|m| (m.key.clone(), m.cpu)).collect(),
        );
    }

    /// Snap each process's start to the one on record for its pid, tree or
    /// member, when within [`START_TOLERANCE`]. Without this a derived start
    /// that moved a second gives the process a new key, and its history is
    /// pruned as gone.
    pub fn settle_starts(&self, procs: &mut [Proc]) {
        let mut known: HashMap<u32, Vec<u64>> = HashMap::new();
        let keys = self
            .tracks
            .keys()
            .chain(self.members.values().flat_map(HashMap::keys));
        for key in keys {
            let parsed = key
                .split_once(':')
                .and_then(|(pid, start)| Some((pid.parse().ok()?, start.parse().ok()?)));
            if let Some((pid, start)) = parsed {
                known.entry(pid).or_default().push(start);
            }
        }
        for p in procs {
            let on_record = known.get(&p.pid).and_then(|starts| {
                starts
                    .iter()
                    .find(|s| s.abs_diff(p.start) <= START_TOLERANCE)
            });
            if let Some(&start) = on_record {
                p.start = start;
            }
        }
    }

    /// Drop trees that are gone and samples older than `keep` seconds.
    pub fn prune(&mut self, alive: &[String], now: u64, keep: u64) {
        self.tracks.retain(|k, _| alive.contains(k));
        for samples in self.tracks.values_mut() {
            samples.retain(|s| now.saturating_sub(s.at) <= keep);
        }
        self.tracks.retain(|_, v| !v.is_empty());
        let tracks = &self.tracks;
        self.members.retain(|k, _| tracks.contains_key(k));
    }

    pub fn to_json(&self) -> Value {
        let tracks: serde_json::Map<String, Value> = self
            .tracks
            .iter()
            .map(|(k, v)| {
                let pairs: Vec<Value> = v.iter().map(|s| json!([s.at, s.cpu])).collect();
                (k.clone(), Value::Array(pairs))
            })
            .collect();
        let members: serde_json::Map<String, Value> = self
            .members
            .iter()
            .map(|(k, m)| (k.clone(), json!(m)))
            .collect();
        json!({ "tracks": tracks, "members": members })
    }

    pub fn from_json(v: &Value) -> Store {
        let mut store = Store::default();
        if let Some(tracks) = v["tracks"].as_object() {
            for (k, list) in tracks {
                let samples = list
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| {
                                Some(Sample {
                                    at: p.get(0)?.as_u64()?,
                                    cpu: p.get(1)?.as_f64()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                store.tracks.insert(k.clone(), samples);
            }
        }
        if let Some(members) = v["members"].as_object() {
            for (k, m) in members {
                let m: HashMap<String, f64> = m
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .filter_map(|(mk, c)| Some((mk.clone(), c.as_f64()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                store.members.insert(k.clone(), m);
            }
        }
        store
    }
}
