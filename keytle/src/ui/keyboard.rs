//! The on-screen keyboard: the layout drawn from its positions.
//!
//! A layout is data — `(row, col)` positions with a `row_offset` stagger per row
//! — so the board is laid out arithmetically rather than painted by hand. Each
//! key is a cell [`KEY_WIDTH`] wide and either two or three rows tall: the third
//! row exists only when the layout has altgr legends, so the altgr character can
//! sit above its base (`é` over `e`) and the host legend, when emulating, below
//! it. A legend the host board cannot produce at its level is drawn as `·`,
//! which is the honest thing to show for a key that will never deliver it.

use std::collections::BTreeSet;

use keytle_core::emulate::Emulator;
use keytle_core::layout::{Finger, Layout, Position};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

/// One key cell's width in terminal cells.
pub const KEY_WIDTH: u16 = 4;
/// The number row and the three letter rows; the space row is not drawn.
const ROWS_SHOWN: u8 = 4;
/// What a legend the host cannot produce looks like.
const UNREACHABLE: &str = "·";

/// The style that marks the next expected key. A test can count the cells using
/// it, which is how "exactly one key is highlighted" is enforced.
pub fn next_key_style() -> Style {
    Style::new()
        .add_modifier(Modifier::REVERSED | Modifier::BOLD)
        .fg(Color::Cyan)
}

/// How tall one key cell is: three rows when the layout has altgr legends,
/// two when it has none.
pub fn cell_height(layout: &Layout) -> u16 {
    if layout.keys.values().any(|key| key.altgr.is_some()) {
        3
    } else {
        2
    }
}

/// The whole board's height for this layout.
pub fn board_height(layout: &Layout) -> u16 {
    u16::from(ROWS_SHOWN) * cell_height(layout)
}

/// Every key's cell rectangle, in row-major position order. Cells that do not
/// fit inside `area` are dropped rather than clipped, so a render can never
/// write outside the frame.
pub fn key_rects(layout: &Layout, area: Rect) -> Vec<(Position, Rect)> {
    let height = cell_height(layout);
    let mut rects = Vec::with_capacity(layout.keys.len());
    for pos in layout.keys.keys() {
        if pos.row >= ROWS_SHOWN {
            continue;
        }
        let stagger = u16::from(
            layout
                .row_offset
                .get(usize::from(pos.row))
                .copied()
                .unwrap_or(0),
        );
        let x = area.x + stagger * (KEY_WIDTH / 2) + u16::from(pos.col) * KEY_WIDTH;
        let y = area.y + u16::from(pos.row) * height;
        let rect = Rect::new(x, y, KEY_WIDTH, height);
        let fits = rect.x.saturating_add(rect.width) <= area.x.saturating_add(area.width)
            && rect.y.saturating_add(rect.height) <= area.y.saturating_add(area.height);
        if fits {
            rects.push((*pos, rect));
        }
    }
    rects
}

/// Draw the board. `next` is the character the typist must produce, `emulator`
/// is present only when emulation is on, and `reachable` is the level-aware set
/// of characters a keystroke can actually produce — the same alphabet lessons
/// are drawn from, so a key the host cannot deliver never looks typable.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    layout: &Layout,
    next: Option<char>,
    emulator: Option<&Emulator>,
    reachable: &BTreeSet<char>,
) {
    let next_pos = next.and_then(|want| {
        layout
            .keys
            .iter()
            .find(|(_, key)| key.base == want || key.shift == Some(want) || key.altgr == Some(want))
            .map(|(pos, _)| *pos)
    });
    let altgr_row = cell_height(layout) == 3;
    let buf = frame.buffer_mut();
    for (pos, rect) in key_rects(layout, area) {
        let Some(key) = layout.key_at(pos) else {
            continue;
        };
        let highlighted = next_pos == Some(pos);
        let base_style = if highlighted {
            next_key_style()
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        if highlighted {
            fill(buf, rect, next_key_style());
        }
        if !reachable.contains(&key.base) {
            put_center(buf, rect, base_row(rect, altgr_row), UNREACHABLE, dim());
            continue;
        }
        if altgr_row {
            if let Some(altgr) = key.altgr {
                // The altgr level is a level of its own: an `é` the host has no
                // altgr legend for is not typeable just because the base is, so
                // it is marked out of reach rather than offered silently.
                let legend = if reachable.contains(&altgr) {
                    altgr.to_string()
                } else {
                    UNREACHABLE.to_string()
                };
                put_center(buf, rect, rect.y, &legend, dim());
            }
        }
        put_center(
            buf,
            rect,
            base_row(rect, altgr_row),
            &key.base.to_string(),
            base_style,
        );
        let host_row = rect.y + cell_height(layout) - 1;
        if let Some(emulator) = emulator {
            if let Some(host) = emulator.host_char_for(key.base) {
                put_center(buf, rect, host_row, &host.to_string(), dim());
            }
        } else if highlighted {
            if let Some(finger) = key.finger {
                put_center(buf, rect, host_row, finger_short(finger), dim());
            }
        }
    }
}

fn base_row(rect: Rect, altgr_row: bool) -> u16 {
    rect.y + u16::from(altgr_row)
}

fn dim() -> Style {
    Style::new()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM)
}

