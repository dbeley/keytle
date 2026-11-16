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
//! unlock budget reset after [`SESSION_IDLE_MS`] of idle time — are marked with
//! `┊` under the chart, and every test that unlocked a letter with `▲`, because
//! those are the only parts of the history that are events rather than numbers.
//! The personal best is a `━` line drawn across the speed chart at its own
//! height, and `c` cycles the chart between speed, accuracy and both.

use ratatui::layout::{Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use keytle_core::engine::profile::SESSION_IDLE_MS;
use keytle_core::stats::{self, Filter, KeyRow, Point, TestRecord};

use crate::app::{parse_rfc3339 as parse_ts, App, ChartMode};
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
/// Marks the test that unlocked a letter.
const UNLOCK_MARK: char = '▲';
/// The personal-best line drawn across the speed chart at the best test's
/// height. Heavy, so it cannot be confused with a panel's light border.
const PB_MARK: char = '━';

/// Draw the dashboard for the app's current (layout, language, mode) selection.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app.stats.to_filter(app.config.mode);
    let points = stats::series(&app.records, &filter);
    if points.is_empty() {
        empty_state(frame, area, app);
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
    let mode = app.stats.chart;
    let speed_rows = matches!(mode, ChartMode::Speed | ChartMode::Both);
    let accuracy_rows = matches!(mode, ChartMode::Accuracy | ChartMode::Both);

    let block = Block::bordered().title(header(app, &filter, points.len(), total_ms));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The rows are planned up front so the error line and the two chart halves
    // only take the space they actually need.
    let mut plan: Vec<Constraint> = Vec::new();
    if app.history_error().is_some() {
        plan.push(Constraint::Length(1));
    }
    if speed_rows {
        plan.push(Constraint::Length(CHART_ROWS as u16));
    }
    if accuracy_rows {
        plan.push(Constraint::Length(CHART_ROWS as u16));
    }
    plan.push(Constraint::Length(1)); // the personal best, under its chart
    plan.push(Constraint::Length(1)); // session breaks and unlocks
    plan.push(Constraint::Length(1)); // the test-number axis
    plan.push(Constraint::Length(1)); // accuracy / consistency / personal best
    plan.push(Constraint::Length(1));
    plan.push(Constraint::Min(2)); // the key table
    plan.push(Constraint::Length(3)); // the unlock panel
    plan.push(Constraint::Length(1)); // the key hints
    let chunks = UiLayout::vertical(plan).split(inner);
    let mut rows = chunks.iter();

    // Only the newest tests fit; the axis says which numbers they are.
    let width = usize::from(chunks[0].width);
    let first = points.len().saturating_sub(width);
    let shown = &points[first..];

    if let Some(err) = app.history_error() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(err, Style::new().fg(Color::Yellow))))
                .wrap(Wrap { trim: true }),
            *rows.next().expect("the error row is planned"),
        );
    }

    if speed_rows {
        // The personal best is drawn as a horizontal line at its own height, so
        // the series is scaled with one row of headroom above it: the line then
        // sits below the top row and stays visible where the bars are lower.
        let shown_max = shown.iter().map(|point| point.wpm).fold(0.0_f32, f32::max);
        let scale = match best {
            Some(pb) if pb > 0.0 => {
                (pb * CHART_ROWS as f32 / (CHART_ROWS - 1) as f32).max(shown_max)
            }
            _ => shown_max,
        };
        let values: Vec<f32> = shown.iter().map(|point| point.wpm).collect();
        render_chart(frame, *rows.next().expect("a speed row"), &values, scale, best);
    }
    if accuracy_rows {
        // Accuracy is always on its own scale: 0 to 100%, never relative to the
        // best test, so a flat 97% reads as a full column and not as noise.
        let values: Vec<f32> = shown.iter().map(|point| point.accuracy).collect();
        render_chart(frame, *rows.next().expect("an accuracy row"), &values, 1.0, None);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            best_line(shown, mode),
            Style::new().fg(Color::Gray),
        ))),
        *rows.next().expect("the personal-best row"),
    );
    frame.render_widget(
        Paragraph::new(Line::from(marker_row(&matching, first, width))),
        *rows.next().expect("the marker row"),
    );
    frame.render_widget(
        Paragraph::new(Line::from(axis_line(shown, width))),
        *rows.next().expect("the axis row"),
    );

    let accuracy = shown.last().map(|point| point.accuracy).unwrap_or(0.0);
    let summary = format!(
        "acc {:.1}%  ·  consistency {:.1}/5  ·  pb {}  ·  {UNLOCK_MARK} unlock",
        accuracy * 100.0,
        stats::consistency(&speed),
        best.map(|wpm| format!("{wpm:.0} wpm"))
            .unwrap_or_else(|| "—".to_string()),
    );
    frame.render_widget(
        Paragraph::new(Line::from(summary))
            .style(Style::new().fg(Color::Gray))
            .wrap(Wrap { trim: true }),
        *rows.next().expect("the summary row"),
    );

    rows.next(); // the blank separator
    frame.render_widget(
        Paragraph::new(key_lines(app, &keys))
            .block(Block::bordered().title(format!(
                "keys · last {KEY_WINDOW} tests · weakest first"
            )))
            .wrap(Wrap { trim: true }),
        *rows.next().expect("the key table"),
    );

    let outcome = app.unlock_outcome();
    frame.render_widget(
        Paragraph::new(Line::from(unlock_line(&outcome)))
            .block(Block::bordered().title("next unlock"))
            .style(Style::new().fg(Color::Green))
            .wrap(Wrap { trim: true }),
        *rows.next().expect("the unlock panel"),
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "s/S layout  ·  l language  ·  c chart  ·  m merge modes  ·  a all layouts  ·  tab back",
            Style::new().fg(Color::DarkGray),
        ))),
        *rows.next().expect("the hints row"),
    );
}

