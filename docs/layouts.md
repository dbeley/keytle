# Layouts

A layout is data, not a picture: a grid of key positions, the character each
position produces at the base, shift and altgr levels, and the order its letters
are taught. keytle ships five - `qwerty`, `azerty`, `colemak-dh`, `bepo` and
`gallium` - and `keytle layouts list` shows each id, name and whether it is
bundled or yours.

## Where the files live

- **Bundled** layouts are compiled into the binary, so a fresh install works
  offline.
- **Yours** live in `<config>/layouts/<id>.toml`, which is
  `~/.config/keytle/layouts/` unless `KEYTLE_CONFIG_DIR` or `$XDG_CONFIG_HOME`
  move that root.

A file whose name matches a bundled id *shadows* it, which is how you tweak a
legend or a teaching order without forking keytle. New ids are first-class
layouts. The `id` inside the file must match its file name, so `load("qwerty")`
can never hand back a different board.

## The file

```toml
id = "colemak-dh"
name = "Colemak-DH (ANSI)"
description = "Colemak with the DH mod: angled index fingers, less same-finger work."
rows = 5
row_offset = [0, 1, 1, 1, 2]      # visual stagger per row, in half keys

[[key]]
row = 2
col = 0
base = "a"
shift = "A"
altgr = "á"                       # optional
finger = "left-pinky"             # optional, accepted, not drawn

# ...one [[key]] per position...

[unlock_order]
en = ["e", "t", "a", "o", "i", "n", "s", "h", ...]   # what is taught, in order
fr = ["e", "s", "a", "i", "t", "n", "r", "u", ...]
default = ["t", "n", "s", "e", "r", "i", "a", "o", ...]
```

Rows are 0-based: 0 is the number row, 1 top, 2 home, 3 bottom and 4 the space
row. The file is validated on the way in, so it fails with a named error rather
than producing a subtly wrong board: `row_offset` needs one entry per row, every
`base` is exactly one character, a position holds at most one key, and every
character in `unlock_order` has to exist on some key.

## The teaching order

A list is the whole teaching order, not an extra rung: it is walked from the
first character to the last. The bundled lists start with the eight most common
letters of the language the layout can type, then fall back on the layout's own
home-row order and the remaining rows, so words form from the first lesson and
posture still comes before the rare keys. Once the list is exhausted, the
punctuation prose needs (`' , . ; : ! ?`) comes next and, for French, its accented
letters.

A fresh profile starts with the first **four** characters of the list. [How it
learns](../README.md#how-it-learns) describes the gate that unlocks the rest.

## Host layouts

`qwerty` and `azerty` double as the boards keytle can treat as *the keyboard in
front of you*. When emulating, a target character the host cannot produce at the
same level is unreachable - Colemak-DH's altgr `é` has no QWERTY keystroke - so
lessons and the unlock rule skip it instead of asking for an impossible
keystroke. `keytle doctor` names them.
