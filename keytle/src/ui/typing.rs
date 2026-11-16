//! The live typing view: speed, accuracy, the text, and the board.
//!
//! The view is a pure function of [`App`] — it reads the current session's
//! cursor and totals and draws them. Every region is derived from the frame
//! area through ratatui constraints, so nothing is indexed by hand and a resize
//! cannot panic.
//!
//! The layout gives the text the upper half of the screen and docks the board
//! to the bottom: a lesson is read and typed at, so it is centred both ways in
//! the space above the keyboard rather than pinned to the top-left corner. When
//! no lesson runs the same space holds the home panel — where the drill stands
//! and which keys open the dashboard, the settings and the help.

use keytle_core::typing::SessionStats;
use ratatui::layout::{Alignment, Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::ui::keyboard;
use crate::ui::unlock_line;

/// The narrowest band the lesson text may be drawn in. The text is centred in
/// it, so the rows above and below are margins rather than wasted space.
const TEXT_ROWS: u16 = 5;
/// The width the home panel may grow to before it is centred and left narrow.
const HOME_WIDTH: u16 = 64;

/// Draw the typing screen into the whole frame.
pub fn render(frame: &mut Frame, app: &App) {
    render_view(frame, app, true);
}

/// The same view with the home panel left out, for the settings overlay to sit
/// on: a panel behind a panel reads as a drawing mistake.
pub fn render_backdrop(frame: &mut Frame, app: &App) {
    render_view(frame, app, false);
}

fn render_view(frame: &mut Frame, app: &App, show_home: bool) {
    let area = frame.area();
    let legends = keyboard::Legends::of(&app.layout, app.config.emulate, app.reachable_chars());
    let board = keyboard::board_height(legends);
    let chunks = UiLayout::vertical([
        Constraint::Length(1),      // title
        Constraint::Length(1),      // speed and accuracy
        Constraint::Min(TEXT_ROWS), // the lesson, or the home panel
        Constraint::Length(board),  // the board, docked to the bottom
        Constraint::Length(1),      // keys
    ])
    .split(area);

    frame.render_widget(Paragraph::new(title_line(app)), chunks[0]);
    // An idle screen has no speed to report: the line stays blank until the
    // first keystroke gives it something true to say.
    if !app.is_idle() {
        frame.render_widget(Paragraph::new(metrics_line(app)), chunks[1]);
    }
    if app.session.is_some() {
        render_lesson(frame, chunks[2], app);
    } else if show_home {
        render_home(frame, chunks[2], app);
    }
    keyboard::render(
        frame,
        chunks[3],
        &app.layout,
        app.expected(),
        app.legend(),
        app.reachable_chars(),
    );
    frame.render_widget(Paragraph::new(notice_line(app)), chunks[4]);
}

/// What is being taught, and under which rules.
fn title_line(app: &App) -> Line<'static> {
    let mode = app.config.mode.to_string();
    let mut flags = Vec::new();
    if app.config.strict {
        flags.push("strict");
    }
    if app.config.emulate {
        flags.push("emulating");
    }
    let flags = if flags.is_empty() {
        String::new()
    } else {
        format!(" · {}", flags.join(" · "))
    };
    Line::from(vec![
        Span::styled(
            format!("keytle — {}", app.layout.name),
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ),
        Span::styled(format!("  {mode}{flags}"), dim()),
    ])
}

/// Elapsed time, both speeds, accuracy, and which key comes next.
fn metrics_line(app: &App) -> Line<'static> {
    let stats = app.stats().unwrap_or_default();
    let seconds = stats.duration_ms / 1000;
    let mut spans = vec![
        Span::styled(format!("{seconds:>4}s"), dim()),
        Span::raw("  "),
        Span::styled(format!("net {:>4.0} wpm", stats.net_wpm), plain()),
        Span::raw("  "),
        Span::styled(format!("raw {:>4.0} wpm", stats.raw_wpm), plain()),
        Span::raw("  "),
        Span::styled(format!("{:>4.0}%", accuracy_percent(&stats)), plain()),
    ];
    if let Some(next) = app.expected() {
        spans.push(Span::raw("  next "));
        spans.push(Span::styled(
            format!("{next}"),
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ));
    }
    Line::from(spans)
}

