//! Rendering: one draw call per frame, sized to the terminal it was given.
//!
//! The frame is checked against [`MIN_WIDTH`]×[`MIN_HEIGHT`] before any screen
//! draws, and a terminal below it gets a single notice instead of a squeezed
//! layout. That is the whole resize strategy: nothing is cached, every region is
//! derived from the area each frame, and no widget indexes the buffer by hand —
//! so shrinking the window mid-lesson cannot panic.

use ratatui::layout::{Alignment, Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Screen};

pub mod keyboard;
pub mod typing;

/// The narrowest terminal the typing view is drawn in.
pub const MIN_WIDTH: u16 = 80;
/// The shortest terminal the typing view is drawn in.
pub const MIN_HEIGHT: u16 = 24;
/// What a terminal below the minimum sees, and nothing else.
pub const TOO_SMALL: &str = "terminal too small (80×24 minimum)";

/// Draw whichever screen is in front.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        too_small(frame, area);
        return;
    }
    match app.screen {
        Screen::Typing => typing::render(frame, app),
        Screen::Results => results(frame, area, app),
        Screen::Stats | Screen::Settings | Screen::Help => placeholder(frame, area, app),
    }
}

/// A panel that says the terminal cannot show the view yet.
fn too_small(frame: &mut Frame, area: Rect) {
    let panel = UiLayout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area)[1];
    let width = panel.width.min(48);
    let panel = Rect::new(
        panel.x + (panel.width - width) / 2,
        panel.y,
        width,
        panel.height.min(3),
    );
    frame.render_widget(
        Paragraph::new(Line::from(TOO_SMALL))
            .block(Block::bordered().border_style(Style::new().fg(Color::Red)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        panel,
    );
}

/// Screens that later tasks fill in: say so rather than draw something wrong.
fn placeholder(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::Stats => "stats",
        Screen::Settings => "settings",
        Screen::Help => "help",
        _ => "keytle",
    };
    let mut lines = vec![Line::from(Span::styled(
        "this screen arrives with the next task",
        Style::new().fg(Color::DarkGray),
    ))];
    if let Some(notice) = &app.notice {
        lines.push(Line::from(Span::styled(
            notice.clone(),
            Style::new().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(Span::styled(
        "esc returns to typing",
        Style::new().fg(Color::DarkGray),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// The lesson's totals, once the results screen exists for real.
fn results(frame: &mut Frame, area: Rect, app: &App) {
    let stats = app.last_stats.clone().unwrap_or_default();
    let line = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(
                format!("{label:<12}"),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled(
                value,
                Style::new()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Cyan),
            ),
        ])
    };
    let lines = vec![
        line("net wpm", format!("{:.0}", stats.net_wpm)),
        line("raw wpm", format!("{:.0}", stats.raw_wpm)),
        line("accuracy", format!("{:.0}%", stats.accuracy * 100.0)),
        line("seconds", format!("{:.0}", stats.duration_ms as f64 / 1000.0)),
        Line::from(""),
        Line::from(Span::styled(
            "enter or tab starts another lesson  ·  q returns",
            Style::new().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("results")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use keytle_core::config::Config;
    use keytle_core::paths::Paths;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app() -> (std::path::PathBuf, App) {
        let base = std::env::temp_dir().join(format!(
            "keytle-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
        };
        let app = App::new(Config::default(), &paths, 0).unwrap();
        (base, app)
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

    #[test]
    fn tiny_terminals_get_the_notice_and_nothing_else() {
        for (width, height) in [(40, 12), (79, 24), (80, 23), (20, 5)] {
            let (base, app) = app();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let text = screen(&terminal);
            assert!(
                text.contains("terminal too small"),
                "{width}x{height} must explain itself:\n{text}"
            );
            assert!(
                !text.contains("keytle —"),
                "{width}x{height} must not draw the view"
            );
            let _ = std::fs::remove_dir_all(base);
        }
    }

    #[test]
    fn the_minimum_size_draws_the_view() {
        let (base, app) = app();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(!screen(&terminal).contains("terminal too small"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn resizing_down_and_back_up_does_not_panic() {
        let (base, mut app) = app();
        app.start_lesson(0);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        for (width, height) in [(120, 40), (80, 24), (40, 12), (10, 3), (80, 24), (200, 60)] {
            terminal.backend_mut().resize(width, height);
            terminal.draw(|frame| draw(frame, &app)).unwrap();
        }
        let _ = std::fs::remove_dir_all(base);
    }
}
