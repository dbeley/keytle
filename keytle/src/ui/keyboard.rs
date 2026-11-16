//! The on-screen keyboard: the layout drawn from its positions.
//!
//! A layout is data — `(row, col)` positions with a `row_offset` stagger per row
//! — so the board is laid out arithmetically rather than painted by hand. Each
//! key is a cell [`KEY_WIDTH`] wide and either two or three rows tall: the third
//! row exists only when the layout has altgr legends, so the altgr character can
//! sit above its base (`é` over `e`) and the host legend, when emulating, below
//! it. A legend the host board cannot produce at its level is drawn as `·`,
//! which is the honest thing to show for a key that will never deliver it.
//!
//! The board is centred in its area rather than drawn from the left edge: it is
//! a picture of a keyboard under the lesson, not a table, and centring keeps the
//! board under the text on a wide terminal.

use std::collections::BTreeSet;

use keytle_core::emulate::Emulator;
use keytle_core::layout::{Layout, Position};
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

/// Which extra legends a key cell has to hold, and therefore how tall it is.
///
/// The base legend always takes a row. An altgr legend takes the row above it,
/// and the host legend a row below: a board showing neither needs one row per
/// key, not three rows of mostly blank cells. The altgr row is only drawn when
/// the level can produce something at all — a whole row of `·` says no more
/// than the one dot it replaces, and it is a quarter of the board's height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Legends {
    pub altgr: bool,
    pub host: bool,
}

impl Legends {
    /// The legends this board can usefully show, given the reachable set.
    ///
    /// The altgr row is only worth its height when the level is actually there:
    /// an emulated Colemak-DH board on a QWERTY host can type three of its
    /// forty-six altgr legends, and a row holding three glyphs and forty-three
    /// blanks is neither readable nor honest about what the board offers. When
    /// at least half the level is reachable, the level is real and is drawn —
    /// with the individual legends that are not reachable left out.
    pub fn of(layout: &Layout, emulated: bool, reachable: &BTreeSet<char>) -> Legends {
        let with_altgr = layout
            .keys
            .values()
            .filter(|key| key.altgr.is_some())
            .count();
        let live = layout
            .keys
            .values()
            .filter(|key| key.altgr.is_some_and(|ch| reachable.contains(&ch)))
            .count();
        Legends {
            altgr: with_altgr > 0 && live * 2 >= with_altgr,
            host: emulated,
        }
    }
}

/// How tall one key cell is, given what has to be drawn in it.
pub fn cell_height(legends: Legends) -> u16 {
    1 + u16::from(legends.altgr) + u16::from(legends.host)
}

/// The whole board's height for this layout.
pub fn board_height(legends: Legends) -> u16 {
    u16::from(ROWS_SHOWN) * cell_height(legends)
}

/// How wide the board is drawn, stagger included, so it can be centred.
pub fn board_width(layout: &Layout) -> u16 {
    let mut widest = 0;
    for pos in layout.keys.keys().filter(|pos| pos.row < ROWS_SHOWN) {
        let stagger = u16::from(
            layout
                .row_offset
                .get(usize::from(pos.row))
                .copied()
                .unwrap_or(0),
        );
        let right = stagger * (KEY_WIDTH / 2) + (u16::from(pos.col) + 1) * KEY_WIDTH;
        widest = widest.max(right);
    }
    widest
}

