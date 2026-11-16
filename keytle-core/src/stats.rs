//! The stats store: append-only JSONL history and the aggregations the dashboards
//! draw.
//!
//! One test is one line. That is the whole schema decision: the file is the
//! history, newest last, and every view is a fold over it. Nothing is rewritten,
//! so a crash mid-append costs at most the line being written — which
//! [`read_all`] skips with a warning instead of refusing to start.
//!
//! **The X axis is the test index**, never the date. A hundred tests in one
//! afternoon give a hundred points, and a week away from the keyboard leaves no
//! gap, because the learner's progress is measured in tests taken, not in days
//! survived. Time is still stored (`ts`) so a `--by-date` view stays possible
//! without a schema change; it is deliberately not what [`series`] plots.
//!
//! Two consequences are pinned by tests below:
//!
//! * personal bests are per `(layout, language, mode)` — sentence mode is not
//!   comparable to pseudo-words, so merging them would be a lie;
//! * [`consistency`] is computed from *per-test* WPM, so a steady typist scores
//!   high and a spiky one low even if their averages match.

use crate::config::Mode;
use crate::engine::KeyStat;
use crate::typing::KeyAgg;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

/// Current record schema version. Bumped only for a breaking change; added
/// fields are tolerated by `serde` defaults.
pub const RECORD_VERSION: u8 = 1;

/// One finished lesson, as written to `tests.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRecord {
    /// Schema version, so a future reader can tell old lines from new ones.
    pub v: u8,
    /// When the test finished, as an RFC 3339 string. Recorded for later views;
    /// the stats views themselves are indexed by test number.
    pub ts: String,
    pub layout: String,
    /// The physical layout the keystrokes were read through.
    pub host: String,
    /// Whether the host layout was emulated positionally.
    pub emulated: bool,
    pub lang: String,
    pub mode: Mode,
    /// Which lesson of the session was typed, 0-based.
    pub lesson: u32,
    pub wpm: f32,
    pub raw_wpm: f32,
    pub accuracy: f32,
    pub duration_ms: u64,
    /// Per-key totals, keyed by the lowercase expected character.
    pub keys: BTreeMap<char, KeyAgg>,
    pub unlocked_before: u32,
    pub unlocked_after: u32,
}

/// A record that could not be read back.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("reading the stats file: {0}")]
    Io(#[from] io::Error),
    /// A line that is not a valid record. `line` is 1-based, as a text editor
    /// counts; only the *last* line may be truncated, and only it is skipped.
    #[error("stats line {line}: {reason}")]
    Line { line: usize, reason: String },
}

/// Append one record as a single line, creating the directory and the file if
/// they do not exist yet.
pub fn append(path: &Path, rec: &TestRecord) -> io::Result<()> {
    // A record with a non-finite float cannot be written as JSON at all; say so
    // instead of writing a line that no reader could ever parse back.
    let line = serde_json::to_string(rec)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

/// Read every record, in file order.
///
/// A missing file is an empty history, not an error. A garbled **last** line is
/// what a crash mid-append leaves behind: it is skipped with a warning on stderr
/// and everything before it is returned. Any other garbled line is real
/// corruption, and is an error naming it.
pub fn read_all(path: &Path) -> Result<Vec<TestRecord>, StatsError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        // No history yet is not a failure: the first run has no file at all.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    // Keep the 1-based line numbers of the file, blank lines included, so an
    // error points at what an editor would call that line.
    let mut lines: Vec<(usize, String)> = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        lines.push((i + 1, line));
    }
    let last = lines.len();
    let mut records = Vec::with_capacity(last);
    for (position, (number, text)) in lines.iter().enumerate() {
        match serde_json::from_str::<TestRecord>(text) {
            Ok(record) => records.push(record),
            // A crash mid-append leaves exactly this: every earlier line whole,
            // the newest one cut short. Losing the test in flight is honest;
            // refusing to start is not.
            Err(err) if position + 1 == last => {
                eprintln!(
                    "keytle: skipping truncated last line {number} of {}: {err}",
                    path.display()
                );
            }
            Err(err) => {
                return Err(StatsError::Line {
                    line: *number,
                    reason: err.to_string(),
                })
            }
        }
    }
    Ok(records)
}

