//! The live typing view: speed, accuracy, the text, and the board.
//!
//! The view is a pure function of [`App`] — it reads the current session's
//! cursor and totals and draws them. Every region is derived from the frame
//! area through ratatui constraints, so nothing is indexed by hand and a resize
//! cannot panic.

use keytle_core::typing::SessionStats;
use ratatui::layout::{Constraint, Layout as UiLayout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::ui::keyboard;

/// How many rows the lesson text may use before it is clipped.
const TEXT_ROWS: u16 = 3;

/// Draw the typing screen into the whole frame.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = UiLayout::vertical([
        Constraint::Length(1), // title
        Constraint::Length(1), // speed and accuracy
        Constraint::Length(TEXT_ROWS),
        Constraint::Min(keyboard::board_height(&app.layout)),
        Constraint::Length(1), // notice / keys
    ])
    .split(area);

    frame.render_widget(Paragraph::new(title_line(app)), chunks[0]);
    frame.render_widget(Paragraph::new(metrics_line(app)), chunks[1]);
    frame.render_widget(
        Paragraph::new(lesson_text(app)).wrap(Wrap { trim: false }),
        chunks[2],
    );
    keyboard::render(
        frame,
        chunks[3],
        &app.layout,
        app.expected(),
        app.legend(),
        app.reachable_positions(),
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
            Style::new()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
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
            Style::new()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ));
        if let Some(finger) = finger_of(app, next) {
            spans.push(Span::styled(format!(" ({finger})"), dim()));
        }
    }
    Line::from(spans)
}

fn accuracy_percent(stats: &SessionStats) -> f32 {
    stats.accuracy * 100.0
}

/// The finger that presses `target` on the taught layout, if the layout says.
fn finger_of(app: &App, target: char) -> Option<&'static str> {
    app.layout
        .keys
        .values()
        .find(|key| key.base == target || key.shift == Some(target) || key.altgr == Some(target))
        .and_then(|key| key.finger)
        .map(keyboard::finger_name)
}

/// The lesson with what is typed, current and still ahead styled apart.
fn lesson_text(app: &App) -> Vec<Line<'static>> {
    let Some(session) = &app.session else {
        return vec![Line::from(Span::styled(
            "press any letter to start a lesson  ·  q quits",
            dim(),
        ))];
    };
    let cursor = session.cursor();
    let errored = app.last_miss.is_some();
    let mut spans = Vec::with_capacity(session.text().len());
    for (index, ch) in session.text().iter().enumerate() {
        let style = match index.cmp(&cursor) {
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
        spans.push(Span::styled(ch.to_string(), style));
    }
    vec![Line::from(spans)]
}

/// One line under the board: an abort, a generator note, or the key map.
fn notice_line(app: &App) -> Line<'static> {
    match &app.notice {
        Some(notice) => Line::from(Span::styled(
            notice.clone(),
            Style::new().fg(Color::Yellow),
        )),
        None => Line::from(vec![
            Span::styled("tab", bold_key()),
            Span::styled(" restart  ·  ", dim()),
            Span::styled("esc", bold_key()),
            Span::styled(" abandon  ·  ", dim()),
            Span::styled("q", bold_key()),
            Span::styled(" quit when idle", dim()),
        ]),
    }
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
    Style::new()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM)
}

fn bold_key() -> Style {
    Style::new()
        .add_modifier(Modifier::BOLD)
        .fg(Color::Cyan)
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
        terminal
            .draw(|frame| crate::ui::draw(frame, app))
            .unwrap();
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
    fn a_small_terminal_is_a_notice_not_a_crash() {
        let (_scratch, mut app) = app();
        app.start_lesson(0);
        let screen = draw(&app, 40, 12);
        assert!(screen.contains("terminal too small"), "{screen}");
        assert!(screen.contains("80"), "{screen}");
        assert!(!screen.contains("keytle —"), "nothing else is drawn: {screen}");
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
        assert!(screen.contains("100%"), "accuracy must read 100%:\n{screen}");
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
        assert!(!screen.contains("100%"), "one miss cannot be 100%:\n{screen}");
        assert!(screen.contains('%'), "{screen}");
        assert!(app.last_miss.is_some());
    }

    #[test]
    fn the_idle_screen_invites_a_lesson() {
        let (_scratch, app) = app();
        let screen = draw(&app, 80, 24);
        assert!(screen.contains("press any letter"), "{screen}");
    }
}
