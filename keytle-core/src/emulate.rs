//! Positional emulation: learning one layout on somebody else's keyboard.
//!
//! A terminal reports the character the *host* layout produces, not the physical
//! key, so `keytle` can still teach a different board: the received character is
//! looked up on the host layout to recover the position that was pressed, and the
//! target layout is read at that same position. Typing `e` on a qwerty keyboard
//! while emulating Colemak-DH therefore counts as `f`, because both keys sit at
//! `(1, 2)`.
//!
//! Because the mapping is positional rather than alphabetical, characters the
//! host cannot produce on a position the target uses are simply out of reach:
//! [`Emulator::unreachable_chars`] names them so lessons and the keyboard legend
//! can leave those keys out instead of asking for impossible keystrokes.
//! Reachability is a property of the *pair* (host, target) and is computed once
//! in [`Emulator::new`]. It is positional — a target key counts if the host has
//! *any* key at that spot — so an altgr legend such as Colemak-DH's `é` counts
//! as reachable even though a bare qwerty host has no keystroke that produces
//! it. Callers that need a character to actually arrive should pair
//! [`Emulator::is_char_reachable`] with [`Emulator::translate`].

use crate::layout::{Key, Layout, Position};
use std::collections::BTreeSet;

/// Which legend of a key a character came from. Emulation maps a keystroke to
/// the target's *same-level* legend, so typing `E` on a qwerty board yields
/// `F`, not `f`: the case the user typed survives the move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Base,
    Shift,
    Altgr,
}

impl Level {
    /// The character `key` shows at this level, if it has one.
    fn legend(self, key: &Key) -> Option<char> {
        match self {
            Level::Base => Some(key.base),
            Level::Shift => key.shift,
            Level::Altgr => key.altgr,
        }
    }
}

/// The level at which `key` produces `c` exactly, trying base, then shift,
/// then altgr. Case matters: `'E'` and `'e'` are different levels.
fn level_of(key: &Key, c: char) -> Option<Level> {
    [Level::Base, Level::Shift, Level::Altgr]
        .into_iter()
        .find(|level| level.legend(key) == Some(c))
}

/// One host board emulating one target board.
pub struct Emulator {
    host: Layout,
    target: Layout,
    reachable: BTreeSet<Position>,
}

impl Emulator {
    /// Pair two layouts and compute which target positions the host can press.
    pub fn new(host: Layout, target: Layout) -> Emulator {
        let reachable = target
            .keys
            .keys()
            .filter(|pos| host.key_at(**pos).is_some())
            .copied()
            .collect();
        Emulator {
            host,
            target,
            reachable,
        }
    }

    /// The target positions the host board also has a key for.
    pub fn reachable(&self) -> &BTreeSet<Position> {
        &self.reachable
    }

    /// The layout the user is typing on.
    pub fn host(&self) -> &Layout {
        &self.host
    }

    /// The layout being taught.
    pub fn target(&self) -> &Layout {
        &self.target
    }

    /// Does `expected` sit on a position the host board can press?
    pub fn is_char_reachable(&self, expected: char) -> bool {
        self.target
            .position_of_base(expected)
            .is_some_and(|pos| self.reachable.contains(&pos))
    }

    /// The target character produced by pressing whatever key types `typed` on
    /// the host board. The host legend that matches `typed` exactly (base,
    /// shift or altgr) picks the level, and the target is read at that same
    /// level, so case survives: `E` on a qwerty host emulating Colemak-DH is
    /// `F`. `None` when the target has no legend at that level.
    pub fn translate(&self, typed: char) -> Option<char> {
        let host_key = self.host.key_at(self.host.position_of_base(typed)?)?;
        let level = level_of(host_key, typed)?;
        self.target
            .key_at(host_key.pos)
            .and_then(|key| level.legend(key))
    }

