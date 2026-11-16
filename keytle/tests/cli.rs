//! End-to-end tests: the real binary, in scratch data and config directories.
//!
//! Nothing here starts the TUI — every case is one of the plain-stdout
//! subcommands, which is exactly what a script or a Nix check would run.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

/// A scratch pair of directories, removed when dropped.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "keytle-cli-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("a scratch dir");
        Scratch { base }
    }

    fn data(&self) -> PathBuf {
        self.base.join("data")
    }

    fn config(&self) -> PathBuf {
        self.base.join("config")
    }

    /// Write a `tests.jsonl` of two well-formed v1 records.
    fn two_records(&self) -> PathBuf {
        let path = self.data().join("tests.jsonl");
        std::fs::create_dir_all(self.data()).expect("the data dir");
        std::fs::write(&path, FIXTURE).expect("the fixture");
        path
    }

    /// Write a `tests.jsonl` of one well-formed v1 record.
    fn one_record(&self) -> PathBuf {
        let path = self.data().join("tests.jsonl");
        std::fs::create_dir_all(self.data()).expect("the data dir");
        let first = FIXTURE.lines().next().expect("the first fixture record");
        std::fs::write(&path, format!("{first}\n")).expect("the fixture");
        path
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_keytle"))
            .args(args)
            .env("KEYTLE_DATA_DIR", self.data())
            .env("KEYTLE_CONFIG_DIR", self.config())
            .env("TERM", "dumb")
            .output()
            .expect("the keytle binary runs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Two completed lessons, in the shape the app writes them.
const FIXTURE: &str = concat!(
    r#"{"v":1,"ts":"2026-11-15T10:00:00Z","layout":"colemak-dh","host":"qwerty","emulated":true,"lang":"en","mode":"pseudo","lesson":0,"wpm":40.0,"raw_wpm":44.0,"accuracy":0.96,"duration_ms":30000,"keys":{},"unlocked_before":4,"unlocked_after":5}"#,
    "\n",
    r#"{"v":1,"ts":"2026-11-15T10:05:00Z","layout":"colemak-dh","host":"qwerty","emulated":true,"lang":"en","mode":"pseudo","lesson":1,"wpm":48.0,"raw_wpm":52.0,"accuracy":0.97,"duration_ms":30000,"keys":{},"unlocked_before":5,"unlocked_after":6}"#,
    "\n",
);

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn doctor_exits_zero_with_the_data_dir_and_a_reachability_line() {
    let scratch = Scratch::new("doctor");
    let out = scratch.run(&["doctor"]);
    let text = stdout(&out);
    assert!(out.status.success(), "doctor failed:\n{text}");
    assert!(
        text.contains(&scratch.data().display().to_string()),
        "the data dir is named:\n{text}"
    );
    assert!(
        text.contains("reachability:"),
        "a reachability line:\n{text}"
    );
    assert!(text.contains("config:"), "{text}");
}

#[test]
fn stats_on_an_empty_data_dir_says_no_tests_yet() {
    let scratch = Scratch::new("stats-empty");
    let out = scratch.run(&["stats"]);
    let text = stdout(&out);
    assert!(out.status.success(), "stats failed:\n{text}");
    assert!(text.contains("no tests yet"), "{text}");
}

#[test]
fn stats_summarises_a_fixture() {
    let scratch = Scratch::new("stats-two");
    scratch.two_records();
    let out = scratch.run(&["stats", "--layout", "colemak-dh", "--mode", "pseudo"]);
    let text = stdout(&out);
    assert!(out.status.success(), "stats failed:\n{text}");
    assert!(text.contains("tests        2"), "{text}");
    assert!(text.contains("best wpm     48"), "{text}");
    assert!(text.contains("consistency"), "{text}");
}

#[test]
fn stats_with_a_filter_that_matches_nothing_does_not_claim_the_history_is_empty() {
    let scratch = Scratch::new("stats-nomatch");
    scratch.one_record();
    let out = scratch.run(&["stats", "--mode", "sentences"]);
    let text = stdout(&out);
    assert!(out.status.success(), "stats failed:\n{text}");
    assert!(
        text.contains("no tests match this filter"),
        "not the empty-store message:\n{text}"
    );
    assert!(
        text.contains("mode sentences"),
        "the filter is named:\n{text}"
    );
    assert!(
        text.contains("1 test recorded for other layouts/languages/modes"),
        "the hidden history is counted:\n{text}"
    );
    assert!(!text.contains("no tests yet"), "{text}");
    assert!(!text.contains("tests        0"), "{text}");
}

#[test]
fn export_jsonl_round_trips_both_fixture_records() {
    let scratch = Scratch::new("export-jsonl");
    scratch.two_records();
    let out = scratch.run(&["export", "--format", "jsonl"]);
    let text = stdout(&out);
    assert!(out.status.success(), "export failed:\n{text}");
    assert_eq!(text.lines().count(), 2, "one object per record:\n{text}");
    assert!(
        text.lines().all(|line| line.starts_with('{')),
        "one JSON object per line:\n{text}"
    );
    // Round-trip through the same reader the app uses, so each exported line
    // has to be a complete, well-formed record — not merely valid JSON.
    let path = scratch.base.join("roundtrip.jsonl");
    std::fs::write(&path, &text).expect("the exported text");
    let parsed = keytle_core::stats::read_all(&path).expect("every exported line parses");
    assert_eq!(parsed.len(), 2, "both records survive:\n{text}");
    let wpms: Vec<f32> = parsed.iter().map(|rec| rec.wpm).collect();
    assert_eq!(wpms, vec![40.0, 48.0], "{text}");
    assert!(
        parsed.iter().all(|rec| rec.layout == "colemak-dh"),
        "{text}"
    );
}

#[test]
fn doctor_reports_the_default_pair_s_reachability_honestly() {
    let scratch = Scratch::new("doctor-reach");
    let out = scratch.run(&["doctor"]);
    let text = stdout(&out);
    assert!(out.status.success(), "doctor failed:\n{text}");
    // The default pair shares a board shape, but qwerty has no altgr level, so
    // colemak-dh's altgr characters are named rather than glossed over.
    assert!(
        text.contains("reachability: qwerty -> colemak-dh:"),
        "the pair is named:\n{text}"
    );
    assert!(
        text.contains("unreachable character"),
        "the gap is reported, not claimed away:\n{text}"
    );
    assert!(text.contains('é'), "the characters are named:\n{text}");
}

#[test]
fn export_tsv_prints_a_header_and_one_row_per_record() {
    let scratch = Scratch::new("export-tsv");
    scratch.two_records();
    let out = scratch.run(&["export", "--format", "tsv"]);
    let text = stdout(&out);
    assert!(out.status.success(), "export failed:\n{text}");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "header plus two records:\n{text}");
    assert!(lines[0].starts_with("ts\tlayout\thost"), "{text}");
    assert!(lines[1].contains("colemak-dh"), "{text}");
    assert!(lines[2].contains("48.00"), "{text}");
}

#[test]
fn export_jsonl_to_a_file_writes_the_records() {
    let scratch = Scratch::new("export-file");
    scratch.two_records();
    let path = scratch.base.join("out/history.jsonl");
    let out = scratch.run(&["export", "-o", path.to_str().expect("a utf-8 path")]);
    assert!(out.status.success(), "export failed:\n{}", stdout(&out));
    let written = std::fs::read_to_string(&path).expect("the exported file");
    assert_eq!(written.lines().count(), 2, "{written}");
    assert!(
        written.lines().all(|line| line.starts_with('{')),
        "{written}"
    );
}

#[test]
fn layouts_list_names_every_bundled_layout() {
    let scratch = Scratch::new("layouts");
    let out = scratch.run(&["layouts", "list"]);
    let text = stdout(&out);
    assert!(out.status.success(), "layouts list failed:\n{text}");
    for id in ["qwerty", "colemak-dh", "bepo", "gallium", "azerty"] {
        assert!(text.contains(id), "{id} is listed:\n{text}");
    }
}
