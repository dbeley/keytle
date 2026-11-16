//! The layouts keytle ships with, and the user files that override them.
//!
//! [`BUNDLED`] holds the five layouts compiled into the binary, so `keytle`
//! works offline and on a fresh install. A layout of the same id in the user's
//! layout directory wins over the bundled one, which lets someone tweak a
//! legend, a finger or a teaching order without forking. New ids in that
//! directory are picked up too, so custom layouts are first-class.

use crate::layout::{Layout, LayoutError};
use std::collections::BTreeSet;
use std::path::Path;

/// The bundled layouts, as `(id, TOML source)`, in the order `keytle` offers
/// them: the two host keyboards first, then the layouts worth learning.
pub static BUNDLED: &[(&str, &str)] = &[
    ("qwerty", include_str!("data/layouts/qwerty.toml")),
    ("azerty", include_str!("data/layouts/azerty.toml")),
    ("colemak-dh", include_str!("data/layouts/colemak-dh.toml")),
    ("bepo", include_str!("data/layouts/bepo.toml")),
    ("gallium", include_str!("data/layouts/gallium.toml")),
];

/// The layouts keytle can treat as "the keyboard in front of you" when it
/// emulates a different board on screen.
pub fn host_ids() -> &'static [&'static str] {
    &["qwerty", "azerty"]
}

/// The bundled source for `id`, if `keytle` ships such a layout.
pub fn bundled_source(id: &str) -> Option<&'static str> {
    BUNDLED
        .iter()
        .find(|(bundled, _)| *bundled == id)
        .map(|(_, src)| *src)
}

/// Load the layout `id`: the user's `<user_dir>/<id>.toml` if there is one,
/// otherwise the bundled definition. A user file whose own `id` disagrees with
/// its file name is rejected, so `load("qwerty")` can never hand back a
/// different layout.
pub fn load(id: &str, user_dir: &Path) -> Result<Layout, LayoutError> {
    let path = user_dir.join(format!("{id}.toml"));
    if path.is_file() {
        let src = std::fs::read_to_string(&path).map_err(|source| LayoutError::Io {
            path: path.clone(),
            source,
        })?;
        let layout = Layout::from_toml(&src)?;
        if layout.id != id {
            return Err(LayoutError::IdMismatch {
                path,
                expected: id.to_string(),
                found: layout.id,
            });
        }
        return Ok(layout);
    }

    match bundled_source(id) {
        Some(src) => Layout::from_toml(src),
        None => Err(LayoutError::UnknownLayout { id: id.to_string() }),
    }
}