/// Every key's cell rectangle, in row-major position order. Cells that do not
/// fit inside `area` are dropped rather than clipped, so a render can never
/// write outside the frame.
pub fn key_rects(layout: &Layout, area: Rect, legends: Legends) -> Vec<(Position, Rect)> {
    let height = cell_height(legends);
    // The board is centred in the area, never wider than it: a narrow terminal
    // loses the rightmost keys to the `fits` check instead of clipping them.
    let left = area.x + area.width.saturating_sub(board_width(layout)) / 2;
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
        let x = left + stagger * (KEY_WIDTH / 2) + u16::from(pos.col) * KEY_WIDTH;
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
    let emulated = emulator.is_some();
    let legends = Legends::of(layout, emulated, reachable);
    let altgr_row = legends.altgr;
    let buf = frame.buffer_mut();
    for (pos, rect) in key_rects(layout, area, legends) {
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
        // The altgr level is a level of its own: an `é` the host has no altgr
        // legend for is not typeable just because the base is, so it is not
        // offered. A dot on every key that lacks one would bury the few legends
        // the level *does* deliver, and the unreachable list is what
        // `keytle doctor` is for.
        if altgr_row {
            if let Some(altgr) = key.altgr.filter(|ch| reachable.contains(ch)) {
                put_center(buf, rect, rect.y, &altgr.to_string(), dim());
            }
        }
        // A base the host cannot produce is a dot: that is the one legend whose
        // absence would silently mislead, because the key looks typable.
        if reachable.contains(&key.base) {
            put_center(
                buf,
                rect,
                base_row(rect, altgr_row),
                &key.base.to_string(),
                base_style,
            );
            if let Some(emulator) = emulator {
                let host_row = rect.y + cell_height(legends) - 1;
                if let Some(host) = emulator.host_char_for(key.base) {
                    put_center(buf, rect, host_row, &host.to_string(), dim());
                }
            }
        } else {
            put_center(buf, rect, base_row(rect, altgr_row), UNREACHABLE, dim());
        }
    }
}

