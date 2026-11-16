# keytle

Teach yourself a keyboard layout from the terminal — offline, no account, no
browser. One binary drills you key by key, keybr-style: you start from four
home-row letters, and a new letter unlocks only once the ones you own are fast
and accurate. Run `keytle` in any terminal; it needs no network.

## Emulation: learn a layout you never installed

You keep typing on the board you already have. A terminal reports the character
your *host* layout produces, not the physical key, so keytle looks that character
up on the host board to recover the pressed position and reads the target layout
there: `e` on QWERTY counts as `f` under Colemak-DH, both sitting at `(1, 2)`.
No system remap, no reboot.

Characters the host cannot produce at the same *level* are unreachable — Colemak-
DH's AltGr `é` has no QWERTY keystroke — so lessons and the unlock rule skip them
rather than block progress forever. `keytle doctor` names them: for
`host_layout = "qwerty"` and `layout = "colemak-dh"` it reports `43 unreachable
characters (… é ç …) — no host keystroke produces them at their level, so lessons
skip them`.

## Layouts and corpora are data files

Bundled: `qwerty`, `azerty`, `colemak-dh`, `bepo`, `gallium` (`keytle layouts list`
shows each id, name, and whether it is bundled or yours).

Drop a layout at `~/.config/keytle/layouts/<id>.toml` to add one or to shadow a
bundled one. It is a grid of keys plus the order its letters are taught:

```toml
id = "colemak-dh"
name = "Colemak-DH (ANSI)"
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 2
col = 0
base = "a"
shift = "A"
altgr = "á"

# ...one [[key]] per position...

[unlock_order]
default = ["t", "n", "s", "e", "r", "i", "a", "o"]
```

A `finger = "left-index"` line is still accepted in a layout file, but nothing
draws it: the board shows keys, not hands.

`en` and `fr` corpora are compiled in, so it works out of the box: English words
are the FrequencyWords list checked against a real dictionary, French words come
from the Lexique 3.83 lexicon, and both keep their published frequencies. `keytle-core`
can also merge `<dir>/<lang>/words.txt` (`word frequency` per line) and
`<dir>/<lang>/sentences.txt` on top; the intended location is
`~/.config/keytle/corpora/<lang>/`, which the TUI reads, so a file dropped there
is used by the next lesson.

## How learning works

* A new profile starts with the first **four** letters of the teach order (`t n s
  e` for Colemak-DH); lessons use only unlocked letters.
* Each key keeps a **confidence** in `0.0..=1.0` mixing speed and accuracy:
  `0.6 * speed + 0.4 * accuracy`. Speed against your `target_wpm` carries more
  weight because progression is about fluency; accuracy has a separate floor.
* A lesson qualifies when the mean confidence of your unlocked, reachable keys
  reaches **0.7** and no key sits below **0.93** accuracy. One more letter then
  unlocks — at most **three per session**, and a session restarts after thirty
  idle minutes, so a lucky streak cannot run away with the alphabet.
* After the letters come punctuation, then French accents for `fr`. Every unlock
  is automatic: there is no key that skips ahead, so the profile can never hold
  a letter the lessons did not earn.
* Lessons are **real dictionary words** (`words` mode) unless the settings say
  otherwise: the bundled lists are filtered against a real lexicon, so what you
  type is English or French rather than generated text. `pseudo` still exists
  for a drill that can use any letter on the first day, and `sentences` types
  real prose once the alphabet is wide enough.
* The home screen is the drill's front page: it shows the test count, latest
  and best speed, consistency and the weakest keys, and every key that leads
  somewhere (`s` stats, `,` settings, `?` help). A finished test starts the next
  one by itself after three seconds — or as soon as you type — and `esc` on the
  results holds it while you read.

## Install

```sh
nix run github:<owner>/keytle      # try it without installing
nix build                          # ./result/bin/keytle
cargo install --path keytle        # from a checkout; Rust 1.85+
```

NixOS and home-manager modules are exported as `nixosModules.default` and
`homeModules.default`, both under the same option path, `services.keytle`:

```nix
services.keytle = {
  enable = true;
  settings = { layout = "colemak-dh"; language = "fr"; target_wpm = 90.0; };
};
```

`settings` is rendered to `config.toml` — `/etc/keytle/config.toml` on NixOS
(with `KEYTLE_CONFIG_DIR` pointing there), `$XDG_CONFIG_HOME/keytle/config.toml`
under home-manager. Every key is optional; unknown keys are rejected, so a typo
fails loudly rather than being silently ignored.

## Usage

| command | what it does |
| --- | --- |
| `keytle`, `keytle run` | start the trainer |
| `keytle stats [--layout L] [--lang L] [--mode M] [--last N]` | dashboard as plain text |
| `keytle doctor` | paths, config, version, `TERM`, reachability |
| `keytle export [--format jsonl\|tsv] [-o PATH]` | raw records to stdout or a file |
| `keytle layouts list` | layout ids with names and source |

Keys: `enter`/`space` start a lesson, `esc` abort, `tab` restart, `s` stats, `?`
help, `,` settings (an overlay over the typing view: layout, host layout,
language, mode, emulation, strictness, lesson length, target speed and automatic
next test), `m` merge modes, `c` chart mode; `q` quits. A settings edit applies
the moment it is made, and leaving the panel with `esc` (or `w` while still in
it) writes the file, so there is no unsaved state to lose.
Progression has no key of its own: the engine unlocks the next letter when a
lesson clears the gate, and nothing else can.

## Where data lives

No database: append-only JSONL and small JSON files, so `grep`, `jq` and `git`
work on them.

```
~/.config/keytle/config.toml        settings: layout, host_layout, language, mode,
                                    emulate, strict, lesson_words, target_wpm,
                                    auto_next
~/.config/keytle/layouts/*.toml     your layouts
~/.local/share/keytle/tests.jsonl   one record per finished test
~/.local/share/keytle/profiles/     <layout>.<lang>.json — unlocked keys, evidence
```

Defaults: Colemak-DH emulated on QWERTY, English, `words`, 20 words, 80 wpm,
next test after 3 s of an untouched results screen (`auto_next`).
`KEYTLE_CONFIG_DIR` / `KEYTLE_DATA_DIR` override the roots; otherwise the XDG
variables, then `~/.config` / `~/.local/share`.

## License

MIT OR Apache-2.0, at your option. Bundled corpora keep their own licences —
FrequencyWords (CC-BY-SA 4.0, derived from OpenSubtitles) and Tatoeba (CC-BY 2.0
FR) — with provenance in `keytle-core/src/data/corpora/NOTICE.md`.
