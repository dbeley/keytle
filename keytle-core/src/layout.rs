//! Keyboard layouts as data: physical positions mapped to characters.
//!
//! A layout is not a picture; it is a map from `(row, col)` positions on a
//! 5-row ANSI-style board to the character each position produces, plus the
//! order in which its keys are taught. Rows are 0-based (0 = numbers, 1 = top,
//! 2 = home, 3 = bottom, 4 = space row) and `row_offset` is the visual stagger
//! of each row in half-keys, so every layout draws in the same shape.
//!
//! Layouts are parsed from TOML ([`Layout::from_toml`]) and validated on the
//! way in, so a malformed file fails with a named error instead of producing
//! a board that is subtly wrong.

use serde::Deserialize;
use std::collections::BTreeMap;

/// A physical key position on the board. Rows are 0-based with 0 = numbers,
/// 1 = top, 2 = home, 3 = bottom and 4 = space row; columns are 0-based within
/// a row. Ordering is row-major, so `BTreeMap<Position, _>` iterates the board
/// from the top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub row: u8,
    pub col: u8,
}

impl Position {
    pub const fn new(row: u8, col: u8) -> Position {
        Position { row, col }
    }
}

/// Which finger is meant to press a key. Written kebab-case in TOML:
/// `left-pinky`, `left-ring`, `left-middle`, `left-index`, `thumb`,
/// `right-index`, `right-middle`, `right-ring`, `right-pinky`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Finger {
    LeftPinky,
    LeftRing,
    LeftMiddle,
    LeftIndex,
    Thumb,
    RightIndex,
    RightMiddle,
    RightRing,
    RightPinky,
}

/// One occupied position and the characters it produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub pos: Position,
    pub base: char,
    pub shift: Option<char>,
    pub altgr: Option<char>,
    pub finger: Option<Finger>,
}

impl Key {
    /// Does pressing this key (with no modifier, shift or altgr) produce `c`?
    fn has_char(&self, c: char) -> bool {
        self.base == c || self.shift == Some(c) || self.altgr == Some(c)
    }
}

/// A whole layout: geometry, the occupied positions, and the order in which
/// the keys are taught.
#[derive(Debug, Clone)]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub rows: u8,
    pub row_offset: Vec<u8>,
    pub keys: BTreeMap<Position, Key>,
    pub unlock_order: BTreeMap<String, Vec<char>>,
}