/// One chart: `▁▂▃▄▅▆▇█` columns scaled to `max`, top row first. `marker`, when
/// given, is a value to draw a horizontal line at (the personal best).
fn render_chart(frame: &mut Frame, area: Rect, values: &[f32], max: f32, marker: Option<f32>) {
    let lines: Vec<Line> = chart_rows(values, max, marker)
        .into_iter()
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The one-line title: what is plotted, how much of it, and how long it took.
fn header(app: &App, filter: &Filter, tests: usize, total_ms: u64) -> String {
    let layout = filter.layout.as_deref().unwrap_or("all layouts");
    let lang = filter.lang.as_deref().unwrap_or("all languages");
    let merged = if app.stats.merged { " · (merged)" } else { "" };
    format!(
        "progress · {layout}/{lang}{merged} · {tests} test{} · {} · {} chart",
        if tests == 1 { "" } else { "s" },
        fmt_duration(total_ms),
        app.stats.chart.label()
    )
}

/// The chart, top row first: one column per test, `▁▂▃▄▅▆▇█` scaled to `max`.
/// A `marker` value is drawn as a horizontal line of [`PB_MARK`] in the row its
/// height lands in, filling only the cells a bar has not already reached, so
/// bars above the line still read as full columns.
fn chart_rows(values: &[f32], max: f32, marker: Option<f32>) -> Vec<String> {
    let max = if max.is_finite() && max > 0.0 { max } else { 1.0 };
    let points: Vec<f32> = values.to_vec();
    // Everything above zero shows at least a sliver, so a flat series is a line
    // of blocks rather than a blank panel.
    let full = (CHART_ROWS * LEVELS.len()) as f32;
    let mut grid = vec![vec![' '; points.len()]; CHART_ROWS];
    for (col, value) in points.iter().enumerate() {
        let value = if value.is_finite() { value.max(0.0) } else { 0.0 };
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
    if let Some(marker) = marker.filter(|value| value.is_finite() && *value > 0.0) {
        // The row the marker's own block would top out in, counted from the
        // bottom, then flipped into the grid's top-down order.
        let units = ((marker / max) * full).ceil().clamp(1.0, full) as usize;
        let from_bottom = (units - 1) / LEVELS.len();
        let row = CHART_ROWS - 1 - from_bottom.min(CHART_ROWS - 1);
        for cell in grid[row].iter_mut() {
            if *cell == ' ' {
                *cell = PB_MARK;
            }
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// The row under the chart marking both things that are events rather than
/// numbers: the first test of each new session (`┊`) and every test that
/// unlocked a letter (`▲`). Both are placed under the column of the test they
/// describe, and an unlock wins a shared column because it is the rarer event.
fn marker_row(records: &[&TestRecord], first: usize, width: usize) -> String {
    let mut row = vec![' '; width];
    let mut place = |at: usize, glyph: char| {
        if let Some(col) = at.checked_sub(first).filter(|col| *col < width) {
            row[col] = glyph;
        }
    };
    for at in session_starts(records) {
        place(at, SESSION_MARK);
    }
    for (at, rec) in records.iter().enumerate() {
        if rec.unlocked_after > rec.unlocked_before {
            place(at, UNLOCK_MARK);
        }
    }
    row.into_iter().collect::<String>().trim_end().to_string()
}

/// The personal-best line: the best of what is on screen and which test it was,
/// reading the same series the chart above it just drew.
fn best_line(shown: &[Point], mode: ChartMode) -> String {
    let speed = matches!(mode, ChartMode::Speed | ChartMode::Both);
    let value = |point: &Point| if speed { point.wpm } else { point.accuracy * 100.0 };
    let Some(top) = shown
        .iter()
        .filter(|point| value(point).is_finite())
        .max_by(|a, b| value(a).total_cmp(&value(b)))
    else {
        return "pb —".to_string();
    };
    if speed {
        format!("pb {PB_MARK} {:.0} wpm  ·  test {}", value(top), top.index)
    } else {
        format!("pb {:.1}% accuracy  ·  test {}", value(top), top.index)
    }
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

/// No tests at all is a state, not an error: say what to do about it. A history
/// that could not be read says so first, because "no tests" would be a lie.
fn empty_state(frame: &mut Frame, area: Rect, app: &App) {
    let why = if let Some(err) = app.history_error() {
        err
    } else if app.records.is_empty() {
        "no tests yet — finish a lesson and come back".to_string()
    } else {
        "no tests match this filter — press s or a to widen it".to_string()
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
    fn c_cycles_the_chart_through_speed_accuracy_and_both() {
        let (base, paths) = scratch("chart-toggle");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let press = |app: &mut App| {
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), 0);
        };

        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(screen(&terminal).contains("speed chart"), "{}", screen(&terminal));
        assert!(screen(&terminal).contains("pb "), "the personal best is named");

        press(&mut app);
        assert_eq!(app.stats.chart, ChartMode::Accuracy);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains("accuracy chart"), "{text}");
        assert!(text.contains("pb "), "accuracy has its own best: {text}");

        press(&mut app);
        assert_eq!(app.stats.chart, ChartMode::Both);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(screen(&terminal).contains("speed + accuracy chart"));
        // Two stacked charts, so twice the block rows of one.
        let blocks = |text: &str| text.chars().filter(|ch| LEVELS.contains(ch)).count();
        assert!(blocks(&screen(&terminal)) > blocks(&text_of(&paths, &app)));

        press(&mut app);
        assert_eq!(app.stats.chart, ChartMode::Speed, "c wraps back round");
        let _ = std::fs::remove_dir_all(base);
    }

    /// The dashboard in speed-only mode, for a block-count comparison.
    fn text_of(paths: &Paths, app: &App) -> String {
        let mut speed = App::new(Config::default(), paths, 0).unwrap();
        speed.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &speed)).unwrap();
        let _ = app;
        screen(&terminal)
    }

    #[test]
    fn the_speed_chart_draws_the_personal_best_as_a_horizontal_line() {
        let (base, paths) = scratch("pb-line");
        fixture(&paths);
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        let line = text
            .lines()
            .position(|row| row.contains(PB_MARK))
            .unwrap_or_else(|| panic!("the personal best is drawn as a line:\n{text}"));
        let label = text
            .lines()
            .position(|row| row.contains(&format!("pb {PB_MARK}")))
            .unwrap_or_else(|| panic!("the line is named under the chart:\n{text}"));
        assert!(line < label, "the line is on the chart, not the text:\n{text}");
        // It is a line across the chart, not one lonely cell.
        assert!(
            text.lines().nth(line).unwrap().matches(PB_MARK).count() > 1,
            "the marker spans the chart:\n{text}"
        );

        // Accuracy has no personal-best line: its scale is 0–100%, not the best.
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), 0);
        assert_eq!(app.stats.chart, ChartMode::Accuracy);
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(!text.contains(PB_MARK), "no pb line on the accuracy chart:\n{text}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_test_that_unlocked_a_letter_is_marked_on_the_axis() {
        let (base, paths) = scratch("unlock-marks");
        let path = paths.tests_file();
        for i in 0..6u64 {
            let mut rec = record(&crate::app::rfc3339(1_780_000_000_000 + i * 60_000), 40.0 + i as f32, None);
            if i == 3 {
                rec.unlocked_before = 4;
                rec.unlocked_after = 5;
            }
            append(&path, &rec).unwrap();
        }
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.open_stats();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains(UNLOCK_MARK), "the unlock is marked: {text}");
        let marked = text
            .lines()
            .find(|line| line.contains(UNLOCK_MARK))
            .expect("the marker row");
        // The marker sits under the fourth column of the chart (column 0 is the
        // panel's border), one column per test.
        let column = marked
            .chars()
            .position(|ch| ch == UNLOCK_MARK)
            .expect("the marker is in its row");
        assert_eq!(column, 4, "{marked:?}");
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
