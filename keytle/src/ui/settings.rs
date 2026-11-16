//! The settings screen: the eight config fields, edited in memory and written
//! with `w`.
//!
//! The screen draws [`SETTINGS_ROWS`] and whatever [`App::settings_values`]
//! reports, so it can never show a value the app is not actually using: every
//! change goes through the app's own cycling, which applies it to the live
//! config (and to the boards, when the change is about them).

use ratatui::layout::{Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, SETTINGS_ROWS};

/// Draw the rows, the hints and the last notice.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let values = app.settings_values();
    let selected = app.settings_row();
    let lines: Vec<Line> = SETTINGS_ROWS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(row, (label, value))| row_line(label, &value, row == selected))
        .collect();

    let chunks = UiLayout::vertical([
        Constraint::Min(SETTINGS_ROWS.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("settings · in memory until w"))
            .wrap(Wrap { trim: true }),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "j/k move  ·  h/l or ←/→ change  ·  space/enter next  ·  w save  ·  esc discard",
            Style::new().fg(Color::DarkGray),
        ))),
        chunks[1],
    );
    let (text, style) = match &app.notice {
        Some(notice) => (notice.clone(), Style::new().fg(Color::Yellow)),
        None => ("edits are in memory until w writes them".to_string(), Style::new().fg(Color::DarkGray)),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).wrap(Wrap { trim: true }),
        chunks[2],
    );
    // The path is always in the footer: `w` writes here, so the screen names it.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("config: {}", app.config_file().display()),
            Style::new().fg(Color::DarkGray),
        )))
        .wrap(Wrap { trim: true }),
        chunks[3],
    );
}

/// One `label   value` row, the selected one marked and highlighted.
fn row_line(label: &str, value: &str, selected: bool) -> Line<'static> {
    let marker = if selected { "▸ " } else { "  " };
    let label_style = if selected {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Gray)
    };
    let value_style = if selected {
        Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Line::from(vec![
        Span::styled(format!("{marker}{label:<14}"), label_style),
        Span::styled(value.to_string(), value_style),
    ])
}
