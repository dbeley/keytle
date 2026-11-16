# keytle — a terminal keyboard-layout trainer

**Status:** design, awaiting approval
**Date:** 2026-11-15
**Name:** `keytle` (chosen). Crate, binary, config and data paths all use `keytle`.

## Intent

Learn and switch keyboard layouts from the terminal, offline, without an account,
fast enough to feel instant.

The reference point is [keybr](https://keybr.com): its teaching method — start on a
few home-row letters, generate practice text from the letters you have unlocked,
introduce new letters as your speed and accuracy on the current set improve — works
well. What it gets wrong for this author: it is heavy for a browser page, and its UI
is busy. This tool keeps the method, drops the weight, and gives the stats the
respect they deserve.

## Why now / what success looks like

The author wants to learn Colemak-DH (and try bépo, gallium, azerty) without
installing every layout system-wide, without a browser, and wants to see *evidence of
improvement* — per key, per test — rather than a vague daily average.

Success: open a terminal, run one command, start typing within two seconds; after a
week of short sessions the stats view answers "which keys are holding me back, and am
I actually faster than last Tuesday?" without leaving the app.

## Goals

- Train five bundled layouts: `qwerty`, `azerty`, `colemak-dh`, `bepo`, `gallium`.
- Optional **emulation**: type on your existing (host) keyboard, app treats a key's
  *physical position* as the target layout's character at that position.
- Progression engine: per-key confidence in speed and accuracy, letters unlocked as
  the learner earns them, practice text generated from the unlocked set.
- French and English. Three text modes: generated pseudo-words (default), real words,
  complete sentences.
- Rich local stats: every test is a data point; trends by test index; per-key
  accuracy/speed with trend; personal bests; consistency.
- Layouts and corpora are data, not code: embed defaults, allow user-provided files.
- First-class NixOS: flake with package, overlay, dev shell, home-manager module.
- Single static-ish binary, no network, no daemon, no runtime assets required.
- Open source (MIT OR Apache-2.0), no accounts, no telemetry.

## Non-goals

- GUI, web frontend, mobile.
- Accounts, sync, leaderboards, sharing.
- Multi-user profiles (one local learner per data directory; override via env var).
- Characters the host board cannot express at a given position (keys absent from the
  host's physical board, or modifiers the terminal swallows). We name them in the UI
  and in `doctor` instead of failing silently.
- Gamification beyond progress metrics: no streaks, badges, or daily-goal nagging.
- Emulating layouts for *other people's* hardware (thumb clusters, split boards).

## Anti-goals (what would make this a failure even if it works)

- Startup or per-keystroke lag noticeable while typing.
- Requiring a config file edit before the first lesson.
- Stats that only show per-day aggregates, hiding 99 tests done in one sitting.
- A data format you cannot `grep` or export.
- A layout/corpus format that requires recompiling the binary.
- Silent wrong-key handling: the learner must always know whether an input was
  rejected, ignored as unsupported, or accepted.

## Constraints

- **Terminals deliver characters, not keycodes.** The OS layout translates input
  before the app sees it. Emulation therefore assumes a known *host layout* and maps
  each received character back to a physical position, then reads the target
  layout's character at that position. Because the join is the *position*, accented
  and AltGr characters work as long as their target position exists on the host
  board (`é` on bépo is typed by pressing the key where bépo puts it, which a QWERTY
  host reports as some ASCII character). What emulation truly cannot reach: gaps in
  the host's physical board and modifiers the terminal intercepts. See "Reachability".
- Minimum terminal size 80×24; the keyboard display and stats degrade gracefully
  (compact keyboard, fewer chart rows) rather than refusing to run.
- Must build and test on Linux; should compile on macOS and Windows (crossterm is
  cross-platform; no `/dev/input`, no platform-specific code).

## Approaches considered

### 1. TUI stack

- **ratatui + crossterm** — picked. Small dependency tree, immediate-mode rendering
  with a `TestBackend` that makes snapshot tests trivial, mature and maintained.
- **cursive** — callback-based, ergonomic for dialogs, but heavier to test and its
  immediate-mode stories are clumsier for a redraw-every-keystroke screen.
- **egui + wgpu GUI** — would make true keycode-level emulation possible, but it is a
  window, not a terminal tool, and defeats "light and fast" on the author's terms.

### 2. Stats presentation

- **A. In-terminal dashboard** — picked. Charts drawn with block/braille glyphs,
  per-key tables, no charting dependency, always available while typing.
- B. Static HTML/SVG export — prettier, shareable, but leaves the terminal and adds a
  charting dependency; also invites a second, divergent renderer.
- C. A plus a later `export --html` — kept as an explicitly deferred option: the
  JSONL store is designed so this needs no schema change.

### 3. Stats storage

- **Append-only JSONL** (`tests.jsonl`) — picked. Volume is tiny (order of a few MB
  per year of daily use), greppable, trivially exportable, no C dependency.
- **SQLite (rusqlite, bundled)** — bundles SQLite's C source for query power we do not
  need at this volume, and makes the file opaque to `grep`/`jq`.

### 4. Code structure

- **Workspace: `keytle-core` + `keytle` (bin)** — picked. The engine, layouts,
  corpora and stats are testable without a terminal, and a future frontend (web, GUI,
  another TUI) can reuse them without reimplementing the pedagogy.
- Single binary crate — simpler today, but every engine test drags in rendering, and
  the "extensible" goal implies a public, documented core.

### 5. Progression model

- **Per-key confidence, keybr-like** — picked, see below. Explicit, tunable, easy to
  explain in the UI ("`r` needs 12 wpm more").
- Fixed lesson ladder (hand-authored "lesson 1 = asdf, lesson 2 = ...") — predictable
  but rigid, layout-specific and language-specific busywork.
- Global WPM threshold unlock — trivial, but unlocks steadily *wrong* keys and hides
  which key is the problem.

## Design

### Crate layout

```
keytle-core/        library
  layout.rs          TOML layout model, physical positions, emulation translation
  corpus.rs          embedded + user corpora, word/sentence sources
  engine.rs          lesson generation, per-keystroke metrics, confidence, unlocks
  stats.rs           JSONL records, aggregation, series, personal bests
  config.rs          config loading/merging
  data/              embedded layouts + corpora (include_str!)
keytle/             binary
  main.rs            clap CLI: run (default), stats, export, layouts list, doctor
  app.rs             state machine, event loop
  ui/                typing view, stats view, settings, help, results
flake.nix            package, overlay, devShell, checks, home-manager module
```

### Layout definition (TOML, user-extensible)

Physical positions are `(row, col)` on a 5-row ANSI-style board, so a layout is a
mapping *from positions to output*, not a picture. This is what makes emulation and
user-defined layouts the same mechanism.

```toml
id = "colemak-dh"
name = "Colemak-DH"
description = "Colemak with the DH mod (moved D/H to the home row)"

# Board geometry. Rows: 0 = numbers, 1 = top, 2 = home, 3 = bottom, 4 = space row.
# Columns are 0-based within a row. row_offset is the visual stagger in half-keys.
rows = 5
row_offset = [0, 1, 1, 1, 2]

[[key]]
row = 1; col = 2
base = "w"; shift = "W"; finger = "left-ring"

[[key]]
row = 3; col = 2
base = "f"; shift = "F"; finger = "left-index"

[[key]]
row = 2; col = 3
base = "r"; shift = "R"; altgr = "®"; finger = "left-index"

# ... one entry per occupied position. A position with no entry is an absent key:
# drawn blank by the renderer and unreachable in emulation.

[unlock_order]
default = ["f", "j", "d", "k", "s", "l", "a", ";", "g", "h", "t", "n"]
fr      = ["f", "j", "d", "k", "s", "l", "q", "m", "g", "h", "t", "n"]  # optional override
```

Validation on load: no duplicate `(row, col)`; `base` present on every key; every
character in `unlock_order` exists somewhere in the layout; `id` unique across bundled
and user layouts (a user file with a bundled `id` replaces it, and says so).

The on-screen keyboard is rendered *from physical positions*, so every layout draws in
the same shape and only the legends differ. Bépo's extra keys and AZERTY's swapped
`A/Q`, `Z/W`, `M` are data, not special cases.

Host layouts are ordinary layout definitions too (`qwerty`, `azerty` bundled; the
config names the host). Emulation:

```
typed char c (as rendered by the host)
  → find key k_h in host layout with base/shift == c
  → read key k_t in target layout at (k_h.row, k_h.col)
  → expected char = k_t.base (or k_t.shift depending on case)
```

If no key in the host layout produces `c`, that input cannot be attributed to a
position and is **ignored** (never counted as an error): the status bar says once per
lesson that this character is not reachable from the current host layout.

**Reachability** is computed at load for the pair (host, target) and is the single
source of truth for "can this layout be practised as configured":

- A target key is reachable iff the host layout has *some* key at the same
  `(row, col)` that produces at least one character, and the target character sits on
  `base`/`shift` or on a modifier combination the terminal is known to deliver. Dead
  keys on the host are handled by mapping the composed character back to its key.
- Unreachable keys are drawn greyed out with a `·` legend, excluded from lesson
  generation, excluded from the mean confidence in the unlock rule (so they cannot
  deadlock progression), and listed by `keytle doctor` with the reason.
- With the five bundled layouts and either bundled host (`qwerty`, `azerty`) the only
  unreachable characters are the rare ones on positions the host board lacks. `doctor`
  prints the count; it is normally zero.

In emulation mode the keyboard legend shows the target character with the host key it
lives on as a small subscript (`é` over `e` on a QWERTY host), so the learner always
knows what to press.

### Corpus format

```
corpora/en/words.txt       "the 23135851162"   # word + frequency, whitespace-separated
corpora/en/sentences.txt   one sentence per line
corpora/fr/words.txt
corpora/fr/sentences.txt
```

Embedded defaults: word frequency lists trimmed to ~10k words per language, and a few
thousand short sentences, chosen from public-domain / permissively licensed sources and
recorded in `NOTICE`/`data/README.md` with license provenance. User corpora live in
`$XDG_CONFIG_HOME/keytle/corpora/<lang>/…` and are merged with the embedded ones
(union for words, concat for sentences). Corpus files are validated at load with a
clear error (line number) rather than silently skipped.

**Coverage filter.** Every text candidate (generated word, real word, sentence) is
checked against *unlocked ∩ reachable*. Words that fail are dropped silently (they are
the majority early on, by design). Sentences that fail are dropped with a count shown in
the status bar (`38 of 412 sentences usable with 8 keys unlocked`), and if fewer than 20
remain, sentence mode offers "practice with the full alphabet" as an explicit choice
rather than switching silently.

### Progression engine

State is keyed by **(layout, language)** and persisted to
`$XDG_DATA_HOME/keytle/profiles/<layout>.<lang>.json`. Host layout is *not* part of the
key: it only changes reachability, which is recomputed each run. Language belongs to the
practice text and the profile, never to the layout file, so "French on Colemak-DH" and
"English on Colemak-DH" are two independent progressions.

- `unlocked: set<char>`
- `keys: { char: KeyStat { hits, misses, shift_misses, interval_ms: Ema, confidence } }`
- `lesson_index`, `tests_completed`
- `last_unlock: { char, ts, reason } | null`, `session_started_at`, `unlocks_this_session`

Unreachable characters stay in the map but are excluded from every average.

On case: comparison is exact — `T` where `t` is expected is an error. Stats normalise to
the lowercase letter (`T` and `t` feed one `KeyStat`), and a wrong case increments
`shift_misses` instead of being blamed on another key. Capitals are never an unlock;
shift is available from lesson one, which sentence mode needs anyway.

Session: one process run. The three-unlock cap resets after 30 idle minutes, and each
reset is recorded in the profile so the stats view can draw a session break on the X
axis.

Per keystroke: `hits`/`misses` update immediately; `interval_ms` is the EMA (α≈0.1) of
the time between consecutive correct keystrokes, ignoring gaps > 2 s (thinking pauses).

Confidence per key:

```
speed_c   = clamp(target_interval / interval_ms, 0, 1)      # target ≈ 150 ms ≈ 80 wpm
acc_c     = clamp((hits / (hits + misses) - 0.90) / 0.08, 0, 1)
confidence = 0.6 * speed_c + 0.4 * acc_c
```

Unlock rule, evaluated at the end of a completed lesson: if the mean confidence of the
*reachable* unlocked set ≥ 0.7 **and** no reachable unlocked key has accuracy < 0.93,
unlock the next letter from the layout's unlock order, then the punctuation the current
language needs (`'`, `,`, `.`, `;`, `:`, `!`, `?`, and for French `é à è ç ù ê î ô û`, in
that order, skipping any the host cannot reach). One letter per qualifying lesson, max
three per session, so progress is felt but not dumped. There is no manual override: the
rule is the only route to an unlock, so a profile cannot hold a letter the lessons did
not earn. (An earlier draft had `u` unlock and `l` pin; both bindings were removed.)

Unlock order ships with the layout TOML as a `[unlock_order]` table with a `default`
list and optional per-language overrides, so a user-added layout works without touching
the engine.

Lesson generation, three modes:

- **pseudo** (default, keybr-like): random words of length 3–8 built from a letter
  bigram model derived at load time from the corpus wordlist, restricted to *unlocked ∩
  reachable*, with per-letter inclusion weight `w = 0.15 + (1 - confidence)`, resampled
  to avoid the same letter three times in a row. **Backoff ladder** for tiny alphabets,
  because real bigram mass vanishes there and naive generation produces `fjfjfj`:
  1. fewer than 6 unlocked letters → vowel/consonant templates (`CVC`, `VCVC`, `CVCVC`),
     vowels taken as `aeiouy` plus accented vowels present; generated sequentially, not
     sampled from a model;
  2. 6–10 unlocked letters → bigram model mixed with a uniform floor (`p = 0.85·bigram +
     0.15·uniform`) so unseen transitions stay possible;
  3. more than 10 → the bigram model as-is.
     The letter set used by each word is drawn with weight `w`, so weak keys appear more
     often without ever leaving the unlocked set.
- **words**: real corpus words that are fully covered by the unlocked set and scored by
  frequency × weakness weighting; if fewer than 50 candidates exist, fall back to pseudo
  and say so in the status bar.
- **sentences**: real sentences fully covered by the unlocked set; early on this is
  almost never satisfiable, so sentences use the *full* alphabet and are offered as a
  separate practice mode with a hint that unlocks do not apply there.

Lesson length: 20 words (pseudo/words) or 3 sentences, configurable. A lesson ends when
the text is consumed.

### Typing rules

Default **strict**: a wrong character is counted as an error (red), the cursor does not
advance, and the correct key must be typed before continuing — keybr's feel. `lenient`
config flag types through mistakes. `esc` aborts (test not recorded), `tab` restarts the
current lesson, `ctrl-c` quits.

A held-down wrong key counts **one** miss per cursor position, not one per auto-repeat
event: while the cursor has not moved, repeats of the same wrong character are ignored
after the first. Otherwise accuracy and the per-key `hits/misses` ratio become noise.
Modifiers (`caps lock`, `shift` alone, `ctrl`+letter, function keys) are ignored and
never counted as errors.

Live display: elapsed time, WPM (net), raw WPM, accuracy, the remaining text with the
current word in focus, and the keyboard showing the next key to press highlighted plus
the finger to use. Emulated keys carry the host-subscript.

### Test records (JSONL, one per completed lesson)

```json
{"v":1,"ts":"2026-11-15T21:04:11Z","layout":"colemak-dh","host":"qwerty",
 "emulated":true,"lang":"en","mode":"pseudo","lesson":14,
 "wpm":42.1,"raw_wpm":46.8,"accuracy":0.961,"duration_ms":38210,
 "keys":{"n":{"hits":31,"misses":0,"mean_dt":142},
         "r":{"hits":18,"misses":3,"mean_dt":233}},
 "unlocked_before":12,"unlocked_after":13}
```

`v` allows future migration; `keys` holds per-test per-key aggregates, which is what
makes per-key trends possible without storing every keystroke. Records are appended,
never rewritten; the file is read fully at startup (a year of daily use is a few MB,
well under a millisecond to parse at this size; if a user ever exceeds ~50 MB we add a
compacted summary file — noted as a shortcut, not built now).

### Stats view (approach A, by test index — not by date)

X axis is **test number**, every test is a point. One hundred tests in one day produce
one hundred points; a day with no practice produces no gap. Smoothing is display-only
(toggled moving average, never replacing the raw points).

```
┌ Progress ─ colemak-dh ───────────────── 42 tests · 3h12m ─────┐
│ 70 ┤                                              ╭─╮         │
│ 55 ┤                                   ╭──────────╯ ╰╮        │
│ 40 ┤                     ╭─────────────╯              ╰─       │
│ 25 ┤       ╭───────╮─────╯                                    │
│ 10 ┤───╮───╯                                                  │
│    └──────────────────────────────────────────────────────    │
│      tests 10     20     30     40     42                     │
│ acc 96.1% ▲2.3     consistency 4.2/5    pb 71 wpm             │
├ Keys ─────────────────────────────────────────────────────────┤
│ n 92% 4.1/5 ▲ │ e 88% 3.9/5 ▲ │ r 71% 2.8/5 ▼  ◀ weakest     │
│ t 95% 4.6/5   │ a 90% 4.2/5   │ s 84% 3.5/5                   │
├ Next unlock ──────────────────────────────────────────────────┤
│ g — needs 0.72 confidence, you are at 0.61 (accuracy ok)      │
└───────────────────────────────────────────────────────────────┘
```

Elements:

1. **Speed chart** by test index: net WPM per test (dots), optional moving average,
   personal best line, unlock events marked on the axis.
2. **Accuracy chart** toggle (same X axis), plus consistency (1 − coefficient of
   variation of per-test WPM, keybr's notion, shown as x/5).
3. **Per-key table**: accuracy, confidence, speed, trend arrow (compare the first half
   of the last 20 tests against the last half), sorted by weakness. Keys are coloured
   by whether they are unlocked or pending, so the table doubles as the unlock map.
4. **Unlock panel**: next letter, what it needs, and why it isn't unlocked yet — speed
   or accuracy — so the learner knows how to get there.
5. **Filter**: series are keyed by **(layout, language, mode)**. The chart shows the
   current mode by default; `m` merges modes into one line (marked as merged);
   `s`/`S` cycles layout and language; `a` toggles across all layouts for a single
   comparison line. `emulated` is recorded but is never a default filter: the physical
   finger movement is identical, so mixing them is honest. Personal bests are kept per
   series, never globally, because sentence mode is not comparable to pseudo-words.

No daily aggregation anywhere. Time *is* available in records, so a `--by-date` flag can
be added later without a schema change, but it is not part of this design.

### Config

`$XDG_CONFIG_HOME/keytle/config.toml`, all fields optional with working defaults:

```toml
layout = "colemak-dh"
host_layout = "qwerty"
language = "en"
mode = "pseudo"       # pseudo | words | sentences
emulate = true
strict = true
lesson_words = 20
target_wpm = 80
```

Precedence: CLI flags > config file > defaults. `keytle doctor` prints resolved
config, data paths, corpus counts, and whether the current layout is emulable on the
configured host (listing unsupported characters).

### NixOS support (first-class)

```
flake.nix
  packages.<system>.default      # buildRustPackage, cargoLock
  packages.<system>.keytle
  overlays.default               # for nixpkgs overlays
  devShells.<system>.default     # rustc, cargo, clippy, rust-analyzer, rustfmt
  checks.<system>                # cargo fmt --check, clippy -D warnings, nextest
  apps.<system>.default          # `nix run`
  nixosModules.default           # installs the package, sets XDG paths
  homeModules.default            # same for home-manager users
```

Because corpora and layouts are `include_str!`-embedded, the derivation has no runtime
data dependency and the sandbox build is a plain `buildRustPackage`. Pinned toolchain
via `rust-toolchain.toml`; `flake.lock` committed. `nix flake check` is the CI gate.

### Testing

- `layout`: round-trip parse; no duplicate `(row, col)`; every `unlock_order` character
  exists in the layout. Emulation qwerty→colemak-dh, qwerty→gallium and qwerty→bépo
  against hand-checked tables; azerty host; accents reached by position; unreachable-key
  detection; case handling.
- `corpus`: merge of embedded + user files; malformed file reports a line number;
  coverage filtering by unlocked ∩ reachable; sentence-count reporting.
- `engine`: confidence formula at boundaries; unlock gating (accuracy floor, mean
  confidence); unreachable keys cannot hold the mean down (regression test for the
  deadlock); max three unlocks per session; the backoff ladder at 4, 8 and 14 unlocked
  letters over a seeded RNG (no `fjfj` degeneracy); sentences fall back correctly.
- `stats`: JSONL append/read round-trip; series indexed by test number with unequal time
  gaps; per-key trend from two halves; personal best; consistency.
- `ui`: `ratatui::TestBackend` snapshots for typing view at 80×24 and 120×40, stats
  view, and the emulation subscript rendering.

### Performance budget

Startup < 30 ms including corpus parse; no allocation per keystroke outside the text
buffer; rendering under 1 ms/frame at 80×24; release profile with `lto = "thin"`,
`codegen-units = 1`, `panic = "abort"` optional.

## Decisions left to the builder

- Exact EMA coefficient, confidence weights and unlock thresholds — the formulas above
  are the starting point; tune against a scripted "learner" fixture and note changes in
  the code.
- Chart glyph strategy (braille vs block) and colour palette, including a
  colour-blind-safe variant. Whether the accuracy chart overlays or toggles.
- Which public-domain corpora to bundle and how aggressively to trim them.
- Whether `keytle export --html` ships in v1 (deferred by default; `export` in v1 writes
  the raw JSONL, or TSV with `--format tsv`, to stdout or a file).
- Exact rules for sticky behaviours: whether `tab` (restart) also resets the current
  lesson without recording, and how the 30-minute idle reset is detected when the process
  is suspended rather than exited.
