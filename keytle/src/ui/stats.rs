//! The stats dashboard: speed by **test index** (never by date), the per-key
//! table, and the next unlock.
//!
//! Every number here is produced by [`keytle_core::stats`] — `series`,
//! `personal_best`, `consistency`, `key_rows`. The view's only job is to pick a
//! width, choose glyphs and lay the result out; if this file ever starts folding
//! records itself, the chart and `keytle stats` would be free to disagree.
//!
//! The X axis is the test number: a hundred tests in one afternoon are a hundred
//! columns, and a week away leaves no gap. Session breaks — where the profile's
//! unlock budget reset after thirty idle minutes — are marked with `┊` under the
//! chart, because that is the only thing about the history that is genuinely
//! about time rather than about tests.

use ratatui::layout::{Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use keytle_core::engine::profile::SESSION_IDLE_MS;
use keytle_core::stats::{self, Filter, KeyRow, Point, TestRecord};

use crate::app::{parse_rfc3339 as parse_ts, App};
use crate::ui::{fmt_duration, trend_glyph, unlock_line};

/// How tall the speed chart is, in block rows.
const CHART_ROWS: usize = 4;
/// The per-key table's window, in tests. The engine's trend compares the halves.
const KEY_WINDOW: usize = 20;
/// Keys drawn side by side in the table.
const KEYS_PER_LINE: usize = 3;
/// The eight levels a block glyph can show.
const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// Marks the first test after an idle gap long enough to reset the unlock budget.
const SESSION_MARK: char = '┊';

/// Draw the dashboard for the app's current (layout, language, mode) selection.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app.stats.to_filter(app.config.mode);
    let points = stats::series(&app.records, &filter);
    if points.is_empty() {
        empty_state(frame, area, app.records.is_empty());
        return;
    }
    // The same predicate the store uses, so "total time" describes exactly the
    // points on the chart.
    let matching: Vec<&TestRecord> = app.records.iter().filter(|rec| filter.matches(rec)).collect();
    let total_ms: u64 = matching.iter().map(|rec| rec.duration_ms).sum::<u64>();
    let keys = stats::key_rows(
        &app.records,
        &filter,
        KEY_WINDOW,
        app.config.target_interval_ms(),
    );
    let best = stats::personal_best(&app.records, &filter);
    let speed: Vec<f32> = points.iter().map(|point| point.wpm).collect();

    let block = Block::bordered().title(header(app, &filter, points.len(), total_ms));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = UiLayout::vertical([
        Constraint::Length(CHART_ROWS as u16),
        Constraint::Length(1), // session breaks, under their own columns
        Constraint::Length(1), // the test-number axis
        Constraint::Length(1), // accuracy / consistency / personal best
        Constraint::Length(1),
        Constraint::Min(2),    // the key table
        Constraint::Length(3), // the unlock panel
        Constraint::Length(1), // the key hints
    ])
    .split(inner);

    // Only the newest tests fit; the axis says which numbers they are.
    let width = usize::from(chunks[0].width);
    let first = points.len().saturating_sub(width);
    let shown = &points[first..];

    let max_wpm = shown.iter().map(|point| point.wpm).fold(0.0_f32, f32::max);
    let chart: Vec<Line> = chart_rows(shown, max_wpm)
        .into_iter()
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(chart), chunks[0]);

    frame.render_widget(
        Paragraph::new(Line::from(session_row(
            &session_starts(&matching),
            first,
            width,
        ))),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(axis_line(shown, width))),
        chunks[2],
    );

    let accuracy = shown
        .last()
        .map(|point| point.accuracy)
        .unwrap_or(0.0);
    let summary = format!(
        "acc {:.1}%  ·  consistency {:.1}/5  ·  pb {}",
        accuracy * 100.0,
        stats::consistency(&speed),
        best.map(|wpm| format!("{wpm:.0} wpm"))
            .unwrap_or_else(|| "—".to_string()),
    );
    frame.render_widget(
        Paragraph::new(Line::from(summary))
            .style(Style::new().fg(Color::Gray))
            .wrap(Wrap { trim: true }),
        chunks[3],
    );

    frame.render_widget(
        Paragraph::new(key_lines(app, &keys))
            .block(Block::bordered().title(format!(
                "keys · last {KEY_WINDOW} tests · weakest first"
            )))
            .wrap(Wrap { trim: true }),
        chunks[5],
    );

    let outcome = app.unlock_outcome();
    frame.render_widget(
        Paragraph::new(Line::from(unlock_line(&outcome)))
            .block(Block::bordered().title("next unlock"))
            .style(Style::new().fg(Color::Green))
            .wrap(Wrap { trim: true }),
        chunks[6],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "s/S layout  ·  l language  ·  m merge modes  ·  a all layouts  ·  tab back",
            Style::new().fg(Color::DarkGray),
        ))),
        chunks[7],
    );
}