fn accuracy_percent(stats: &SessionStats) -> f32 {
    stats.accuracy * 100.0
}

/// The lesson, centred both ways in the band above the board.
fn render_lesson(frame: &mut Frame, area: Rect, app: &App) {
    let lines = lesson_text(app);
    // A band of `TEXT_ROWS` rows, vertically centred: the leftover rows above
    // and below are the margin that keeps the eye on the text, and the band is
    // tall enough for a twenty-word lesson to wrap without being clipped.
    let height = TEXT_ROWS.min(area.height);
    let top = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(area.x, top, area.width, height);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

/// The lesson with what is typed, current and still ahead styled apart. The
/// word under the cursor is bold, so the eye has a word to aim at rather than a
/// single reversed character.
fn lesson_text(app: &App) -> Vec<Line<'static>> {
    let Some(session) = &app.session else {
        return Vec::new();
    };
    let cursor = session.cursor();
    let errored = app.last_miss.is_some();
    let text = session.text();
    let (word_start, word_end) = word_bounds(text, cursor);
    let mut spans = Vec::with_capacity(text.len());
    for (index, ch) in text.iter().enumerate() {
        let base = match index.cmp(&cursor) {
            std::cmp::Ordering::Less => typed(),
            std::cmp::Ordering::Equal => {
                if errored {
                    error()
                } else {
                    current()
                }
            }
            std::cmp::Ordering::Greater => plain(),
        };
        let style = if (word_start..word_end).contains(&index) {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    vec![Line::from(spans)]
}

/// The bounds of the word the cursor sits in (or at the end of), spaces
/// excluded, so it can be emphasised as one unit.
fn word_bounds(text: &[char], cursor: usize) -> (usize, usize) {
    let mut start = cursor.min(text.len());
    while start > 0 && text[start - 1] != ' ' {
        start -= 1;
    }
    let mut end = cursor.min(text.len());
    while end < text.len() && text[end] != ' ' {
        end += 1;
    }
    (start, end)
}

/// The idle screen: where the drill stands, and every key that moves it on.
///
/// The numbers come from the same folds the dashboard uses, restricted to the
/// configured pair and mode, so "76 wpm" here and "76 wpm" there are the same
/// record. Reading the dashboard is then optional rather than the only way to
/// find out how the last week went.
fn render_home(frame: &mut Frame, area: Rect, app: &App) {
    let summary = app.home_summary();
    let mut lines = Vec::new();
    if summary.tests == 0 {
        lines.push(Line::from(Span::styled(
            "no tests yet — this is where they add up",
            Style::new().fg(Color::DarkGray),
        )));
    } else {
        // Two short lines rather than one long one: the panel sizes itself to
        // the rows its text needs, and a line that wraps behind its own back
        // used to push the shortcut row off the bottom of the panel.
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} test{}", summary.tests, plural(summary.tests)),
                Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan),
            ),
            Span::styled(
                format!("  ·  latest {:.0} wpm", summary.latest_wpm),
                plain(),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "best {}  ·  consistency {:.1}/5",
                summary
                    .best_wpm
                    .map(|wpm| format!("{wpm:.0} wpm"))
                    .unwrap_or_else(|| "—".to_string()),
                summary.consistency
            ),
            plain(),
        )));
        let weak = if summary.weakest.is_empty() {
            "—".to_string()
        } else {
            summary
                .weakest
                .iter()
                .map(|row| format!("{} {:.0}%", row.ch, row.accuracy * 100.0))
                .collect::<Vec<_>>()
                .join("  ·  ")
        };
        lines.push(home_row("weakest", weak, Color::Yellow));
    }
    lines.push(home_row(
        "next unlock",
        unlock_line(&app.unlock_outcome()),
        Color::Green,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("enter", bold_key()),
        Span::styled(" or any letter starts a lesson", plain()),
    ]));

    let width = area.width.saturating_sub(4).min(HOME_WIDTH);
    // The height follows the *wrapped* text, not the number of `Line`s: a line
    // that needs two rows must be paid for, or the rows below it are clipped.
    let inner = usize::from(width.saturating_sub(2)).max(1);
    let text_rows: usize = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(inner).max(1))
        .sum();
    let height = area.height.min(
        u16::try_from(text_rows)
            .unwrap_or(u16::MAX)
            .saturating_add(2),
    );
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(format!(
                "{} · {} · {}",
                app.layout.name, app.config.language, app.config.mode
            )))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        rect,
    );
}

