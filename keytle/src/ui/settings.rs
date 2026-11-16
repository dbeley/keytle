//! The settings panel: the config fields, edited in memory and written with `w`.
//!
//! The panel is an overlay, not a screen of its own: it is drawn over the typing
//! view, which stays visible around it, because a setting is changed to change
//! the drill behind it — a layout or a mode is best judged with its keyboard in
//! sight. It draws [`SETTINGS_ROWS`] and whatever [`App::settings_values`]
//! reports, so it can never show a value the app is not actually using: every
//! change goes through the app's own cycling, which applies it to the live
//! config (and to the boards, when the change is about them).

use ratatui::layout::{Alignment, Constraint, Layout as UiLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, SETTINGS_ROWS};

/// How wide the panel may grow before it stops and centres.
const PANEL_WIDTH: u16 = 64;

/// Draw the rows, the hints and the last notice, over whatever is behind.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let values = app.settings_values();
    let selected = app.settings_row();
    let lines: Vec<Line> = SETTINGS_ROWS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(row, (label, value))| row_line(label, &value, row == selected))
        .collect();

    // Border, one row per setting, and three rows of footer.
    let height = u16::try_from(SETTINGS_ROWS.len()).unwrap_or(0) + 5;
    let width = area.width.saturating_sub(4).min(PANEL_WIDTH);
    let panel = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height.min(area.height),
    );
    // Clear first: the panel is opaque, and the view behind it stays readable
    // around the edges without bleeding through the rows.
    frame.render_widget(Clear, panel);

    // The footer rows live inside the border, under the settings rows.
    let inner = Rect::new(
        panel.x + 1,
        panel.y + 1,
        panel.width.saturating_sub(2),
        panel.height.saturating_sub(2),
    );
    let footer = UiLayout::vertical([
        Constraint::Length(u16::try_from(SETTINGS_ROWS.len()).unwrap_or(0)),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title("settings · esc writes and closes")
                    .border_style(Style::new().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: true }),
        panel,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "j/k move · h/l change · w save now · esc save and close",
            Style::new().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center),
        footer[1],
    );
    let (text, style) = match &app.notice {
        Some(notice) => (notice.clone(), Style::new().fg(Color::Yellow)),
        None => (
            "every change applies at once; leaving writes the file".to_string(),
            Style::new().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style)))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        footer[2],
    );
    // The path is always in the footer: `w` writes here, so the panel names it.
    // A deep temp directory is elided from the left, never wrapped: the file
    // name is the part worth reading, and it has to stay on the one row.
    let room = usize::from(inner.width).saturating_sub("config: ".len());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("config: {}", fit_path(&app.config_file(), room)),
            Style::new().fg(Color::DarkGray),
        )))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        footer[3],
    );
}

/// A path shortened from the left until it fits `width` characters, keeping the
/// file name and as many parent directories as there is room for.
fn fit_path(path: &std::path::Path, width: usize) -> String {
    let full = path.display().to_string();
    if full.chars().count() <= width {
        return full;
    }
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = full.chars().collect();
    // One character of the budget goes to the ellipsis.
    let tail: String = chars[chars.len() - (width - 1)..].iter().collect();
    // Start on a component boundary where one is available.
    let tail = match tail.find('/') {
        Some(1) | None => tail,
        Some(at) => tail[at + 1..].to_string(),
    };
    format!("…/{tail}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fitting_path_is_shown_whole() {
        let path = std::path::Path::new("/home/me/.config/keytle/config.toml");
        assert_eq!(fit_path(path, 60), "/home/me/.config/keytle/config.toml");
    }

    #[test]
    fn a_long_path_keeps_its_file_name_and_the_nearer_directories() {
        let path = std::path::Path::new(
            "/tmp/nix-shell.sSwZCR/keytle-ui-9766-1794822922497544159/config/config.toml",
        );
        let shown = fit_path(path, 40);
        assert!(shown.chars().count() <= 40, "{shown}");
        assert!(shown.starts_with('…'), "{shown}");
        assert!(shown.ends_with("config/config.toml"), "{shown}");
    }
}
