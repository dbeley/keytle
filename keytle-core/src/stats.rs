//! The stats store: append-only JSONL history and the aggregations the dashboards
//! draw.
//!
//! One test is one line. That is the whole schema decision: the file is the
//! history, newest last, and every view is a fold over it. Nothing is rewritten,
//! so a crash mid-append costs at most the line being written - which
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
//! * personal bests are per `(layout, language, mode)` - sentence mode is not
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
/// fields carry `#[serde(default)]`, so an old line missing one still parses.
pub const RECORD_VERSION: u8 = 1;

/// One finished lesson, as written to `tests.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestRecord {
    /// Schema version, so a future reader can tell old lines from new ones.
    pub v: u8,
    /// When the test finished, as an RFC 3339 string. Recorded for later views;
    /// the stats views themselves are indexed by test number.
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub layout: String,
    /// The physical layout the keystrokes were read through.
    #[serde(default)]
    pub host: String,
    /// Whether the host layout was emulated positionally.
    #[serde(default)]
    pub emulated: bool,
    #[serde(default)]
    pub lang: String,
    // `mode` has no sensible default: a record that does not say what it was
    // typing cannot be compared to anything, so it stays required.
    pub mode: Mode,
    /// Which lesson of the session was typed, 0-based.
    #[serde(default)]
    pub lesson: u32,
    #[serde(default)]
    pub wpm: f32,
    #[serde(default)]
    pub raw_wpm: f32,
    #[serde(default)]
    pub accuracy: f32,
    #[serde(default)]
    pub duration_ms: u64,
    /// Per-key totals, keyed by the lowercase expected character.
    #[serde(default)]
    pub keys: BTreeMap<char, KeyAgg>,
    #[serde(default)]
    pub unlocked_before: u32,
    #[serde(default)]
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
    let mut line = serde_json::to_string(rec)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    // The newline belongs to the line: one `write_all` per record, so a second
    // O_APPEND writer can never land between the object and its terminator and
    // put two objects on one line - which `read_all` treats as fatal.
    line.push('\n');
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
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
    pub fn matches(&self, rec: &TestRecord) -> bool {
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
        .filter(|rec| filter.matches(rec) && rec.is_comparable())
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

/// A test must run this long to be charted.
pub const MIN_COMPARABLE_MS: u64 = 10_000;
/// ... and have this many keystrokes. A five-second burst of a dozen words
/// reports a flattering speed that belongs to the easing-in, not to the typist,
/// and one such point also rescales the whole speed chart.
pub const MIN_COMPARABLE_KEYS: u32 = 50;

impl TestRecord {
    /// Every keystroke the test charged to a key, right or wrong.
    pub fn keystrokes(&self) -> u32 {
        self.keys
            .values()
            .map(|agg| agg.hits + agg.misses)
            .sum::<u32>()
    }

    /// Whether this test is long and large enough for its speed to be compared
    /// with the others. Short tests stay in the file and count for the per-key
    /// table; they just do not get a column of their own.
    pub fn is_comparable(&self) -> bool {
        self.duration_ms >= MIN_COMPARABLE_MS && self.keystrokes() >= MIN_COMPARABLE_KEYS
    }
}

/// The best net WPM of the filtered series, or `None` before the first test.
pub fn personal_best(records: &[TestRecord], filter: &Filter) -> Option<f32> {
    records
        .iter()
        .filter(|rec| filter.matches(rec) && rec.is_comparable())
        .map(|rec| rec.wpm)
        .filter(|wpm| wpm.is_finite())
        .reduce(f32::max)
}

/// How steady the speed was, on a 0–5 scale: `(1 - cv) * 5`, clamped.
///
/// A single test (or none) has no spread to speak of and scores `5.0` - the
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
/// The trend compares the mean interval per key in the older half of the window
/// against the newer half: "improving" means the second half is *faster*.
/// Accuracy is reported separately and plays no part in it.
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
                // The space bar is not a key to practise: it is pressed by
                // whichever thumb is free, it never holds a letter back, and a
                // "weakest key" list headed by a blank cell helps nobody.
                if ch.is_whitespace() {
                    continue;
                }
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

/// How much the mean interval (ms) may move between the halves of the window
/// before it counts as a trend rather than noise.
const TREND_EPSILON: f32 = 0.5;

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

/// Timed keystrokes inside one half of the window, as a weighted mean interval.
#[derive(Debug, Default)]
struct HalfAcc {
    dt_weighted_sum: f32,
    dt_weight: u32,
}

impl HalfAcc {
    fn add(&mut self, agg: &KeyAgg) {
        // The hit count weights a per-record mean of intervals, as in `Totals`.
        if agg.mean_dt > 0 && agg.hits > 0 {
            self.dt_weighted_sum += agg.mean_dt as f32 * agg.hits as f32;
            self.dt_weight += agg.hits;
        }
    }

    /// Mean interval over this half, or `None` when nothing was timed.
    fn mean_dt(&self) -> Option<f32> {
        (self.dt_weight > 0).then(|| self.dt_weighted_sum / self.dt_weight as f32)
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
        // Faster is better, and shorter intervals are faster: a smaller mean
        // interval on the newer half means the key is improving. Accuracy is
        // deliberately not consulted here - it is reported on its own column,
        // and mixing it in made "improving" claim things the intervals deny.
        match (self.older_acc.mean_dt(), self.newer_acc.mean_dt()) {
            (Some(older), Some(newer)) if older - newer > TREND_EPSILON => Trend::Up,
            (Some(older), Some(newer)) if newer - older > TREND_EPSILON => Trend::Down,
            // One half is untimed, or the two agree: nothing to claim.
            _ => Trend::Flat,
        }
    }
}

/// The history as JSONL: one record per line, the same shape the file holds.
pub fn export_jsonl(records: &[TestRecord]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for rec in records {
        out.push_str(&serde_json::to_string(rec)?);
        out.push('\n');
    }
    Ok(out)
}

/// The columns of the TSV export, in order. The header is this list, so a
/// column cannot be added to one side only.
const TSV_COLUMNS: [&str; 13] = [
    "ts",
    "layout",
    "host",
    "emulated",
    "lang",
    "mode",
    "lesson",
    "wpm",
    "raw_wpm",
    "accuracy",
    "duration_ms",
    "unlocked_before",
    "unlocked_after",
];

/// The history as TSV: a header row, then one row per record. Per-key totals
/// are left out - they are a nested map, and a spreadsheet column would have to
/// lie about that; JSONL is the lossless export.
pub fn export_tsv(records: &[TestRecord]) -> String {
    let mut out = TSV_COLUMNS.join("\t");
    out.push('\n');
    for rec in records {
        let row = [
            rec.ts.clone(),
            rec.layout.clone(),
            rec.host.clone(),
            rec.emulated.to_string(),
            rec.lang.clone(),
            rec.mode.to_string(),
            rec.lesson.to_string(),
            format!("{:.2}", rec.wpm),
            format!("{:.2}", rec.raw_wpm),
            format!("{:.4}", rec.accuracy),
            rec.duration_ms.to_string(),
            rec.unlocked_before.to_string(),
            rec.unlocked_after.to_string(),
        ];
        out.push_str(&row.join("\t"));
        out.push('\n');
    }
    out
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

    /// A record that clears the charting floor (10 s, 50 keystrokes), for the
    /// tests that are about the series, the personal best or consistency.
    fn charted(ts: &str, mode: Mode, wpm: f32, g: f32) -> TestRecord {
        let mut rec = record(ts, mode, wpm, g);
        rec.keys = [('e', key(60, 1, 150))].into_iter().collect();
        rec
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
    fn append_writes_each_record_and_its_newline_as_one_line() {
        let scratch = Scratch::new("atomic");
        let path = scratch.file();
        let records = [
            record("2026-11-15T10:00:00Z", Mode::Pseudo, 61.0, 0.0),
            record("2026-11-15T10:05:00Z", Mode::Words, 62.5, 1.0),
        ];
        for rec in &records {
            append(&path, rec).expect("append");
        }
        // The object and its `\n` go out in a single `write_all`, so the file is
        // exactly the serialised lines back to back: a second writer cannot land
        // between an object and its terminator and put two objects on one line.
        let raw = fs::read_to_string(&path).expect("the file");
        let expected: String = records
            .iter()
            .map(|rec| format!("{}\n", serde_json::to_string(rec).expect("encode")))
            .collect();
        assert_eq!(raw, expected, "one write per record, newline included");
        assert_eq!(
            raw.matches('\n').count(),
            records.len(),
            "exactly one newline per record"
        );
        for line in raw.lines() {
            serde_json::from_str::<TestRecord>(line).expect("every line parses");
        }
        assert_eq!(read_all(&path).expect("read back"), records.to_vec());
    }

    #[test]
    fn a_record_missing_a_later_added_field_still_parses() {
        // The shape an older keytle wrote, before `host`, `emulated`, `lesson`
        // and `keys` existed. A future added field must not brick startup.
        let line = concat!(
            r#"{"v":1,"ts":"2026-11-15T10:00:00Z","layout":"colemak-dh","lang":"en","#,
            r#""mode":"pseudo","wpm":50.0,"raw_wpm":52.0,"accuracy":0.96,"#,
            r#""duration_ms":30000,"unlocked_before":4,"unlocked_after":5}"#,
        );
        let rec: TestRecord = serde_json::from_str(line).expect("old line parses");
        assert_eq!(rec.host, "");
        assert!(!rec.emulated);
        assert_eq!(rec.lesson, 0);
        assert!(rec.keys.is_empty());
        assert_eq!(rec.wpm, 50.0, "present fields still win");

        // `KeyAgg` gains a field the same way.
        let agg: KeyAgg = serde_json::from_str(r#"{"hits":3}"#).expect("short KeyAgg parses");
        assert_eq!(
            agg,
            KeyAgg {
                hits: 3,
                misses: 0,
                shift_misses: 0,
                mean_dt: 0,
            }
        );
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
    fn a_test_under_the_floor_is_counted_but_not_charted() {
        let mut short = record("2026-11-15T09:00:00Z", Mode::Words, 95.0, 0.0);
        short.duration_ms = 4_900;
        short.keys = [('e', key(39, 0, 150))].into_iter().collect();
        let records = vec![
            short,
            charted("2026-11-15T09:05:00Z", Mode::Words, 60.0, 0.0),
        ];
        let points = series(&records, &Filter::default());
        assert_eq!(points.len(), 1, "only the full-length test is a column");
        assert_eq!(points[0].wpm, 60.0);
        assert_eq!(
            personal_best(&records, &Filter::default()),
            Some(60.0),
            "a five-second burst is not a personal best"
        );
    }

    #[test]
    fn series_is_indexed_by_test_not_by_date() {
        // Three tests over two days: two on the 15th, one on the 17th. The index
        // is the test number, so the missing 16th leaves no gap and no bucket.
        let records = vec![
            charted("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
            charted("2026-11-15T09:10:00Z", Mode::Pseudo, 50.0, 0.0),
            charted("2026-11-17T21:00:00Z", Mode::Pseudo, 60.0, 0.0),
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
                charted(
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
            charted("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
            charted("2026-11-15T09:10:00Z", Mode::Sentences, 30.0, 0.0),
            charted("2026-11-15T09:20:00Z", Mode::Pseudo, 45.0, 0.0),
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
        let mut other = charted("2026-11-15T09:30:00Z", Mode::Pseudo, 99.0, 0.0);
        other.layout = "bepo".to_string();
        let mut french = charted("2026-11-15T09:40:00Z", Mode::Pseudo, 88.0, 0.0);
        french.lang = "fr".to_string();
        let records = vec![
            charted("2026-11-15T09:00:00Z", Mode::Pseudo, 40.0, 0.0),
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
            charted("2026-11-15T09:00:00Z", Mode::Pseudo, 60.0, 0.0),
            charted("2026-11-15T09:10:00Z", Mode::Pseudo, 71.0, 0.0),
            charted("2026-11-15T09:20:00Z", Mode::Sentences, 44.0, 0.0),
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
    fn a_key_whose_newer_half_is_faster_trends_up() {
        // Same accuracy in both halves (4 hits, 1 miss each), so only the
        // interval can drive the verdict: 200 ms → 100 ms is faster.
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(4, 1, 200)),
            with_key("2026-11-15T09:05:00Z", 'k', key(4, 1, 200)),
            with_key("2026-11-15T09:10:00Z", 'k', key(4, 1, 100)),
            with_key("2026-11-15T09:15:00Z", 'k', key(4, 1, 100)),
        ];
        let rows = key_rows(&records, &Filter::default(), 20, 150.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ch, 'k');
        assert_eq!(rows[0].trend, Trend::Up);
        assert!(close(rows[0].accuracy, 0.8), "windowed accuracy");
        assert_eq!(rows[0].samples, 20);

        // The mirror image, so the sign is not an accident of the ordering.
        let worse = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(4, 1, 100)),
            with_key("2026-11-15T09:05:00Z", 'k', key(4, 1, 100)),
            with_key("2026-11-15T09:10:00Z", 'k', key(4, 1, 200)),
            with_key("2026-11-15T09:15:00Z", 'k', key(4, 1, 200)),
        ];
        assert_eq!(
            key_rows(&worse, &Filter::default(), 20, 150.0)[0].trend,
            Trend::Down
        );
        // Same interval either side of the midpoint: nothing to report.
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
    fn identical_accuracy_but_a_faster_newer_half_trends_up() {
        // Grades never move - 9 hits, 1 miss on both sides - but the key is
        // typed twice as fast in the newer half. That is improvement.
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'e', key(9, 1, 240)),
            with_key("2026-11-15T09:05:00Z", 'e', key(9, 1, 120)),
        ];
        let row = key_rows(&records, &Filter::default(), 20, 150.0)[0];
        assert!(close(row.accuracy, 0.9));
        assert_eq!(row.trend, Trend::Up);
    }

    #[test]
    fn identical_accuracy_but_a_slower_newer_half_trends_down() {
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'e', key(9, 1, 120)),
            with_key("2026-11-15T09:05:00Z", 'e', key(9, 1, 240)),
        ];
        let row = key_rows(&records, &Filter::default(), 20, 150.0)[0];
        assert!(close(row.accuracy, 0.9));
        assert_eq!(row.trend, Trend::Down);
    }

    #[test]
    fn trend_ignores_accuracy() {
        // Accuracy soars in the newer half while the interval is unchanged.
        // Speed is what the trend measures, so this stays Flat.
        let records = vec![
            with_key("2026-11-15T09:00:00Z", 'k', key(2, 8, 150)),
            with_key("2026-11-15T09:05:00Z", 'k', key(10, 0, 150)),
        ];
        let row = key_rows(&records, &Filter::default(), 20, 150.0)[0];
        assert_eq!(row.trend, Trend::Flat, "accuracy does not move the trend");
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
