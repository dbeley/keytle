//! The help screen: every binding keytle answers to, in one list.
//!
//! [`BINDINGS`] is the single source: the screen draws it and the tests read it,
//! so a key that only exists in the handler is a visible omission rather than a
//! silent one.

use ratatui::layout::{Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

/// `(key, what it does)`, in the order the screen lists them.
pub const BINDINGS: &[(&str, &str)] = &[
    ("esc", "abort"),
    ("tab", "restart"),
    ("u", "unlock next"),
    ("l", "lock current"),
    ("s", "stats"),
    ("?", "help"),
    ("q", "quit"),
    ("m", "merge modes"),
    ("c", "chart mode"),
    ("w", "save settings"),
];

/// Draw the bindings, plus where the settings screen is.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(key, does)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<6}"),
                    Style::new()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::Cyan),
                ),
                Span::styled((*does).to_string(), Style::new().fg(Color::Gray)),
            ])
        })
        .collect();
    let chunks = UiLayout::vertical([
        Constraint::Min(BINDINGS.len() as u16),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("help · keys"))
            .wrap(Wrap { trim: true }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "`, opens the settings screen from the typing screen",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "u and l act on the typing screen while no lesson is running",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "m, c and l also drive the stats dashboard's filters",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .block(Block::bordered().title("notes"))
        .wrap(Wrap { trim: true }),
        chunks[1],
    );
    let footer = match &app.notice {
        Some(notice) => notice.clone(),
        None => "esc returns to typing".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(footer, Style::new().fg(Color::Yellow))))
            .wrap(Wrap { trim: true }),
        chunks[2],
    );
}