    /// The host character that has to be pressed to produce `expected`; this is
    /// the subscript the emulated keyboard legend shows. It is read at the
    /// level `expected` occupies on the target, so the subscript for `F` is `E`
    /// and for `f` is `e`.
    pub fn host_char_for(&self, expected: char) -> Option<char> {
        let target_key = self
            .target
            .key_at(self.target.position_of_base(expected)?)?;
        let level = level_of(target_key, expected)?;
        self.host
            .key_at(target_key.pos)
            .and_then(|key| level.legend(key))
    }

    /// Every character of the target layout that no keystroke on the host can
    /// produce, in row-major position order.
    pub fn unreachable_chars(&self) -> Vec<char> {
        let mut chars: Vec<char> = Vec::new();
        for (pos, key) in &self.target.keys {
            if self.reachable.contains(pos) {
                continue;
            }
            for c in [Some(key.base), key.shift, key.altgr].into_iter().flatten() {
                if !chars.contains(&c) {
                    chars.push(c);
                }
            }
        }
        chars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts;

    fn layout(id: &str) -> Layout {
        let src = layouts::bundled_source(id).unwrap_or_else(|| panic!("{id} is bundled"));
        Layout::from_toml(src).unwrap_or_else(|err| panic!("{id} parses: {err}"))
    }

    fn emulator(host: &str, target: &str) -> Emulator {
        Emulator::new(layout(host), layout(target))
    }

    /// Every expectation below was read off `data/layouts/*.toml` with a script
    /// that pairs the two key tables by `(row, col)`; the position is in the
    /// comment so a future edit to the data fails the test with the reason.
    #[test]
    fn qwerty_host_to_colemak_dh_target_translates_by_position() {
        let e = emulator("qwerty", "colemak-dh");

        assert_eq!(e.translate('w'), Some('w')); // (1, 1) both
        assert_eq!(e.translate('e'), Some('f')); // (1, 2)
        assert_eq!(e.translate('u'), Some('l')); // (1, 6)
        assert_eq!(e.translate('j'), Some('n')); // (2, 6)
        assert_eq!(e.translate('k'), Some('e')); // (2, 7)
        assert_eq!(e.translate('n'), Some('k')); // (3, 5)
        assert_eq!(e.translate(' '), Some(' ')); // (4, 0)

        assert_eq!(e.translate('é'), None, "qwerty cannot type é at all");
        assert_eq!(e.translate('€'), None, "nor €");
    }

    #[test]
    fn qwerty_host_to_gallium_target_translates_by_position() {
        let e = emulator("qwerty", "gallium");

        // gallium row 1 is "bldcvjfou..." and row 2 is "nrtsgyhaei/", so the
        // qwerty keys at (1, 0) and (2, 2) produce 'b' and 't'.
        assert_eq!(e.translate('q'), Some('b')); // (1, 0)
        assert_eq!(e.translate('d'), Some('t')); // (2, 2)
        assert_eq!(e.translate('f'), Some('s')); // (2, 3)
        assert_eq!(e.translate('n'), Some('k')); // (3, 5)
    }

    #[test]
    fn qwerty_host_to_bepo_target_reaches_accents_by_position() {
        let e = emulator("qwerty", "bepo");

        // bépo row 1 is "bépoè^vdljzwç", so é lives at (1, 1) — the key a
        // qwerty board labels 'w' — and 'p' sits at (1, 2), where qwerty has 'e'.
        assert_eq!(e.translate('w'), Some('é')); // (1, 1)
        assert_eq!(e.translate('e'), Some('p')); // (1, 2)
        assert_eq!(e.translate('a'), Some('a')); // (2, 0)

        assert!(e.is_char_reachable('é'), "é is reachable by position");
        assert_eq!(e.host_char_for('é'), Some('w'), "the legend subscript");
        assert_eq!(e.host_char_for('p'), Some('e'));

        assert_eq!(
            e.unreachable_chars(),
            Vec::<char>::new(),
            "same board shape"
        );
    }

    #[test]
    fn azerty_host_to_colemak_dh_target_uses_azerty_positions() {
        let e = emulator("azerty", "colemak-dh");

        // azerty row 1 starts with 'a' and its row 2 starts with 'q', so the
        // two layouts disagree about where those letters live.
        assert_eq!(e.translate('a'), Some('q')); // (1, 0)
        assert_eq!(e.translate('z'), Some('w')); // (1, 1)
        assert_eq!(e.translate('q'), Some('a')); // (2, 0)
        assert_eq!(e.translate('w'), Some('z')); // (3, 0)

        // azerty's é is at (0, 2), where colemak-dh has '2'.
        assert_eq!(e.translate('é'), Some('2'));
    }

    #[test]
    fn a_target_key_on_a_position_the_host_lacks_is_unreachable() {
        // azerty has no key at (3, 6); colemak-dh puts 'h' there.
        let e = emulator("azerty", "colemak-dh");

        assert!(!e.reachable().contains(&Position::new(3, 6)));
        assert_eq!(e.reachable().len(), 47, "the host board's 47 positions");
        assert!(!e.is_char_reachable('h'));
        assert!(e.is_char_reachable('k'), "k is at (3, 5), which azerty has");

        let unreachable = e.unreachable_chars();
        assert!(unreachable.contains(&'h'), "got {unreachable:?}");
        assert!(unreachable.contains(&'H'), "its shifted form too");
        assert!(!unreachable.contains(&'k'));
        assert_eq!(e.host_char_for('h'), None, "no host key to press for h");
        // azerty's own h is at (2, 5), which colemak-dh labels 'm'.
        assert_eq!(e.translate('h'), Some('m'));

        // The same gap under a different target names that target's letter.
        let e = emulator("azerty", "bepo");
        assert_eq!(e.unreachable_chars(), vec!['q', 'Q']);
    }

    #[test]
    fn a_shared_board_shape_leaves_nothing_unreachable() {
        let e = emulator("qwerty", "colemak-dh");
        assert_eq!(e.reachable().len(), e.target().keys.len());
        assert!(e.unreachable_chars().is_empty());
        // Reachability is positional: colemak-dh carries é as an altgr legend on
        // a shared key, so the position is reachable although no bare qwerty
        // keystroke produces é (translate of é is None: qwerty has no such key).
        assert!(e.is_char_reachable('é'));
        assert_eq!(e.translate('é'), None);
        assert!(e.is_char_reachable('f'));
    }

    #[test]
    fn a_layout_emulated_against_itself_is_the_identity() {
        let e = emulator("qwerty", "qwerty");
        for c in ['q', 'a', 'z', ' ', '/'] {
            assert_eq!(e.translate(c), Some(c), "{c:?} maps to itself");
            assert_eq!(e.host_char_for(c), Some(c));
        }
    }

    #[test]
    fn shifted_characters_resolve_through_the_same_position() {
        let e = emulator("azerty", "colemak-dh");
        // azerty's 'A' is the shifted legend of the (1, 0) key, where colemak
        // has its shifted 'Q'.
        assert_eq!(e.translate('A'), Some('Q'), "the shift level, not base");
        assert!(e.is_char_reachable('Q'), "colemak's shifted Q at (1, 0)");
        assert_eq!(e.host_char_for('Q'), Some('A'), "the shifted subscript");
    }

    #[test]
    fn a_capital_letter_survives_emulation() {
        let e = emulator("qwerty", "colemak-dh");

        // qwerty's 'E' is the shifted legend at (1, 2), which colemak-dh labels
        // with a shifted 'F'; the lower case pair behaves the same way.
        assert_eq!(e.translate('E'), Some('F'));
        assert_eq!(e.translate('e'), Some('f'));

        // The legend subscript follows the level too: E for F, e for f.
        assert_eq!(e.host_char_for('F'), Some('E'));
        assert_eq!(e.host_char_for('f'), Some('e'));

        // A digit's shifted form moves to whatever sits there on the target.
        assert_eq!(e.translate('!'), Some('!'), "both shift-'1' at (0, 1)");
    }
}