/// Paint the whole cell so a highlighted key reads as one block.
fn fill(buf: &mut Buffer, rect: Rect, style: Style) {
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
    }
}

/// Write `text` centred on one row of a cell, in terminal columns rather than
/// chars, so a wide character does not push the cell out of shape.
fn put_center(buf: &mut Buffer, rect: Rect, y: u16, text: &str, style: Style) {
    let width: u16 = text
        .chars()
        .map(|c| u16::try_from(c.width().unwrap_or(0)).unwrap_or(1))
        .sum();
    let x = rect.x + rect.width.saturating_sub(width) / 2;
    buf.set_string(x, y, text, style);
}

/// Where a finger sits, in the words the status line uses.
pub fn finger_name(finger: Finger) -> &'static str {
    match finger {
        Finger::LeftPinky => "left-pinky",
        Finger::LeftRing => "left-ring",
        Finger::LeftMiddle => "left-middle",
        Finger::LeftIndex => "left-index",
        Finger::Thumb => "thumb",
        Finger::RightIndex => "right-index",
        Finger::RightMiddle => "right-middle",
        Finger::RightRing => "right-ring",
        Finger::RightPinky => "right-pinky",
    }
}

/// A two-letter stand-in that fits under a key.
fn finger_short(finger: Finger) -> &'static str {
    match finger {
        Finger::LeftPinky => "LP",
        Finger::LeftRing => "LR",
        Finger::LeftMiddle => "LM",
        Finger::LeftIndex => "LI",
        Finger::Thumb => "TH",
        Finger::RightIndex => "RI",
        Finger::RightMiddle => "RM",
        Finger::RightRing => "RR",
        Finger::RightPinky => "RP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keytle_core::layouts::{self, host_ids};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::Path;

    fn layout(id: &str) -> Layout {
        layouts::load(id, Path::new("/nonexistent")).unwrap()
    }

    /// Every legend of the layout, which is what a non-emulated board can type.
    fn all_chars(layout: &Layout) -> BTreeSet<char> {
        layout
            .keys
            .values()
            .flat_map(|key| [Some(key.base), key.shift, key.altgr])
            .flatten()
            .collect()
    }

    fn draw(id: &str, next: Option<char>, emulator: Option<&Emulator>, width: u16) -> Terminal<TestBackend> {
        let layout = layout(id);
        let area = Rect::new(0, 0, width, board_height(&layout) + 2);
        let reachable = match emulator {
            Some(em) => em.reachable_chars(),
            None => all_chars(&layout),
        };
        let mut terminal = Terminal::new(TestBackend::new(width, area.height)).unwrap();
        terminal
            .draw(|frame| render(frame, area, &layout, next, emulator, &reachable))
            .unwrap();
        terminal
    }

    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buf = terminal.backend().buffer();
        let width = buf.area().width as usize;
        buf.content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    /// Every cell of the rectangle, symbols joined.
    fn cell_text(terminal: &Terminal<TestBackend>, rect: Rect) -> String {
        let buf = terminal.backend().buffer();
        let width = buf.area().width as usize;
        let mut out = String::new();
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                out.push_str(buf.content()[y as usize * width + x as usize].symbol());
            }
        }
        out
    }

    fn cells_with_style(
        terminal: &Terminal<TestBackend>,
        style: Style,
        rects: &[(Position, Rect)],
    ) -> Vec<Position> {
        let buf = terminal.backend().buffer();
        let width = buf.area().width as usize;
        rects
            .iter()
            .filter(|(_, rect)| {
                (rect.y..rect.y + rect.height).any(|y| {
                    (rect.x..rect.x + rect.width).any(|x| {
                        buf.content()[y as usize * width + x as usize].modifier
                            == style.add_modifier
                    })
                })
            })
            .map(|(pos, _)| *pos)
            .collect()
    }

    #[test]
    fn colemak_dh_at_eighty_columns_shows_every_letter() {
        let terminal = draw("colemak-dh", None, None, 80);
        let text = rows(&terminal).join("\n");
        for ch in "nethao".chars() {
            assert!(text.contains(ch), "the board must show {ch}:\n{text}");
        }
        assert_eq!(
            text.lines().count(),
            usize::from(board_height(&layout("colemak-dh"))) + 2,
            "the board is drawn inside its area"
        );
    }

    #[test]
    fn exactly_one_key_cell_is_highlighted_for_the_next_character() {
        let layout = layout("colemak-dh");
        let area = Rect::new(0, 0, 80, board_height(&layout));
        let terminal = draw("colemak-dh", Some('e'), None, 80);
        let rects = key_rects(&layout, area);
        let style = next_key_style();
        let highlighted = cells_with_style(&terminal, style, &rects);
        assert_eq!(highlighted.len(), 1, "one key, not {highlighted:?}");
        let rect = rects.iter().find(|(pos, _)| *pos == highlighted[0]).unwrap().1;
        assert!(
            cell_text(&terminal, rect).contains('e'),
            "the highlighted cell is the one that types e"
        );
    }

    #[test]
    fn emulation_puts_the_host_letter_under_the_target_legend() {
        let layout = layout("colemak-dh");
        let host = layout_of("qwerty");
        let emulator = Emulator::new(host, layout.clone());
        let area = Rect::new(0, 0, 80, board_height(&layout));
        let terminal = draw("colemak-dh", None, Some(&emulator), 80);
        let rects = key_rects(&layout, area);
        let e_rect = rects
            .iter()
            .find(|(pos, _)| {
                let key = layout.key_at(*pos).unwrap();
                key.base == 'e'
            })
            .map(|(_, rect)| *rect)
            .expect("colemak-dh has an e key");
        let text = cell_text(&terminal, e_rect);
        assert!(text.contains('e'), "the base legend is drawn: {text:?}");
        assert!(text.contains('k'), "qwerty's k subscript sits below: {text:?}");
    }

    #[test]
    fn unreachable_positions_render_as_dots() {
        // A key whose character the host cannot produce has to be visibly out of
        // reach rather than silently pretending to be typeable.
        let layout = layout("colemak-dh");
        let area = Rect::new(0, 0, 80, board_height(&layout));
        let mut reachable = all_chars(&layout);
        reachable.remove(&'n');
        let mut terminal = Terminal::new(TestBackend::new(80, area.height)).unwrap();
        terminal
            .draw(|frame| render(frame, area, &layout, None, None, &reachable))
            .unwrap();
        let text = rows(&terminal).join("\n");
        assert!(text.contains('·'), "an unreachable key is a dot:\n{text}");
        let rect = key_rects(&layout, area)
            .into_iter()
            .find(|(pos, _)| *pos == Position::new(2, 6))
            .map(|(_, rect)| rect)
            .unwrap();
        assert_eq!(cell_text(&terminal, rect).trim(), "·");
    }

    #[test]
    fn an_altgr_legend_the_host_cannot_type_is_a_dot() {
        // qwerty has no altgr level, so colemak-dh's `é` can never be typed on
        // it: drawing it as an ordinary legend would offer a key that produces
        // nothing. The practice alphabet leaves it out, and so must the board.
        let layout = layout("colemak-dh");
        let emulator = Emulator::new(layout_of("qwerty"), layout.clone());
        let area = Rect::new(0, 0, 80, board_height(&layout));
        let terminal = draw("colemak-dh", None, Some(&emulator), 80);
        let e_rect = key_rects(&layout, area)
            .into_iter()
            .find(|(pos, _)| layout.key_at(*pos).is_some_and(|key| key.base == 'e'))
            .map(|(_, rect)| rect)
            .expect("colemak-dh has an e key");
        let altgr_cell = cell_text(&terminal, Rect::new(e_rect.x, e_rect.y, e_rect.width, 1));
        assert!(
            !altgr_cell.contains('é'),
            "the untypeable altgr legend is not offered: {altgr_cell:?}"
        );
        assert!(
            altgr_cell.trim() == "·",
            "it is marked out of reach instead: {altgr_cell:?}"
        );
        let text = cell_text(&terminal, e_rect);
        assert!(text.contains('e'), "the base letter stays typeable: {text:?}");
        assert!(text.contains('k'), "with qwerty's k below it: {text:?}");
    }

    #[test]
    fn every_host_layout_can_draw_its_own_board() {
        for id in host_ids() {
            let terminal = draw(id, Some('a'), None, 80);
            assert!(rows(&terminal).join("\n").contains('a'), "{id}");
        }
    }

    fn layout_of(id: &str) -> Layout {
        layouts::load(id, Path::new("/nonexistent")).unwrap()
    }
}