/// Every layout id `load` would accept: the bundled ones plus the user's own
/// `.toml` files, sorted and deduplicated. A missing or unreadable user
/// directory simply contributes nothing.
pub fn available(user_dir: &Path) -> Vec<String> {
    let mut ids: BTreeSet<String> = BUNDLED.iter().map(|(id, _)| (*id).to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(user_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                ids.insert(stem.to_string());
            }
        }
    }
    ids.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Finger, Position};
    use std::path::PathBuf;

    /// A scratch directory that cleans itself up, so tests need no tempfile
    /// crate and do not touch the real config.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("keytle-layouts-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a scratch layout dir");
            Scratch(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.0.join(name), contents).expect("a written layout file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bundled(id: &str) -> Layout {
        let empty = Path::new("/nonexistent/keytle/layouts");
        load(id, empty).unwrap_or_else(|e| panic!("bundled {id} loads: {e}"))
    }

    /// The `base` characters of one row, left to right, skipping empty columns.
    fn row(layout: &Layout, row: u8) -> String {
        layout
            .keys
            .values()
            .filter(|key| key.pos.row == row)
            .map(|key| key.base)
            .collect()
    }

    #[test]
    fn every_bundled_id_parses_and_validates() {
        assert_eq!(BUNDLED.len(), 5, "five layouts ship with keytle");

        let mut ids: Vec<&str> = BUNDLED.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids,
            ["qwerty", "azerty", "colemak-dh", "bepo", "gallium"],
            "in offer order"
        );
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), BUNDLED.len(), "ids are unique");

        for (id, src) in BUNDLED {
            let layout = Layout::from_toml(src)
                .unwrap_or_else(|e| panic!("bundled layout {id} is valid: {e}"));
            assert_eq!(&layout.id, id, "the file id matches its bundle id");
            assert!(!layout.name.is_empty(), "{id} has a name");
            assert_eq!(layout.rows, 5, "{id} uses the five-row board");
            assert_eq!(
                layout.row_offset,
                [0, 1, 1, 1, 2],
                "{id} uses the same stagger"
            );
            assert!(
                layout.key_at(Position::new(4, 0)).is_some(),
                "{id} has a space key"
            );
        }
    }

    #[test]
    fn every_layout_types_all_26_letters() {
        for (id, _) in BUNDLED {
            let layout = bundled(id);
            for letter in 'a'..='z' {
                assert!(layout.contains_char(letter), "{id} is missing {letter:?}");
            }
        }
    }

    #[test]
    fn no_bundled_layout_places_two_keys_on_one_position() {
        for (id, src) in BUNDLED {
            let written = src.matches("[[key]]").count();
            let layout = Layout::from_toml(src).expect("a valid layout");
            assert_eq!(
                layout.keys.len(),
                written,
                "{id} has {written} [[key]] blocks but {} distinct positions",
                layout.keys.len()
            );
        }
    }

    #[test]
    fn every_unlock_character_exists_in_its_layout() {
        for (id, _) in BUNDLED {
            let layout = bundled(id);
            assert!(
                !layout.unlock_order.is_empty(),
                "{id} teaches its keys in some order"
            );
            for (lang, chars) in &layout.unlock_order {
                for &c in chars {
                    assert!(
                        layout.contains_char(c),
                        "{id}'s {lang} order names {c:?}, which it cannot type"
                    );
                }
            }
        }
    }

    #[test]
    fn every_letter_key_names_a_finger() {
        for (id, _) in BUNDLED {
            let layout = bundled(id);
            for key in layout.keys.values() {
                if key.base.is_ascii_alphabetic() {
                    assert!(
                        key.finger.is_some(),
                        "{id} leaves {:?} at ({}, {}) without a finger",
                        key.base,
                        key.pos.row,
                        key.pos.col
                    );
                }
            }
        }
    }

    #[test]
    fn every_letter_key_shifts_to_its_uppercase_form() {
        for (id, _) in BUNDLED {
            let layout = bundled(id);
            for key in layout.keys.values() {
                if key.base.is_ascii_alphabetic() {
                    assert_eq!(
                        key.shift,
                        Some(key.base.to_ascii_uppercase()),
                        "{id} cannot type the capital of {:?} at ({}, {})",
                        key.base,
                        key.pos.row,
                        key.pos.col
                    );
                }
            }
        }
    }

    #[test]
    fn qwerty_rows_are_the_us_layout() {
        let layout = bundled("qwerty");
        assert_eq!(row(&layout, 0), "`1234567890-=");
        assert_eq!(row(&layout, 1), "qwertyuiop[]\\");
        assert_eq!(row(&layout, 2), "asdfghjkl;'");
        assert_eq!(row(&layout, 3), "zxcvbnm,./");
        assert_eq!(row(&layout, 4), " ");
        assert_eq!(
            layout.position_of_base('f'),
            Some(Position::new(2, 3)),
            "the home key under the left index"
        );
        // Frequency first, so English words form from the first lesson, then the
        // layout's own home-row walk, then the remaining letters by row.
        let order = layout.unlock_order_for("en");
        assert_eq!(&order[..8], &['e', 't', 'a', 'o', 'i', 'n', 's', 'h']);
        assert_eq!(
            &order[8..],
            &[
                'f', 'j', 'd', 'k', 'l', ';', 'g', 'q', 'w', 'r', 'y', 'u', 'p', 'z', 'x', 'c',
                'v', 'b', 'm'
            ]
        );
    }

    #[test]
    fn azerty_rows_are_the_french_layout() {
        let layout = bundled("azerty");
        assert_eq!(row(&layout, 0), "²&é\"'(-è_çà)=");
        assert_eq!(row(&layout, 1), "azertyuiop^$*");
        assert_eq!(row(&layout, 2), "qsdfghjklmù");
        assert_eq!(row(&layout, 3), "wxcvbn,;:");
        assert_eq!(
            layout.key_at(Position::new(3, 6)),
            None,
            "the AZERTY bottom row is one key short of ten"
        );
        let comma = layout.key_at(Position::new(3, 7)).expect("the comma key");
        assert_eq!(comma.base, ',');
        assert_eq!(comma.shift, Some('?'), "AZERTY puts ? on shift-comma");
        // French starts on French letters, and the order still covers all of
        // them, accents included: a language array is an order, not a list of
        // extras bolted on the end.
        assert_eq!(
            &layout.unlock_order_for("fr")[..8],
            &['e', 's', 'a', 'i', 't', 'n', 'r', 'u']
        );
        assert!(layout.unlock_order_for("fr").contains(&'é'));
        assert!(layout.unlock_order_for("fr").contains(&'à'));
        assert_eq!(
            &layout.unlock_order_for("en")[..8],
            &['e', 't', 'a', 'o', 'i', 'n', 's', 'h']
        );
    }

    #[test]
    fn colemak_dh_rows_are_the_dh_ansi_layout() {
        let layout = bundled("colemak-dh");
        assert_eq!(row(&layout, 0), "`1234567890-=");
        assert_eq!(row(&layout, 1), "qwfpbjluy;[]\\");
        assert_eq!(row(&layout, 2), "arstgmneio'");
        assert_eq!(row(&layout, 3), "zxcdvkh,./");
        // f sits at the home row's left-index position, u under the right
        // index, and the DH mod keeps d and h down on the bottom row.
        assert_eq!(layout.position_of_base('f'), Some(Position::new(1, 2)));
        assert_eq!(layout.position_of_base('u'), Some(Position::new(1, 7)));
        assert_eq!(layout.position_of_base('d'), Some(Position::new(3, 3)));
        assert_eq!(layout.position_of_base('h'), Some(Position::new(3, 6)));
        // ... and b and g moved up to the top and home rows.
        assert_eq!(layout.position_of_base('b'), Some(Position::new(1, 4)));
        assert_eq!(layout.position_of_base('g'), Some(Position::new(2, 4)));
        let f = layout
            .key_at(Position::new(1, 2))
            .expect("f on the top row");
        assert_eq!(f.altgr, Some('ã'), "the altgr legend comes along");
        assert_eq!(f.finger, Some(Finger::LeftMiddle));
    }

    #[test]
    fn bepo_rows_are_the_french_ergonomic_layout() {
        let layout = bundled("bepo");
        assert_eq!(row(&layout, 0), "$\"«»()@+-/*=%");
        assert_eq!(row(&layout, 1), "bépoè^vdljzwç");
        assert_eq!(row(&layout, 2), "auie,ctsrnm");
        assert_eq!(row(&layout, 3), "àyx.k'qghf");
        assert_eq!(layout.position_of_base('é'), Some(Position::new(1, 1)));
        assert_eq!(layout.position_of_base('e'), Some(Position::new(2, 3)));
        let e = layout
            .key_at(Position::new(2, 3))
            .expect("e on the home row");
        assert_eq!(e.altgr, Some('€'), "bépo's altgr euro");
        let order = layout.unlock_order_for("fr");
        assert_eq!(&order[..8], &['e', 's', 'a', 'i', 't', 'n', 'r', 'u']);
        assert!(['é', 'è', 'à', 'ç'].iter().all(|ch| order.contains(ch)));
    }

    #[test]
    fn gallium_rows_are_the_gallium_layout() {
        let layout = bundled("gallium");
        assert_eq!(row(&layout, 0), "`1234567890-=");
        assert_eq!(row(&layout, 1), "bldcvjfou,[]\\");
        assert_eq!(row(&layout, 2), "nrtsgyhaei/");
        assert_eq!(row(&layout, 3), "xqmwzkp';.");
        assert_eq!(layout.position_of_base('g'), Some(Position::new(2, 4)));
    }

    #[test]
    fn host_ids_are_the_two_keyboards_keytle_can_emulate() {
        assert_eq!(host_ids(), ["qwerty", "azerty"]);
        for id in host_ids() {
            assert!(
                BUNDLED.iter().any(|(bundled, _)| bundled == id),
                "{id} is a bundled layout"
            );
        }
    }

    #[test]
    fn a_user_file_replaces_the_bundled_layout_of_the_same_id() {
        let scratch = Scratch::new("override");
        scratch.write(
            "qwerty.toml",
            r#"
id = "qwerty"
name = "QWERTY with my tweaks"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 2
base = "x"
finger = "left-middle"

[unlock_order]
default = ["x"]
"#,
        );

        let layout = load("qwerty", scratch.path()).expect("the user layout loads");
        assert_eq!(layout.name, "QWERTY with my tweaks");
        assert_eq!(
            layout.keys.len(),
            1,
            "the user file replaces the bundled one"
        );

        let bundled_qwerty = bundled("qwerty");
        assert_eq!(
            bundled_qwerty.name, "QWERTY (US)",
            "the bundled one is untouched"
        );
    }

    #[test]
    fn a_user_layout_with_a_new_id_is_listed_and_loadable() {
        let scratch = Scratch::new("new-id");
        scratch.write(
            "workman.toml",
            r#"
id = "workman"
name = "Workman"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 2
base = "d"
finger = "left-middle"

[unlock_order]
default = ["d"]
"#,
        );

        let ids = available(scratch.path());
        assert_eq!(
            ids,
            [
                "azerty",
                "bepo",
                "colemak-dh",
                "gallium",
                "qwerty",
                "workman"
            ],
            "bundled ids plus the user's, sorted"
        );

        let layout = load("workman", scratch.path()).expect("the user layout loads");
        assert_eq!(layout.id, "workman");
        assert_eq!(layout.name, "Workman");
    }

    #[test]
    fn available_ignores_other_files_and_a_missing_directory() {
        let scratch = Scratch::new("ignored");
        scratch.write("notes.txt", "not a layout");
        scratch.write("qwerty.toml.bak", "not a layout either");

        assert_eq!(available(scratch.path()).len(), BUNDLED.len());
        assert_eq!(
            available(Path::new("/nonexistent/keytle/layouts")).len(),
            BUNDLED.len(),
            "a missing user dir is not an error"
        );
    }

    #[test]
    fn an_unknown_id_and_a_broken_user_file_are_named_errors() {
        let empty = Path::new("/nonexistent/keytle/layouts");
        let err = load("workman", empty).expect_err("no such bundled layout");
        assert!(
            matches!(&err, LayoutError::UnknownLayout { id } if id == "workman"),
            "got {err:?}"
        );
        assert!(
            err.to_string().contains("workman"),
            "message names it: {err}"
        );

        let scratch = Scratch::new("broken");
        scratch.write("bad.toml", "id = \"bad\"\n");
        let err = load("bad", scratch.path()).expect_err("a layout without a name");
        assert!(matches!(err, LayoutError::Toml(_)), "got {err:?}");

        scratch.write(
            "mismatched.toml",
            r#"
id = "something-else"
name = "Something Else"
rows = 5
row_offset = [0, 1, 1, 1, 2]
"#,
        );
        let err =
            load("mismatched", scratch.path()).expect_err("the id does not match the file name");
        assert!(
            matches!(&err, LayoutError::IdMismatch { expected, found, .. }
                if expected == "mismatched" && found == "something-else"),
            "got {err:?}"
        );
    }
}