impl Layout {
    /// Parse and validate a layout definition. A position holds at most one
    /// key, every `base` is exactly one character, every key lies on the
    /// declared board, and every unlock character is actually reachable.
    pub fn from_toml(src: &str) -> Result<Layout, LayoutError> {
        let raw: RawLayout = toml::from_str(src).map_err(|e| LayoutError::Toml(e.to_string()))?;

        if raw.rows == 0 {
            return Err(LayoutError::BadGeometry(
                "a board needs at least one row".to_string(),
            ));
        }
        if raw.row_offset.len() != usize::from(raw.rows) {
            return Err(LayoutError::BadGeometry(format!(
                "row_offset has {} entries for {} rows",
                raw.row_offset.len(),
                raw.rows
            )));
        }

        let mut keys = BTreeMap::new();
        for raw_key in raw.key {
            let pos = Position::new(raw_key.row, raw_key.col);
            if raw_key.row >= raw.rows {
                return Err(LayoutError::BadGeometry(format!(
                    "key at ({}, {}) on a board of {} rows",
                    raw_key.row, raw_key.col, raw.rows
                )));
            }
            let mut base_chars = raw_key.base.chars();
            let base = match (base_chars.next(), base_chars.next()) {
                (Some(c), None) => c,
                _ => return Err(LayoutError::MultiCharBase { key: raw_key.base }),
            };
            let key = Key {
                pos,
                base,
                shift: raw_key.shift,
                altgr: raw_key.altgr,
                finger: raw_key.finger,
            };
            if keys.insert(pos, key).is_some() {
                return Err(LayoutError::DuplicatePosition {
                    row: pos.row,
                    col: pos.col,
                });
            }
        }

        let layout = Layout {
            id: raw.id,
            name: raw.name,
            description: raw.description,
            rows: raw.rows,
            row_offset: raw.row_offset,
            keys,
            unlock_order: raw.unlock_order,
        };

        for chars in layout.unlock_order.values() {
            for &c in chars {
                if !layout.contains_char(c) {
                    return Err(LayoutError::UnknownUnlockChar { r#char: c });
                }
            }
        }

        Ok(layout)
    }

    /// The key at `pos`, if that position is occupied.
    pub fn key_at(&self, pos: Position) -> Option<&Key> {
        self.keys.get(&pos)
    }

    /// Where the character `c` lives, whether it is on `base`, `shift` or
    /// `altgr`. When the same character appears more than once the first
    /// position in row-major order wins.
    pub fn position_of_base(&self, c: char) -> Option<Position> {
        self.keys
            .values()
            .find(|key| key.has_char(c))
            .map(|key| key.pos)
    }

    /// The teaching order for `lang`, falling back to `default`, then to an
    /// empty order.
    pub fn unlock_order_for(&self, lang: &str) -> Vec<char> {
        self.unlock_order
            .get(lang)
            .or_else(|| self.unlock_order.get("default"))
            .cloned()
            .unwrap_or_default()
    }

    /// Does any key of the layout produce `c`?
    pub fn contains_char(&self, c: char) -> bool {
        self.keys.values().any(|key| key.has_char(c))
    }
}

/// Why a layout definition was rejected. Every variant names the offending
/// value.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("two keys sit at ({row}, {col})")]
    DuplicatePosition { row: u8, col: u8 },
    #[error("base {key:?} is not a single character")]
    MultiCharBase { key: String },
    #[error("unlock_order names {ch:?}, which no key produces", ch = .r#char)]
    UnknownUnlockChar { r#char: char },
    #[error("bad layout TOML: {0}")]
    Toml(String),
    #[error("bad board geometry: {0}")]
    BadGeometry(String),
}

/// The file as written, before validation. `base` stays a string here so a
/// multi-character base can be reported by value rather than as a generic
/// deserialisation failure.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    id: String,
    name: String,
    description: Option<String>,
    rows: u8,
    row_offset: Vec<u8>,
    #[serde(default)]
    key: Vec<RawKey>,
    #[serde(default)]
    unlock_order: BTreeMap<String, Vec<char>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    row: u8,
    col: u8,
    base: String,
    shift: Option<char>,
    altgr: Option<char>,
    finger: Option<Finger>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three keys on three different rows, one with an altgr legend, and no
    /// unlock order.
    const THREE_KEYS: &str = r#"
id = "three"
name = "Three Keys"
description = "a tiny layout for tests"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 2
base = "f"
shift = "F"
finger = "left-index"

[[key]]
row = 2
col = 3
base = "r"
shift = "R"
altgr = "®"
finger = "left-index"

[[key]]
row = 4
col = 5
base = " "
"#;

    /// Two keys with a default order and a French override.
    const UNLOCKED: &str = r#"
id = "unlock"
name = "Unlock"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 2
base = "a"

[[key]]
row = 2
col = 3
base = "b"

[unlock_order]
default = ["a", "b"]
fr = ["b", "a"]
"#;

    /// A one-key layout whose home-row key has `base`.
    fn one_key(base: &str) -> String {
        format!(
            r#"
id = "one"
name = "One"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 2
base = {base:?}
"#
        )
    }

    #[test]
    fn parses_keys_and_round_trips_every_character() {
        let layout = Layout::from_toml(THREE_KEYS).expect("the three-key layout parses");

        assert_eq!(layout.id, "three");
        assert_eq!(layout.name, "Three Keys");
        assert_eq!(
            layout.description.as_deref(),
            Some("a tiny layout for tests")
        );
        assert_eq!(layout.rows, 5);
        assert_eq!(layout.row_offset, [0, 1, 1, 1, 2]);
        assert_eq!(layout.keys.len(), 3);

        let f = layout
            .key_at(Position::new(2, 2))
            .expect("a key on the home row");
        assert_eq!(f.base, 'f');
        assert_eq!(f.shift, Some('F'));
        assert_eq!(f.altgr, None);
        assert_eq!(f.finger, Some(Finger::LeftIndex));

        let r = layout.key_at(Position::new(2, 3)).expect("its neighbour");
        assert_eq!(r.altgr, Some('®'));
        assert_eq!(r.finger, Some(Finger::LeftIndex));

        let space = layout
            .key_at(Position::new(4, 5))
            .expect("a key on the space row");
        assert_eq!(space.base, ' ');
        assert_eq!(space.finger, None);

        for c in ['f', 'F', 'r', 'R', '®', ' '] {
            assert!(layout.contains_char(c), "{c:?} is on the layout");
            assert!(layout.position_of_base(c).is_some(), "{c:?} has a home");
        }

        assert_eq!(layout.position_of_base('f'), Some(Position::new(2, 2)));
        assert_eq!(layout.position_of_base('F'), Some(Position::new(2, 2)));
        assert_eq!(layout.position_of_base('r'), Some(Position::new(2, 3)));
        assert_eq!(layout.position_of_base('R'), Some(Position::new(2, 3)));
        assert_eq!(layout.position_of_base('®'), Some(Position::new(2, 3)));
        assert_eq!(layout.position_of_base(' '), Some(Position::new(4, 5)));
        assert_eq!(layout.position_of_base('z'), None);
        assert!(!layout.contains_char('z'));
        assert!(layout.key_at(Position::new(0, 0)).is_none());
    }

    #[test]
    fn positions_sort_by_row_then_column() {
        let mut positions = [
            Position::new(2, 1),
            Position::new(1, 3),
            Position::new(1, 0),
            Position::new(0, 9),
        ];
        positions.sort();
        assert_eq!(
            positions,
            [
                Position::new(0, 9),
                Position::new(1, 0),
                Position::new(1, 3),
                Position::new(2, 1),
            ]
        );
    }

    #[test]
    fn a_duplicate_position_is_rejected_by_name() {
        let src = r#"
id = "dup"
name = "Dup"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 1
col = 2
base = "w"

[[key]]
row = 1
col = 2
base = "e"
"#;
        let err = Layout::from_toml(src).expect_err("two keys at the same spot");
        assert!(
            matches!(err, LayoutError::DuplicatePosition { row: 1, col: 2 }),
            "got {err:?}"
        );
        assert!(
            err.to_string().contains("(1, 2)"),
            "message names it: {err}"
        );
    }

    #[test]
    fn a_multi_character_base_is_rejected_by_name() {
        let err = Layout::from_toml(&one_key("ab")).expect_err("a base is one character");
        assert!(
            matches!(&err, LayoutError::MultiCharBase { key } if key == "ab"),
            "got {err:?}"
        );
        assert!(err.to_string().contains("ab"), "message names it: {err}");

        let err = Layout::from_toml(&one_key("")).expect_err("an empty base is not a character");
        assert!(
            matches!(err, LayoutError::MultiCharBase { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn an_unlock_character_the_layout_lacks_is_rejected_by_name() {
        let src = UNLOCKED.replace(r#"fr = ["b", "a"]"#, r#"fr = ["b", "z"]"#);
        let err = Layout::from_toml(&src).expect_err("no key produces z");
        assert!(
            matches!(err, LayoutError::UnknownUnlockChar { r#char: 'z' }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("'z'"), "message names it: {err}");

        let src = UNLOCKED.replace(r#"default = ["a", "b"]"#, r#"default = ["a", "x"]"#);
        let err = Layout::from_toml(&src).expect_err("the default order names a missing key");
        assert!(
            matches!(err, LayoutError::UnknownUnlockChar { r#char: 'x' }),
            "got {err:?}"
        );
    }

    #[test]
    fn finger_names_are_kebab_case() {
        #[derive(Deserialize)]
        struct Named {
            finger: Finger,
        }
        let parse = |name: &str| {
            toml::from_str::<Named>(&format!("finger = {name:?}"))
                .unwrap_or_else(|e| panic!("{name} is a finger: {e}"))
                .finger
        };
        assert_eq!(parse("left-pinky"), Finger::LeftPinky);
        assert_eq!(parse("left-ring"), Finger::LeftRing);
        assert_eq!(parse("left-middle"), Finger::LeftMiddle);
        assert_eq!(parse("left-index"), Finger::LeftIndex);
        assert_eq!(parse("thumb"), Finger::Thumb);
        assert_eq!(parse("right-index"), Finger::RightIndex);
        assert_eq!(parse("right-middle"), Finger::RightMiddle);
        assert_eq!(parse("right-ring"), Finger::RightRing);
        assert_eq!(parse("right-pinky"), Finger::RightPinky);
        assert!(
            toml::from_str::<Named>("finger = \"LeftIndex\"").is_err(),
            "the file spelling is kebab-case, not Rust case"
        );
    }

    #[test]
    fn unlock_order_for_prefers_the_language_entry() {
        let layout = Layout::from_toml(UNLOCKED).expect("the unlock layout parses");
        assert_eq!(layout.unlock_order_for("fr"), ['b', 'a']);
        assert_eq!(
            layout.unlock_order_for("en"),
            ['a', 'b'],
            "falls back to default"
        );
        assert_eq!(layout.unlock_order_for("de"), ['a', 'b']);
        assert_eq!(layout.unlock_order_for("default"), ['a', 'b']);
    }

    #[test]
    fn a_layout_without_an_unlock_order_has_an_empty_one() {
        let layout = Layout::from_toml(THREE_KEYS).expect("the three-key layout parses");
        assert!(layout.unlock_order.is_empty());
        assert_eq!(layout.unlock_order_for("en"), Vec::<char>::new());
    }

    #[test]
    fn keys_outside_the_declared_board_are_bad_geometry() {
        let too_tall = r#"
id = "tall"
name = "Tall"
rows = 3
row_offset = [0, 1, 1]

[[key]]
row = 4
col = 0
base = "a"
"#;
        let err = Layout::from_toml(too_tall).expect_err("row 4 of a 3-row board");
        assert!(matches!(err, LayoutError::BadGeometry(_)), "got {err:?}");
        assert!(
            err.to_string().contains("4"),
            "message names the row: {err}"
        );

        let ragged = r#"
id = "ragged"
name = "Ragged"
rows = 5
row_offset = [0, 1, 1]
"#;
        let err = Layout::from_toml(ragged).expect_err("3 offsets for 5 rows");
        assert!(matches!(err, LayoutError::BadGeometry(_)), "got {err:?}");
        assert!(
            err.to_string().contains("row_offset"),
            "message names the field: {err}"
        );

        let empty = r#"
id = "empty"
name = "Empty"
rows = 0
row_offset = []
"#;
        let err = Layout::from_toml(empty).expect_err("a board needs a row");
        assert!(matches!(err, LayoutError::BadGeometry(_)), "got {err:?}");
    }

    #[test]
    fn broken_toml_is_a_toml_error() {
        let err = Layout::from_toml("id = \"only-an-id\"").expect_err("fields are missing");
        assert!(matches!(err, LayoutError::Toml(_)), "got {err:?}");
        assert!(
            err.to_string().contains("name"),
            "toml names the field: {err}"
        );
    }
}