/// Which records a view is about. `None` means "don't care".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub layout: Option<String>,
    pub lang: Option<String>,
    pub mode: Option<Mode>,
}

impl Filter {
    fn matches(&self, rec: &TestRecord) -> bool {
        self.layout.as_deref().is_none_or(|l| l == rec.layout)
            && self.lang.as_deref().is_none_or(|l| l == rec.lang)
            && self.mode.is_none_or(|m| m == rec.mode)
    }
}

/// One point on a chart: the test's number in the series and what it measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// 1-based test index within the filtered series, in file order.
    pub index: u32,
    pub wpm: f32,
    pub raw_wpm: f32,
    pub accuracy: f32,
}

/// The filtered records as a chart series, indexed `1..=n` by test number.
pub fn series(records: &[TestRecord], filter: &Filter) -> Vec<Point> {
    records
        .iter()
        .filter(|rec| filter.matches(rec))
        .enumerate()
        .map(|(i, rec)| Point {
            // The index is the test number in file order, not a date bucket: a
            // hundred tests in a day are a hundred points, a week off is no gap.
            index: u32::try_from(i + 1).unwrap_or(u32::MAX),
            wpm: rec.wpm,
            raw_wpm: rec.raw_wpm,
            accuracy: rec.accuracy,
        })
        .collect()
}

/// The best net WPM of the filtered series, or `None` before the first test.
pub fn personal_best(records: &[TestRecord], filter: &Filter) -> Option<f32> {
    records
        .iter()
        .filter(|rec| filter.matches(rec))
        .map(|rec| rec.wpm)
        .filter(|wpm| wpm.is_finite())
        .reduce(f32::max)
}

/// How steady the speed was, on a 0–5 scale: `(1 - cv) * 5`, clamped.
///
/// A single test (or none) has no spread to speak of and scores `5.0` — the
/// scale is about the absence of variance, not about the size of the numbers.
pub fn consistency(wpms: &[f32]) -> f32 {
    let n = wpms.len();
    if n < 2 {
        return 5.0;
    }
    let mean = wpms.iter().sum::<f32>() / n as f32;
    if mean <= 0.0 {
        // No speed at all is no variance at all; there is nothing to be
        // inconsistent about.
        return 5.0;
    }
    let variance = wpms.iter().map(|wpm| (wpm - mean).powi(2)).sum::<f32>() / n as f32;
    let cv = variance.sqrt() / mean;
    (1.0 - cv).clamp(0.0, 1.0) * 5.0
}

/// Whether a key is getting better, holding steady, or getting worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Flat,
    Down,
}

/// One line of the per-key table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyRow {
    pub ch: char,
    pub accuracy: f32,
    /// `KeyStat::confidence` over the window's aggregated evidence.
    pub confidence: f32,
    pub trend: Trend,
    /// Scored keystrokes for this key inside the window.
    pub samples: u32,
}