/// The one-line title: what is plotted, how much of it, and how long it took.
fn header(app: &App, filter: &Filter, tests: usize, total_ms: u64) -> String {
    let layout = filter.layout.as_deref().unwrap_or("all layouts");
    let lang = filter.lang.as_deref().unwrap_or("all languages");
    let merged = if app.stats.merged { " · (merged)" } else { "" };
    format!(
        "progress · {layout}/{lang}{merged} · {tests} test{} · {}",
        if tests == 1 { "" } else { "s" },
        fmt_duration(total_ms)
    )
}

/// The chart, top row first: one column per test, `▁▂▃▄▅▆▇█` scaled to the best
/// test on screen.
fn chart_rows(points: &[Point], max_wpm: f32) -> Vec<String> {
    let max = if max_wpm.is_finite() && max_wpm > 0.0 {
        max_wpm
    } else {
        1.0
    };
    // Everything above zero shows at least a sliver, so a flat series is a line
    // of blocks rather than a blank panel.
    let full = (CHART_ROWS * LEVELS.len()) as f32;
    let mut grid = vec![vec![' '; points.len()]; CHART_ROWS];
    for (col, point) in points.iter().enumerate() {
        let value = if point.wpm.is_finite() { point.wpm.max(0.0) } else { 0.0 };
        let units = ((value / max) * full).ceil().clamp(0.0, full) as usize;
        for (row, line) in grid.iter_mut().enumerate() {
            let base = (CHART_ROWS - 1 - row) * LEVELS.len();
            if units >= base + LEVELS.len() {
                line[col] = '█';
            } else if units > base {
                line[col] = LEVELS[(units - base - 1).min(LEVELS.len() - 1)];
            }
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// The row under the chart marking the first test of each new session.
fn session_row(breaks: &[usize], first: usize, width: usize) -> String {
    let mut row = vec![' '; width];
    for &at in breaks {
        if let Some(col) = at.checked_sub(first).filter(|col| *col < width) {
            row[col] = SESSION_MARK;
        }
    }
    row.into_iter().collect::<String>().trim_end().to_string()
}

/// Which tests begin a new session: the gap since the previous recorded test is
/// at least [`SESSION_IDLE_MS`], the same rule that resets the unlock budget.
/// Positions are 0-based within `records`; test 0 is never a break.
fn session_starts(records: &[&TestRecord]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut previous: Option<u64> = None;
    for (position, rec) in records.iter().enumerate() {
        let Some(ts) = parse_ts(&rec.ts) else {
            previous = None;
            continue;
        };
        if previous.is_some_and(|prev| ts.saturating_sub(prev) >= SESSION_IDLE_MS) {
            starts.push(position);
        }
        previous = Some(ts);
    }
    starts
}

/// The test-number axis: `tests` and then every tenth index under its column.
/// Never a date — the axis is the count of tests taken.
fn axis_line(points: &[Point], width: usize) -> String {
    let mut row = vec![' '; width];
    for (i, ch) in "tests".chars().enumerate() {
        if i < width {
            row[i] = ch;
        }
    }
    let mut cursor = 6;
    let last = points.last().map(|point| point.index);
    for (col, point) in points.iter().enumerate() {
        if point.index % 10 != 0 && Some(point.index) != last {
            continue;
        }
        let label: Vec<char> = point.index.to_string().chars().collect();
        let start = col.max(cursor);
        if start + label.len() > width {
            continue;
        }
        let len = label.len();
        for (offset, ch) in label.into_iter().enumerate() {
            row[start + offset] = ch;
        }
        cursor = start + len + 1;
    }
    row.into_iter().collect::<String>().trim_end().to_string()
}

/// The key table, three keys per line, weakest first.
fn key_lines(app: &App, rows: &[KeyRow]) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![Line::from(Span::styled(
            "no per-key evidence yet — finish a lesson",
            Style::new().fg(Color::DarkGray),
        ))];
    }
    let unlocked = &app.profile.unlocked;
    rows.chunks(KEYS_PER_LINE)
        .enumerate()
        .map(|(line_no, chunk)| {
            let mut spans = Vec::new();
            for row in chunk {
                let style = if unlocked.contains(&row.ch) {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new().fg(Color::Magenta)
                };
                spans.push(Span::styled(
                    format!(
                        "{} {:>3.0}% {:.1}/5 {}  ",
                        row.ch,
                        row.accuracy * 100.0,
                        (row.confidence * 5.0).clamp(0.0, 5.0),
                        trend_glyph(row.trend),
                    ),
                    style,
                ));
            }
            if line_no == 0 {
                spans.push(Span::styled(
                    "◀ weakest",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::DIM),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// No tests at all is a state, not an error: say what to do about it.
fn empty_state(frame: &mut Frame, area: Rect, no_file: bool) {
    let why = if no_file {
        "no tests yet — finish a lesson and come back"
    } else {
        "no tests match this filter — press s or a to widen it"
    };
    let lines = vec![
        Line::from(Span::styled(why, Style::new().fg(Color::Yellow))),
        Line::from(""),
        Line::from(Span::styled(
            "s/S layout  ·  l language  ·  m merge modes  ·  a all layouts  ·  tab back",
            Style::new().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("progress"))
            .alignment(ratatui::layout::Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::draw;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use keytle_core::config::{Config, Mode};
    use keytle_core::paths::Paths;
    use keytle_core::stats::{append, TestRecord, RECORD_VERSION};
    use keytle_core::typing::KeyAgg;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::BTreeMap;

    fn scratch(tag: &str) -> (std::path::PathBuf, Paths) {
        let base = std::env::temp_dir().join(format!("keytle-dash-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
        };
        (base, paths)
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let width = buf.area().width as usize;
        buf.content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn record(ts: &str, wpm: f32, key: Option<(char, KeyAgg)>) -> TestRecord {
        let mut keys = BTreeMap::new();
        if let Some((ch, agg)) = key {
            keys.insert(ch, agg);
        }
        TestRecord {
            v: RECORD_VERSION,
            ts: ts.to_string(),
            layout: "colemak-dh".to_string(),
            host: "qwerty".to_string(),
            emulated: true,
            lang: "en".to_string(),
            mode: Mode::Pseudo,
            lesson: 0,
            wpm,
            raw_wpm: wpm + 2.0,
            accuracy: 0.97,
            duration_ms: 60_000,
            keys,
            unlocked_before: 4,
            unlocked_after: 4,
        }
    }

    /// Thirty steady tests, one long pause after the fifteenth so there is a
    /// session break to draw. Times are deliberately spread over two months: the
    /// axis must not care.
    fn fixture(paths: &Paths) -> std::path::PathBuf {
        let path = paths.tests_file();
        for i in 0..30u64 {
            let start = 1_780_000_000_000 + i * 60_000 + if i >= 15 { 2 * 3_600_000 } else { 0 };
            let key = ('n', KeyAgg { hits: 10, misses: u32::from(i % 3 == 0), shift_misses: 0, mean_dt: 150 });
            append(&path, &record(&crate::app::rfc3339(start), 40.0 + i as f32, Some(key))).unwrap();
        }
        path
    }

    #[test]
    fn the_axis_is_test_numbers_not_dates() {
        let (base, paths) = scratch("axis");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        let axis = text
            .lines()
            .find(|line| line.contains("tests") && line.contains("10") && line.contains("20"))
            .unwrap_or_else(|| panic!("no axis line:\n{text}"));
        assert!(axis.contains("10"), "tenth test is labelled:\n{axis}");
        assert!(axis.contains("20"), "twentieth test is labelled:\n{axis}");
        assert!(axis.contains("30"), "the last test is labelled:\n{axis}");
        assert!(!axis.contains('-'), "no dates on the axis:\n{axis}");
        assert!(!axis.contains(':'), "no times on the axis:\n{axis}");
        assert!(text.contains("pb "), "{text}");
        assert!(text.contains("consistency"), "{text}");
        assert!(text.contains("30 tests"), "the count is in the header:\n{text}");
        assert!(text.contains('▁') || text.contains('█'), "block glyphs are drawn");
        assert!(text.contains('┊'), "the session break is marked:\n{text}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn m_merges_modes_and_says_so_in_the_header() {
        let (base, paths) = scratch("merge");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(!screen(&terminal).contains("(merged)"));

        app.handle_key(
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            0,
        );
        assert!(app.stats.merged);
        assert_eq!(app.stats.to_filter(Mode::Pseudo).mode, None);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains("(merged)"), "{text}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn cycling_the_layout_and_language_moves_the_header() {
        let (base, paths) = scratch("cycle");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        assert_eq!(app.stats.layout.as_deref(), Some("colemak-dh"));
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), 0);
        assert_eq!(app.stats.layout, None, "`a` is every layout");
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE), 0);
        assert_eq!(app.stats.layout.as_deref(), Some("colemak-dh"), "s wraps round");
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE), 0);
        assert_eq!(app.stats.lang, None, "the second language choice is all of them");
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), 0);
        assert_eq!(app.screen, crate::app::Screen::Typing, "tab goes back to typing");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn an_empty_history_is_a_state_not_a_panic() {
        let (base, paths) = scratch("empty");
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains("no tests yet"), "{text}");
        assert!(text.contains("progress"), "the panel still has a title:\n{text}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_empty_state_says_so_when_the_filter_matches_nothing() {
        let (base, paths) = scratch("filtered-out");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        app.stats.lang = Some("fr".to_string());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains("no tests match this filter"), "{text}");
        let _ = std::fs::remove_dir_all(base);
    }
}
