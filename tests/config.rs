//! Settings: defaults, overlay, and the values that are refused.

use std::path::Path;
use std::time::Duration;

use proc_gc::config::Settings;
use proc_gc::toml_subset::Config;

fn settings(text: &str) -> Result<Settings, String> {
    Settings::from_config(&Config::parse(text).expect("parse"))
}

#[test]
fn the_defaults_are_an_hour_at_eighty_percent() {
    let s = Settings::defaults();
    assert_eq!(s.min_hot, Duration::from_secs(3600));
    assert!((s.cores - 0.8).abs() < 1e-9);
    assert!(s.build_names.iter().any(|n| n == "cargo"));
}

#[test]
fn values_overlay_the_defaults() {
    let s = settings(
        "[proc-gc]\nmin_hot_minutes = \"90\"\ncpu_percent = \"150\"\nhold = [\"ollama\"]\n",
    )
    .expect("settings");
    assert_eq!(s.min_hot, Duration::from_secs(90 * 60));
    assert!((s.cores - 1.5).abs() < 1e-9);
    assert_eq!(s.holds, vec!["ollama".to_string()]);
}

/// An empty or extra list must never drop cargo, rustc or the linkers.
#[test]
fn build_names_add_to_the_built_ins() {
    let empty = settings("[proc-gc]\nbuild_names = []\n").expect("settings");
    assert_eq!(empty.build_names, Settings::defaults().build_names);

    let extra = settings("[proc-gc]\nbuild_names = [\"bazel\"]\n").expect("settings");
    assert!(extra.build_names.iter().any(|n| n == "bazel"));
    assert!(extra.build_names.iter().any(|n| n == "rustc"));
}

#[test]
fn unsafe_values_are_refused() {
    for bad in [
        "min_hot_minutes = \"0\"",
        "cpu_percent = \"5\"",
        "max_gap_minutes = \"0\"",
        "min_hot_minutes = \"10\"\nmax_gap_minutes = \"20\"",
        "grace_seconds = \"600\"",
        "cpu_percent = \"eighty\"",
    ] {
        assert!(
            settings(&format!("[proc-gc]\n{bad}\n")).is_err(),
            "accepted {bad:?}"
        );
    }
}

#[test]
fn the_example_file_is_exactly_the_defaults() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    assert_eq!(
        Settings::load(&example).expect("load"),
        Settings::defaults()
    );
}