/// The per-key table over the last `last` matching records, weakest first.
///
/// The trend compares the mean accuracy of the older half of the window against
/// the newer half, so "improving" means the second half is better than the first.
pub fn key_rows(
    records: &[TestRecord],
    filter: &Filter,
    last: usize,
    target_interval_ms: f32,
) -> Vec<KeyRow> {
    // The window is the last `last` *matching* records, so an old busted key
    // cannot haunt a table that claims to describe the recent past.
    let window: Vec<&TestRecord> = records.iter().filter(|rec| filter.matches(rec)).collect();
    let window = if last < window.len() {
        &window[window.len() - last..]
    } else {
        &window[..]
    };
    if window.is_empty() {
        return Vec::new();
    }
    let mid = window.len() / 2;
    let (older, newer) = window.split_at(mid);

    let mut totals: BTreeMap<char, Totals> = BTreeMap::new();
    for (slice, half) in [(older, Half::Older), (newer, Half::Newer)] {
        for rec in slice {
            for (ch, agg) in &rec.keys {
                let total = totals.entry(*ch).or_default();
                total.add(agg, half);
            }
        }
    }

    let mut rows: Vec<KeyRow> = totals
        .into_iter()
        .map(|(ch, total)| {
            let mean_dt = total.mean_dt();
            let stat = KeyStat {
                hits: total.hits,
                misses: total.misses,
                shift_misses: total.shift_misses,
                mean_interval_ms: mean_dt,
            };
            KeyRow {
                ch,
                accuracy: stat.accuracy(),
                confidence: stat.confidence(target_interval_ms),
                trend: total.trend(),
                samples: total.hits + total.misses + total.shift_misses,
            }
        })
        .collect();
    // Weakest first: the least confident key is the one to practise next.
    rows.sort_by(|a, b| {
        a.confidence
            .total_cmp(&b.confidence)
            .then(a.accuracy.total_cmp(&b.accuracy))
            .then(a.ch.cmp(&b.ch))
    });
    rows
}

/// How much accuracy moved between the halves of the window before it counts as
/// a trend rather than noise.
const TREND_EPSILON: f32 = 0.005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    Older,
    Newer,
}

/// Running evidence for one key across the window.
#[derive(Debug, Default)]
struct Totals {
    hits: u32,
    misses: u32,
    shift_misses: u32,
    dt_weighted_sum: f32,
    dt_weight: u32,
    older_acc: HalfAcc,
    newer_acc: HalfAcc,
}

/// Hits and misses inside one half of the window. Shift misses are excluded:
/// the letter was known and the modifier was not, so blaming the letter would
/// misreport the trend.
#[derive(Debug, Default)]
struct HalfAcc {
    hits: u32,
    misses: u32,
}

impl HalfAcc {
    fn add(&mut self, agg: &KeyAgg) {
        self.hits += agg.hits;
        self.misses += agg.misses;
    }

    /// `None` when this half holds no evidence for the key.
    fn accuracy(&self) -> Option<f32> {
        let total = self.hits + self.misses;
        (total > 0).then(|| self.hits as f32 / total as f32)
    }
}

impl Totals {
    fn add(&mut self, agg: &KeyAgg, half: Half) {
        self.hits += agg.hits;
        self.misses += agg.misses;
        self.shift_misses += agg.shift_misses;
        // A per-record mean_dt is itself a mean of intervals; the hit count is
        // the best available weight for folding several records into one.
        if agg.mean_dt > 0 && agg.hits > 0 {
            self.dt_weighted_sum += agg.mean_dt as f32 * agg.hits as f32;
            self.dt_weight += agg.hits;
        }
        match half {
            Half::Older => self.older_acc.add(agg),
            Half::Newer => self.newer_acc.add(agg),
        }
    }

    fn mean_dt(&self) -> f32 {
        if self.dt_weight == 0 {
            0.0
        } else {
            self.dt_weighted_sum / self.dt_weight as f32
        }
    }