/// A `label  value` line for the home panel, centred like the rest of it.
fn home_row(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}  "), Style::new().fg(Color::DarkGray)),
        Span::styled(value, Style::new().add_modifier(Modifier::BOLD).fg(color)),
    ])
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// One line under the board: an abort, a generator note, or the key map.
fn notice_line(app: &App) -> Line<'static> {
    if let Some(notice) = &app.notice {
        return Line::from(Span::styled(notice.clone(), Style::new().fg(Color::Yellow)));
    }
    if app.is_idle() {
        // The ways in live on the fixed-height footer, not only inside the home
        // panel: a panel that has to shrink on a short terminal can lose a row,
        // and the settings key is the one a new typist looks for first.
        return Line::from(vec![
            Span::styled("enter", bold_key()),
            Span::styled(" start  ·  ", dim()),
            Span::styled("s", bold_key()),
            Span::styled(" stats  ·  ", dim()),
            Span::styled(",", bold_key()),
            Span::styled(" settings  ·  ", dim()),
            Span::styled("?", bold_key()),
            Span::styled(" help  ·  ", dim()),
            Span::styled("q", bold_key()),
            Span::styled(" quit", dim()),
        ]);
    }
    Line::from(vec![
        Span::styled("tab", bold_key()),
        Span::styled(" restart  ·  ", dim()),
        Span::styled("esc", bold_key()),
        Span::styled(" abandon  ·  ", dim()),
        Span::styled("q", bold_key()),
        Span::styled(" quit when idle", dim()),
    ])
}

fn typed() -> Style {
    Style::new().fg(Color::Green)
}

fn current() -> Style {
    Style::new()
        .add_modifier(Modifier::REVERSED | Modifier::BOLD)
        .fg(Color::Cyan)
}

fn error() -> Style {
    Style::new()
        .add_modifier(Modifier::REVERSED | Modifier::BOLD)
        .fg(Color::Red)
}

fn plain() -> Style {
    Style::new().fg(Color::Gray)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM)
}

