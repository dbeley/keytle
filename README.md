# keytle

keytle is a TUI utility to help you to learn a new keyboard layout and track your progress over time.

- Support for multiple layouts (qwerty, azerty, bépo, colemak-dh, gallium, + your own!)
- Stats and charts to track your progress
- Learning algorithm that increase difficulty and unlock new letters as you learn the keyboard layout
- Emulate keyboard layouts to test uncommon layouts witout having to install it on your operating system
- English and French dictionary

![keytle mid-lesson: the words to type above, the board below with the next key
highlighted](docs/screenshots/typing.png)

## Quick start

```sh
nix run github:dbeley/keytle     # try it without installing
cargo install --path keytle      # or build a checkout; Rust 1.85+
```

Out of the box it teaches Colemak-DH emulated on your QWERTY keyboard, with
English words, 20 words a lesson and an 80 wpm target. `enter` (or `space`)
starts a lesson, `,` opens the settings, `s` the charts, `?` the help, `q` quits. It needs a
terminal of at least 80×24 and nothing else.

## How it learns

- A fresh profile knows the first four letters of the teaching order for your
  language, and a lesson only ever uses letters you have unlocked.
- Every key keeps a confidence mixing speed and accuracy. A finished lesson
  unlocks the next letter when the mean confidence of your unlocked keys reaches
  **0.6** and no key sits below **90%** accuracy over its recent keystrokes.
- There is no session cap and no unlock key: a typist who already knows the layout
  can climb to their real level in one sitting, and nothing can hand out a letter
  the lessons did not earn.
- The letter standing between you and the next unlock shows up in more words than
  its frequency alone would give it, so the drill works on what blocks it.
- A finished lesson starts the next one by itself after three seconds. `esc` on
  the results screen holds it while you read.

## Progress

`s` opens the dashboard: speed and accuracy per test, the personal best, session
breaks and unlocks, a per-key table, and what the next unlock is waiting for.
`keytle stats` prints the same numbers as plain text.

![the stats dashboard: speed per test, the per-key table, the next
unlock](docs/screenshots/stats.png)

## Install

`nix build` puts the binary at `./result/bin/keytle`, `cargo install --path
keytle` installs a checkout, and `nix run github:dbeley/keytle` runs it straight
from the flake without installing anything.

NixOS and home-manager modules are exported as `nixosModules.default` and
`homeModules.default`, both under the same option path, `services.keytle`:

```nix
services.keytle = {
  enable = true;
  settings = { layout = "colemak-dh"; language = "fr"; target_wpm = 90.0; };
};
```

<details>
<summary>Where those settings land</summary>

`settings` is rendered to `config.toml` - `/etc/keytle/config.toml` on NixOS
(with `KEYTLE_CONFIG_DIR` pointing there), `$XDG_CONFIG_HOME/keytle/config.toml`
under home-manager. Every key is optional; unknown keys are rejected, so a typo
fails loudly instead of being silently ignored.

</details>

## Usage

| command | what it does |
| --- | --- |
| `keytle`, `keytle run` | start the trainer |
| `keytle stats [--layout L] [--lang L] [--mode M] [--last N]` | dashboard as plain text |
| `keytle doctor` | paths, config, version, `TERM`, and which characters your host board cannot type |
| `keytle export [--format jsonl\|tsv] [-o PATH]` | raw records to stdout or a file |
| `keytle layouts list` | layout ids with names and source |

**While typing:** `esc` abandons the lesson (nothing is recorded), `tab` restarts
it, `q` quits when no lesson is running, and `ctrl-c` always quits. A wrong key
does not advance the cursor until you press the right one, unless `strict` is off.

**On the dashboard** (opened with `s`; `tab` or `esc` returns): `s`/`S` and `l` pick
the layout and language, `a` includes every layout, `m` merges modes, `c` cycles
the chart between speed, accuracy and both.

**In the settings panel** (`,`): `j`/`k` move between rows and `h`/`l` change the
value - layout, host layout, language, mode, emulation, strictness, lesson
length, target speed and automatic next test. An edit applies the moment it is
made, and leaving with `esc` (or `w` while still in it) writes the file, so there
is no unsaved state to lose.

Progression has no key of its own: only a lesson that clears the gate unlocks a
letter.

## Layouts and corpora

Five layouts are compiled in - `qwerty`, `azerty`, `colemak-dh`, `bepo`,
`gallium` - and `keytle layouts list` shows them with their source. A file at
`~/.config/keytle/layouts/<id>.toml` adds your own, or shadows a bundled one:
it is a grid of keys, their shift and altgr legends, and the order its letters
are taught. Word lists work the same way in
`~/.config/keytle/corpora/<lang>/words.txt`.

- [docs/layouts.md](docs/layouts.md) - the layout file, the teaching order, host
  layouts and unreachable characters
- [docs/corpora.md](docs/corpora.md) - the bundled word lists, how to add your
  own, and what each mode drills

## Where data lives

No database: append-only JSONL and small JSON files, so `grep`, `jq` and `git`
work on them.

```
~/.config/keytle/config.toml        settings: layout, host_layout, language, mode,
                                    emulate, strict, lesson_words, target_wpm,
                                    auto_next
~/.config/keytle/layouts/*.toml     your layouts
~/.config/keytle/corpora/<lang>/    your words.txt and sentences.txt
~/.local/share/keytle/tests.jsonl   one record per finished test
~/.local/share/keytle/profiles/     <layout>.<lang>.json - unlocked keys, evidence
```

`KEYTLE_CONFIG_DIR` / `KEYTLE_DATA_DIR` override the roots; otherwise the XDG
variables, then `~/.config` / `~/.local/share`.

## License

MIT. The bundled corpora keep their own licences - FrequencyWords and
Lexique 3.83 (CC-BY-SA 4.0), Tatoeba (CC-BY 2.0 FR) - with provenance in
[`keytle-core/src/data/corpora/NOTICE.md`](keytle-core/src/data/corpora/NOTICE.md).