    fn trend(&self) -> Trend {
        match (self.older_acc.accuracy(), self.newer_acc.accuracy()) {
            (Some(older), Some(newer)) if newer - older > TREND_EPSILON => Trend::Up,
            (Some(older), Some(newer)) if older - newer > TREND_EPSILON => Trend::Down,
            // One half is empty, or the two agree: nothing to claim.
            _ => Trend::Flat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    /// A scratch directory that cleans itself up, so tests need no tempfile crate
    /// and never touch the real stats file.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("keytle-stats-{}-{tag}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Scratch(dir)
        }

        /// `<scratch>/nested/tests.jsonl`, a path whose parent does not exist yet.
        fn file(&self) -> std::path::PathBuf {
            self.0.join("nested").join("tests.jsonl")
        }

        fn write(&self, contents: &str) -> std::path::PathBuf {
            let path = self.0.join("tests.jsonl");
            fs::create_dir_all(&self.0).expect("a scratch dir");
            fs::write(&path, contents).expect("a written stats file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn key(hits: u32, misses: u32, mean_dt: u32) -> KeyAgg {
        KeyAgg {
            hits,
            misses,
            shift_misses: 0,
            mean_dt,
        }
    }

    fn record(ts: &str, mode: Mode, wpm: f32, g: f32) -> TestRecord {
        TestRecord {
            v: RECORD_VERSION,
            ts: ts.to_string(),
            layout: "colemak-dh".to_string(),
            host: "qwerty".to_string(),
            emulated: true,
            lang: "en".to_string(),
            mode,
            lesson: 1,
            wpm,
            raw_wpm: wpm + g / 10.0,
            accuracy: 0.95,
            duration_ms: 30_000,
            keys: BTreeMap::new(),
            unlocked_before: 4,
            unlocked_after: 5,
        }
    }

    /// A record carrying one key's evidence, for the per-key tests.
    fn with_key(ts: &str, ch: char, agg: KeyAgg) -> TestRecord {
        let mut rec = record(ts, Mode::Pseudo, 50.0, 0.0);
        rec.keys.insert(ch, agg);
        rec
    }

    #[test]
    fn append_then_read_round_trips_and_creates_parent_dirs() {
        let scratch = Scratch::new("roundtrip");
        let path = scratch.file();
        let first = record("2026-11-15T10:00:00Z", Mode::Pseudo, 60.0, 1.0);
        let second = record("2026-11-15T10:05:00Z", Mode::Sentences, 72.5, 2.0);
        append(&path, &first).expect("append");
        append(&path, &second).expect("append");

        let read = read_all(&path).expect("read back");
        assert_eq!(read, vec![first, second], "append order is file order");
        // One line per record, so a partial tail costs one record at most.
        let raw = fs::read_to_string(&path).expect("the file");
        assert_eq!(raw.lines().count(), 2);
        assert!(raw.ends_with('\n'), "every line is terminated");
    }

    #[test]
    fn per_key_totals_survive_the_round_trip() {
        let scratch = Scratch::new("keys");
        let path = scratch.file();
        let mut rec = record("2026-11-15T10:00:00Z", Mode::Words, 55.0, 0.0);
        rec.keys.insert('a', key(9, 1, 180));
        rec.keys.insert('é', key(3, 0, 240));
        append(&path, &rec).expect("append");
        assert_eq!(read_all(&path).expect("read back"), vec![rec]);
    }

    #[test]
    fn missing_file_is_an_empty_history() {
        let scratch = Scratch::new("missing");
        assert!(read_all(&scratch.0.join("nope.jsonl"))
            .expect("absent is not an error")
            .is_empty());
    }

    #[test]
    fn a_truncated_last_line_is_skipped_not_fatal() {
        // Exactly the shape a crash mid-append leaves: whole records, then half
        // a JSON object and no newline.
        let scratch = Scratch::new("truncated");
        let path = scratch.write(concat!(
            r#"{"v":1,"ts":"2026-11-15T10:00:00Z","layout":"colemak-dh","host":"qwerty","emulated":true,"lang":"en","mode":"pseudo","lesson":1,"wpm":50.0,"raw_wpm":52.0,"accuracy":0.96,"duration_ms":30000,"keys":{},"unlocked_before":4,"unlocked_after":5}"#,
            "\n",
            r#"{"v":1,"ts":"2026-11-15T10:05:00Z","layout":"colemak-dh","host":"qwerty","emulated":true,"lang":"en","mode":"pseudo","lesson":2,"wpm":55.0,"raw_wpm":58.0,"accuracy":0.97,"duration_ms":30000,"keys":{},"unlocked_before":5,"unlocked_after":6}"#,
            "\n",
            r#"{"v":1,"ts":"2026-11-15T10:1"#,
        ));
        let read = read_all(&path).expect("a truncated tail is skipped, not an error");
        assert_eq!(read.len(), 2, "the two complete records survive");
        assert_eq!(read[1].wpm, 55.0);
    }

    #[test]
    fn a_garbled_middle_line_names_its_line_number() {
        let scratch = Scratch::new("garbled");
        let good = serde_json::to_string(&record("2026-11-15T10:00:00Z", Mode::Pseudo, 50.0, 0.0))
            .expect("serialise");
        let path = scratch.write(&format!("{good}\nnot json at all\n{good}\n"));
        let err = read_all(&path).expect_err("corruption in the middle is an error");
        match err {
            StatsError::Line { line, .. } => assert_eq!(line, 2),
            other => panic!("expected a line error, got {other:?}"),
        }
        assert!(err.to_string().contains('2'), "the message names the line");
    }

    #[test]
    fn blank_lines_are_ignored() {
        let scratch = Scratch::new("blank");
        let good = serde_json::to_string(&record("2026-11-15T10:00:00Z", Mode::Pseudo, 50.0, 0.0))
            .expect("serialise");
        let path = scratch.write(&format!("{good}\n\n{good}\n"));
        assert_eq!(read_all(&path).expect("blank lines skipped").len(), 2);
    }

    #[test]
    fn series_is_indexed_by_test_not_by_date() {
        // Three tests over two days: two on the 15th, one on the 17th. The index
        // is the test number, so the missing 16th leaves no gap and no bucket.
        let records = vec![
            record("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
            record("2026-11-15T09:10:00Z", Mode::Pseudo, 50.0, 0.0),
            record("2026-11-17T21:00:00Z", Mode::Pseudo, 60.0, 0.0),
        ];
        let points = series(&records, &Filter::default());
        assert_eq!(
            points.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            points.iter().map(|p| p.wpm).collect::<Vec<_>>(),
            vec![40.0, 50.0, 60.0]
        );
        assert!(close(points[2].accuracy, 0.95));
    }

    #[test]
    fn a_hundred_tests_in_one_day_are_a_hundred_points() {
        let records: Vec<TestRecord> = (0..100)
            .map(|i| {
                record(
                    &format!("2026-11-15T09:{:02}:00Z", i % 60),
                    Mode::Pseudo,
                    i as f32,
                    0.0,
                )
            })
            .collect();
        let points = series(&records, &Filter::default());
        assert_eq!(points.len(), 100);
        assert_eq!(points[99].index, 100, "the last point is test 100");
    }

    #[test]
    fn series_filters_by_mode() {
        let records = vec![
            record("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
            record("2026-11-15T09:10:00Z", Mode::Sentences, 30.0, 0.0),
            record("2026-11-15T09:20:00Z", Mode::Pseudo, 45.0, 0.0),
        ];
        let filter = Filter {
            mode: Some(Mode::Sentences),
            ..Filter::default()
        };
        let points = series(&records, &filter);
        assert_eq!(points.len(), 1, "only the sentence test matches");
        assert_eq!(points[0].wpm, 30.0);
        assert_eq!(points[0].index, 1, "the index counts matched tests only");
    }

    #[test]
    fn series_filters_by_layout_and_language() {
        let mut other = record("2026-11-15T09:30:00Z", Mode::Pseudo, 99.0, 0.0);
        other.layout = "bepo".to_string();
        let mut french = record("2026-11-15T09:40:00Z", Mode::Pseudo, 88.0, 0.0);
        french.lang = "fr".to_string();
        let records = vec![
            record("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
            other,
            french,
        ];
        let by_layout = series(
            &records,
            &Filter {
                layout: Some("bepo".to_string()),
                ..Filter::default()
            },
        );
        assert_eq!(by_layout.len(), 1);
        assert_eq!(by_layout[0].wpm, 99.0);
        let by_lang = series(
            &records,
            &Filter {
                lang: Some("fr".to_string()),
                ..Filter::default()
            },
        );
        assert_eq!(by_lang.len(), 1);
        assert_eq!(by_lang[0].wpm, 88.0);
    }

    #[test]
    fn personal_best_is_per_series_never_merged_across_modes() {
        let records = vec![
            record("2026-11-15T09:00:00Z", Mode::Pseudo, 60.0, 0.0),
            record("2026-11-15T09:10:00Z", Mode::Pseudo, 71.0, 0.0),
            record("2026-11-15T09:20:00Z", Mode::Sentences, 44.0, 0.0),
        ];
        let pseudo = Filter {
            mode: Some(Mode::Pseudo),
            ..Filter::default()
        };
        let sentences = Filter {
            mode: Some(Mode::Sentences),
            ..Filter::default()
        };
        assert_eq!(personal_best(&records, &pseudo), Some(71.0));
        assert_eq!(
            personal_best(&records, &sentences),
            Some(44.0),
            "a sentence PB is not comparable to a pseudo PB"
        );
        assert_ne!(
            personal_best(&records, &pseudo),
            personal_best(&records, &sentences)
        );
        assert_eq!(personal_best(&records, &Filter::default()), Some(71.0));
        assert_eq!(
            personal_best(&[], &pseudo),
            None,
            "no tests, no personal best"
        );
    }

    #[test]
    fn consistency_of_identical_wpms_is_five() {
        assert!(close(consistency(&[60.0; 10]), 5.0));
        assert!(close(consistency(&[42.0]), 5.0), "one test has no spread");
        assert!(close(consistency(&[]), 5.0), "no tests is not a failure");
    }

    #[test]
    fn consistency_falls_with_spread_and_stays_on_scale() {
        // cv of [40, 60]: sd (population) 10, mean 50 → cv 0.2 → 4.0.
        assert!(close(consistency(&[40.0, 60.0]), 4.0));
        // An outlier-heavy series approaches, never passes, zero.
        let spiky = [10.0, 10.0, 10.0, 200.0];
        let c = consistency(&spiky);
        assert!((0.0..=5.0).contains(&c), "consistency {c} out of range");
        assert!(c < 1.0, "a wildly spiky series is not steady: {c}");
        // All-zero WPM has no coefficient of variation to speak of.
        assert!(close(consistency(&[0.0, 0.0]), 5.0));
    }

    #[test]
    fn a_key_whose_newer_half_is_better_trends_up() {
        // Older half: 6 hits, 4 misses → 0.6. Newer half: 10 hits, 0 misses.
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(3, 2, 150)),
            with_key("2026-11-15T09:05:00Z", 'k', key(3, 2, 150)),
            with_key("2026-11-15T09:10:00Z", 'k', key(5, 0, 150)),
            with_key("2026-11-15T09:15:00Z", 'k', key(5, 0, 150)),
        ];
        let rows = key_rows(&records, &Filter::default(), 20, 150.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ch, 'k');
        assert_eq!(rows[0].trend, Trend::Up);
        assert!(close(rows[0].accuracy, 16.0 / 20.0), "windowed accuracy");
        assert_eq!(rows[0].samples, 20);

        // The mirror image, so the sign is not an accident of the ordering.
        let worse = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(5, 0, 150)),
            with_key("2026-11-15T09:05:00Z", 'k', key(5, 0, 150)),
            with_key("2026-11-15T09:10:00Z", 'k', key(3, 2, 150)),
            with_key("2026-11-15T09:15:00Z", 'k', key(3, 2, 150)),
        ];
        assert_eq!(
            key_rows(&worse, &Filter::default(), 20, 150.0)[0].trend,
            Trend::Down
        );
        // Same evidence either side of the midpoint: nothing to report.
        let flat = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(4, 1, 150)),
            with_key("2026-11-15T09:05:00Z", 'k', key(4, 1, 150)),
        ];
        assert_eq!(
            key_rows(&flat, &Filter::default(), 20, 150.0)[0].trend,
            Trend::Flat
        );
    }

    #[test]
    fn key_rows_sorts_weakest_first() {
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'a', key(10, 0, 150)),
            with_key("2026-11-15T09:05:00Z", 'b', key(5, 5, 150)),
            with_key("2026-11-15T09:10:00Z", 'c', key(8, 2, 150)),
        ];
        let rows = key_rows(&records, &Filter::default(), 20, 150.0);
        let order: Vec<char> = rows.iter().map(|r| r.ch).collect();
        assert_eq!(order, vec!['b', 'c', 'a'], "weakest accuracy first");
        assert!(rows[0].confidence < rows[2].confidence);
        assert!(!rows[0].trend.eq(&Trend::Up) || rows[0].accuracy < 1.0);
    }

    #[test]
    fn key_rows_reads_only_the_last_n_matching_records() {
        // 30 pseudo tests, then 5 sentence tests. Two things are pinned: the
        // window is the last 20 *of the filter*, and old records cannot leak in.
        let mut records: Vec<TestRecord> = (0..30)
            .map(|i| {
                let agg = if i < 10 {
                    key(0, 10, 150)
                } else {
                    key(10, 0, 150)
                };
                with_key(&format!("2026-11-15T09:00:{:02}Z", i), 'a', agg)
            })
            .collect();
        for i in 0..5 {
            let mut rec = record(
                &format!("2026-11-15T10:00:{:02}Z", i),
                Mode::Sentences,
                30.0,
                0.0,
            );
            rec.keys.insert('a', key(0, 5, 150));
            records.push(rec);
        }
        let filter = Filter {
            mode: Some(Mode::Pseudo),
            ..Filter::default()
        };
        let rows = key_rows(&records, &filter, 20, 150.0);
        assert_eq!(rows.len(), 1);
        // Last 20 pseudo records are tests 11..=30, all clean: 200 hits, 0 misses.
        assert_eq!(rows[0].samples, 200, "only the last 20 matching records");
        assert!(
            close(rows[0].accuracy, 1.0),
            "old misses fell out of the window"
        );

        // A bigger window still sees the whole pseudo history.
        let all = key_rows(&records, &filter, 100, 150.0);
        assert_eq!(
            all[0].samples, 300,
            "10 misses + 200 hits, sentence excluded"
        );
        // And `last` cannot reach past the filter into sentence records.
        // …while the wider window does see them: 200 hits over 300 keystrokes.
        assert!(close(all[0].accuracy, 200.0 / 300.0));
    }

    #[test]
    fn key_rows_confidence_uses_interval_against_target() {
        // On target: full speed credit; all hits: full accuracy credit → 1.0.
        let on_target = vec![with_key("2026-11-15T09:00:00Z", 'x', key(10, 0, 150))];
        assert!(close(
            key_rows(&on_target, &Filter::default(), 20, 150.0)[0].confidence,
            1.0
        ));
        // Twice as slow as target: half speed credit → 0.6 * 0.5 + 0.4 = 0.7.
        let slow = vec![with_key("2026-11-15T09:00:00Z", 'x', key(10, 0, 300))];
        assert!(close(
            key_rows(&slow, &Filter::default(), 20, 150.0)[0].confidence,
            0.7
        ));
    }

    #[test]
    fn key_rows_is_empty_without_matching_records() {
        let records = vec![record("2026-11-15T09:00:00Z", Mode::Sentences, 40.0, 0.0)];
        let filter = Filter {
            mode: Some(Mode::Pseudo),
            ..Filter::default()
        };
        assert!(key_rows(&records, &filter, 20, 150.0).is_empty());
        assert!(key_rows(&[], &Filter::default(), 20, 150.0).is_empty());
    }
}