fn bold_key() -> Style {
    Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use keytle_core::config::Config;
    use keytle_core::paths::Paths;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    struct Scratch {
        paths: Paths,
        base: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let base = std::env::temp_dir().join(format!(
                "keytle-typing-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let paths = Paths {
                config_dir: base.join("config"),
                data_dir: base.join("data"),
            };
            Scratch { paths, base }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn app() -> (Scratch, App) {
        let scratch = Scratch::new();
        let app = App::new(Config::default(), &scratch.paths, 0).unwrap();
        (scratch, app)
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

    fn draw(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        screen(&terminal)
    }

    #[test]
    fn eighty_by_twenty_four_shows_the_lesson_text() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        let screen = draw(&app, 80, 24);
        // The first word must be visible somewhere in the frame.
        let first_word: String = text.chars().take(6).collect();
        assert!(
            screen.contains(&first_word),
            "expected {first_word:?} in:\n{screen}"
        );
        assert!(screen.contains("keytle —"), "{screen}");
        assert!(screen.contains("wpm"), "{screen}");
        assert!(screen.contains('%'), "{screen}");
    }

    #[test]
    fn one_twenty_by_forty_shows_the_lesson_text() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        let screen = draw(&app, 120, 40);
        let first_word: String = text.chars().take(6).collect();
        assert!(screen.contains(&first_word), "{screen}");
    }

    #[test]
    fn the_lesson_text_is_centred_and_above_the_board() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        // Blank the notice so the text is the only long line on screen.
        app.notice = None;
        let screen = draw(&app, 100, 30);
        let rows: Vec<&str> = screen.lines().collect();
        let text_row = rows
            .iter()
            .position(|row| row.contains(&text[..1]))
            .unwrap_or_else(|| panic!("the lesson is drawn:\n{screen}"));
        // The board's first key row is below the text: the text owns the top.
        let board_row = rows
            .iter()
            .position(|row| row.contains('q') && !row.contains(&text[..1]))
            .unwrap_or_else(|| panic!("the board is drawn:\n{screen}"));
        assert!(
            text_row < board_row,
            "text at row {text_row}, board at {board_row}:\n{screen}"
        );
        // Centred means the text is not flush left.
        let first = rows[text_row].find(&text[..1]).unwrap();
        assert!(first > 0, "the lesson starts off the left edge:\n{screen}");
    }

    #[test]
    fn a_small_terminal_is_a_notice_not_a_crash() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let screen = draw(&app, 40, 12);
        assert!(screen.contains("terminal too small"), "{screen}");
        assert!(screen.contains("80"), "{screen}");
        assert!(
            !screen.contains("keytle —"),
            "nothing else is drawn: {screen}"
        );
    }

    #[test]
    fn three_correct_keystrokes_read_one_hundred_percent() {
        let (_scratch, mut app) = app();
        app.config.emulate = false;
        app.start_lesson(0);
        let chars: Vec<char> = app.lesson.as_ref().unwrap().text.chars().collect();
        for (index, ch) in chars.iter().take(3).enumerate() {
            app.handle_key(
                KeyEvent::new(KeyCode::Char(*ch), KeyModifiers::NONE),
                100 + index as u64 * 100,
            );
        }
        let screen = draw(&app, 80, 24);
        assert!(
            screen.contains("100%"),
            "accuracy must read 100%:\n{screen}"
        );
        let net = app.stats().unwrap().net_wpm;
        assert!(net > 0.0, "three hits in 300 ms is a real speed, got {net}");
    }

    #[test]
    fn a_miss_drops_accuracy_and_styles_the_expected_key_as_an_error() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let expected = app.expected().unwrap();
        let wrong = if expected == 'z' { 'y' } else { 'z' };
        // Emulation off keeps the typed character as typed.
        app.config.emulate = false;
        app.handle_key(KeyEvent::new(KeyCode::Char(wrong), KeyModifiers::NONE), 100);
        let screen = draw(&app, 80, 24);
        assert!(
            !screen.contains("100%"),
            "one miss cannot be 100%:\n{screen}"
        );
        assert!(screen.contains('%'), "{screen}");
        assert!(app.last_miss.is_some());
    }

    #[test]
    fn the_idle_screen_invites_a_lesson_and_names_the_ways_in() {
        let (_scratch, app) = app();
        let screen = draw(&app, 100, 30);
        assert!(screen.contains("starts a lesson"), "{screen}");
        assert!(
            screen.contains("stats"),
            "the dashboard is one key away:\n{screen}"
        );
        assert!(screen.contains("settings"), "{screen}");
        assert!(screen.contains("help"), "{screen}");
        assert!(
            screen.contains("no tests yet"),
            "a fresh home says so:\n{screen}"
        );
    }

    #[test]
    fn the_shortcuts_survive_a_home_panel_full_of_history() {
        // A finished test adds three rows to the panel. The panel used to size
        // itself from the number of `Line`s, so the summary line that wrapped
        // pushed the shortcut row off the bottom — on a terminal that is
        // otherwise perfectly adequate.
        let (_scratch, mut app) = app();
        app.config.emulate = false;
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        for (i, ch) in text.chars().enumerate() {
            let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
            if app.handle_key(key, 100 + i as u64) == crate::app::Action::Finish {
                app.finish_lesson(100 + i as u64);
            }
        }
        app.screen = crate::app::Screen::Typing;
        app.notice = None;
        let screen = draw(&app, 80, 24);
        for expected in ["settings", "stats", "help", "latest", "next unlock"] {
            assert!(
                screen.contains(expected),
                "{expected:?} is on the home screen:\n{screen}"
            );
        }
    }

    #[test]
    fn the_home_panel_reports_the_history_it_has() {
        let (_scratch, mut app) = app();
        app.config.emulate = false;
        // One finished lesson is enough for the panel to have numbers.
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        for (i, ch) in text.chars().enumerate() {
            if app.handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                100 + i as u64,
            ) == crate::app::Action::Finish
            {
                app.finish_lesson(100 + i as u64);
            }
        }
        app.screen = crate::app::Screen::Typing;
        let screen = draw(&app, 100, 30);
        assert!(screen.contains("1 test"), "{screen}");
        assert!(screen.contains("latest"), "{screen}");
        assert!(screen.contains("consistency"), "{screen}");
    }
}