fn base_row(rect: Rect, altgr_row: bool) -> u16 {
    rect.y + u16::from(altgr_row)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM)
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

    fn draw(
        id: &str,
        next: Option<char>,
        emulator: Option<&Emulator>,
        width: u16,
    ) -> Terminal<TestBackend> {
        let layout = layout(id);
        let reachable = match emulator {
            Some(em) => em.reachable_chars(),
            None => all_chars(&layout),
        };
        draw_with(&layout, next, emulator, width, &reachable)
    }

    /// Draw a board with an explicit reachable set, the way a caller that knows
    /// the alphabet would, and hand back the legends it was drawn with so the
    /// test can measure the same cells.
    fn draw_with(
        layout: &Layout,
        next: Option<char>,
        emulator: Option<&Emulator>,
        width: u16,
        reachable: &BTreeSet<char>,
    ) -> Terminal<TestBackend> {
        let legends = Legends::of(layout, emulator.is_some(), reachable);
        let area = Rect::new(0, 0, width, board_height(legends) + 2);
        let mut terminal = Terminal::new(TestBackend::new(width, area.height)).unwrap();
        terminal
            .draw(|frame| render(frame, area, layout, next, emulator, reachable))
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
            usize::from(board_height(Legends {
                altgr: true,
                host: false
            })) + 2,
            "the board is drawn inside its area"
        );
    }

    #[test]
    fn exactly_one_key_cell_is_highlighted_for_the_next_character() {
        let layout = layout("colemak-dh");
        let legends = Legends {
            altgr: true,
            host: false,
        };
        let area = Rect::new(0, 0, 80, board_height(legends));
        let terminal = draw("colemak-dh", Some('e'), None, 80);
        let rects = key_rects(&layout, area, legends);
        let style = next_key_style();
        let highlighted = cells_with_style(&terminal, style, &rects);
        assert_eq!(highlighted.len(), 1, "one key, not {highlighted:?}");
        let rect = rects
            .iter()
            .find(|(pos, _)| *pos == highlighted[0])
            .unwrap()
            .1;
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
        let reachable = emulator.reachable_chars();
        let legends = Legends::of(&layout, true, &reachable);
        let area = Rect::new(0, 0, 80, board_height(legends));
        let terminal = draw("colemak-dh", None, Some(&emulator), 80);
        let rects = key_rects(&layout, area, legends);
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
        assert!(
            text.contains('k'),
            "qwerty's k subscript sits below: {text:?}"
        );
    }

    #[test]
    fn unreachable_positions_render_as_dots() {
        // A key whose character the host cannot produce has to be visibly out of
        // reach rather than silently pretending to be typeable.
        let layout = layout("colemak-dh");
        let mut reachable = all_chars(&layout);
        reachable.remove(&'n');
        let legends = Legends::of(&layout, false, &reachable);
        let area = Rect::new(0, 0, 80, board_height(legends));
        let terminal = draw_with(&layout, None, None, 80, &reachable);
        let text = rows(&terminal).join("\n");
        assert!(text.contains('·'), "an unreachable key is a dot:\n{text}");
        let rect = key_rects(&layout, area, legends)
            .into_iter()
            .find(|(pos, _)| *pos == Position::new(2, 6))
            .map(|(_, rect)| rect)
            .unwrap();
        // The base legend is the dot; the altgr legend above it is reachable on
        // its own level and stays, because one out-of-reach level says nothing
        // about the other.
        let base_row = Rect::new(rect.x, rect.y + 1, rect.width, 1);
        assert_eq!(cell_text(&terminal, base_row).trim(), "·");
        let cell = cell_text(&terminal, rect);
        assert!(
            cell.contains('ñ') && cell.trim_end().ends_with('·'),
            "the altgr legend and the out-of-reach base share the cell: {cell:?}"
        );
    }

    #[test]
    fn an_altgr_legend_that_cannot_be_typed_is_not_offered() {
        // One altgr legend out of reach, the rest of the level alive: the row is
        // drawn for the legends that work, and the key that cannot deliver its
        // own simply does not offer one. (A level with nothing reachable draws
        // no row at all — see the test below.)
        let layout = layout("colemak-dh");
        let mut reachable = all_chars(&layout);
        reachable.remove(&'é');
        let legends = Legends::of(&layout, false, &reachable);
        assert!(legends.altgr, "the level still has legends to show");
        let area = Rect::new(0, 0, 80, board_height(legends));
        let terminal = draw_with(&layout, None, None, 80, &reachable);
        let e_rect = key_rects(&layout, area, legends)
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
            altgr_cell.trim().is_empty(),
            "nothing is drawn in its place: {altgr_cell:?}"
        );
        let text = cell_text(&terminal, e_rect);
        assert!(
            text.contains('e'),
            "the base letter stays typeable: {text:?}"
        );
    }

    #[test]
    fn a_level_reachable_on_a_handful_of_keys_costs_no_row() {
        // What an emulated Colemak-DH board on QWERTY actually looks like: three
        // altgr legends out of forty-six (`*`, backtick and `^`). The level is
        // not usable, so the board does not spend a row on it.
        let layout = layout("colemak-dh");
        let emulator = Emulator::new(layout_of("qwerty"), layout.clone());
        let reachable = emulator.reachable_chars();
        let legends = Legends::of(&layout, true, &reachable);
        assert!(!legends.altgr, "three of forty-six is not a level");
        assert_eq!(cell_height(legends), 2, "base and host legend only");
        let terminal = draw("colemak-dh", None, Some(&emulator), 80);
        let text = rows(&terminal).join("\n");
        assert!(!text.contains('é'), "no altgr legend is drawn:\n{text}");
        assert!(text.contains('k'), "the host legend under `e` is:\n{text}");
    }

    #[test]
    fn a_level_with_nothing_reachable_costs_no_row() {
        // Every altgr legend out of reach: an empty level earns no row at all,
        // and the board is one row shorter rather than a row of blanks.
        let layout = layout("colemak-dh");
        let mut reachable = all_chars(&layout);
        for altgr in layout.keys.values().filter_map(|key| key.altgr) {
            reachable.remove(&altgr);
        }
        let legends = Legends::of(&layout, true, &reachable);
        assert!(!legends.altgr, "no altgr row");
        assert!(legends.host, "the host legend row stays");
        assert_eq!(
            cell_height(legends),
            2,
            "an emulated board with no altgr level is two rows per key"
        );
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
