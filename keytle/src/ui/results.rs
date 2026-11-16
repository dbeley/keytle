//! What a finished lesson earned: the three numbers it is judged by, the keys
//! that need work, and the unlock rule's verdict in plain words.
//!
//! The screen reads only what `App` already decided when the lesson finished
//! ([`App::last_stats`], [`App::last_weak`], [`App::last_unlock`]); nothing here
//! aggregates, so the results cannot disagree with the profile or the stats file
//! that the same `finish_lesson` call wrote.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::ui::{fmt_duration, metric_line, unlock_line};

/// Draw the results of the lesson that just finished.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let stats = app.last_stats.clone().unwrap_or_default();
    let mut lines = vec![
        metric_line("net wpm", format!("{:.0}", stats.net_wpm)),
        metric_line("raw wpm", format!("{:.0}", stats.raw_wpm)),
        metric_line("accuracy", format!("{:.1}%", stats.accuracy * 100.0)),
        metric_line("duration", fmt_duration(stats.duration_ms)),
        Line::from(""),
    ];

    // Weakest first, as the engine ranked them; the columns are wide so the
    // letters line up whatever their width.
    let weakest = if app.last_weak.is_empty() {
        "—".to_string()
    } else {
        app.last_weak
            .iter()
            .take(3)
            .map(|row| format!("{} {:.0}%", row.ch, row.accuracy * 100.0))
            .collect::<Vec<_>>()
            .join("  ·  ")
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{:<12}", "weakest"), Style::new().fg(Color::DarkGray)),
        Span::styled(
            weakest,
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Yellow),
        ),
    ]));

    // The verdict line: what the next letter needs, or that it has arrived.
    if let Some(outcome) = &app.last_unlock {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            unlock_line(outcome),
            Style::new().fg(Color::Green),
        )));
    } else if let Some(notice) = &app.notice {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            notice.clone(),
            Style::new().fg(Color::Yellow),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "enter or tab starts another lesson  ·  s shows your progress  ·  esc back",
        Style::new().fg(Color::DarkGray),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("results"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Screen;
    use crate::ui::draw;
    use keytle_core::config::Config;
    use keytle_core::paths::Paths;
    use keytle_core::typing::TypingSession;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn scratch(tag: &str) -> (std::path::PathBuf, Paths) {
        let base = std::env::temp_dir().join(format!(
            "keytle-results-{}-{tag}",
            std::process::id()
        ));
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

    /// Finish a one-line lesson where `b` is the only key that was missed.
    fn finish_a_lesson_with_one_bad_key(app: &mut App) {
        app.config.emulate = false;
        app.start_lesson(0);
        // A three-character lesson, typed with `b` wrong: `c` and `a` are clean,
        // so `b` is the weakest key by both accuracy and confidence.
        app.session = Some(TypingSession::new("abc", app.config.strict, 0));
        for (i, typed) in ['a', 'x', 'b', 'c'].into_iter().enumerate() {
            app.handle_key(
                ratatui_key(typed),
                100 + 100 * u64::try_from(i).unwrap(),
            );
        }
        app.finish_lesson(1_000);
    }

    fn ratatui_key(ch: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(ch),
            crossterm::event::KeyModifiers::NONE,
        )
    }

    #[test]
    fn results_shows_the_three_numbers_and_the_weakest_keys() {
        let (base, paths) = scratch("weak");
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        finish_a_lesson_with_one_bad_key(&mut app);
        assert_eq!(app.screen, Screen::Results);
        assert_eq!(app.last_weak.len(), 3, "all three keys were ranked");
        assert!(
            app.last_weak.iter().any(|row| row.ch == 'b' && row.accuracy < 0.6),
            "the missed key is in the list and reads as half-right: {:?}",
            app.last_weak
        );

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains("net wpm"), "{text}");
        assert!(text.contains("raw wpm"), "{text}");
        assert!(text.contains("accuracy"), "{text}");
        let weakest = text
            .lines()
            .find(|line| line.contains("weakest"))
            .unwrap_or_else(|| panic!("no weakest line:\n{text}"));
        assert!(weakest.contains('b'), "the bad key is named:\n{weakest}");
        assert!(weakest.contains("50%"), "and so is its accuracy:\n{weakest}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_unlock_verdict_says_how_much_confidence_is_missing() {
        let (base, paths) = scratch("verdict");
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        finish_a_lesson_with_one_bad_key(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = screen(&terminal);
        // A fresh profile is blocked for confidence, so the line names the next
        // letter and both numbers: where it is and where it has to get to.
        let line = text
            .lines()
            .find(|line| line.contains("confidence"))
            .unwrap_or_else(|| panic!("no verdict line:\n{text}"));
        assert!(line.contains("0.70"), "the gate is named:\n{line}");
        assert!(
            line.contains("0."),
            "the current confidence is named:\n{line}"
        );
        let _ = std::fs::remove_dir_all(base);
    }
}
