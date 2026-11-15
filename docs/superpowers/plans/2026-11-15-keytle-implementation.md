# keytle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fast, offline Rust TUI that teaches keyboard layouts the way keybr does — per-key confidence, unlocked letters, generated practice text — with every test plotted by test number.

**Architecture:** Cargo workspace. `keytle-core` holds layouts, corpora, the typing state machine, the progression engine and the JSONL stats store, all pure and terminal-free. `keytle` is a `ratatui`/`crossterm` binary that renders core state and feeds keystrokes back into it. Layouts and corpora are compiled in (`include_str!`) and overridden by user files at runtime.

**Tech Stack:** Rust (stable, 1.83+), `ratatui`, `crossterm`, `clap` (derive), `serde`/`serde_json`, `toml`, `rand` (`SmallRng`), `thiserror`, `unicode-width`. Dev: `insta` (snapshots). No async runtime, no network, no database.

**Spec:** `docs/superpowers/specs/2026-11-15-keytle-design.md`

## Global Constraints

- Edition 2021. `rust-toolchain.toml` pins channel `1.83`; `flake.lock` and `Cargo.lock` are committed.
- Binary name `keytle`; crates `keytle-core`, `keytle`. License `MIT OR Apache-2.0`.
- Config: `$XDG_CONFIG_HOME/keytle/config.toml`. Data: `$XDG_DATA_HOME/keytle/` (`tests.jsonl`, `profiles/<layout>.<lang>.json`). Both fall back to `~/.config` and `~/.local/share`. `KEYTLE_CONFIG_DIR` / `KEYTLE_DATA_DIR` override for tests.
- No network access at runtime. No accounts. No telemetry. No async.
- Terminal floor 80×24; renderers degrade, never panic.
- Release profile: `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`.
- Modes are `pseudo | words | sentences`; languages `en | fr`; layouts `qwerty | azerty | colemak-dh | bepo | gallium`.
- Strict typing default: wrong key is an error, cursor does not advance, one miss per cursor position (auto-repeat ignored). `lenient` types through.
- Stats X axis is **test index**, never date. Personal bests per (layout, language, mode).
- Confidence: `speed_c = clamp(target_interval_ms / mean_interval_ms, 0, 1)`, `acc_c = clamp((accuracy - 0.90) / 0.08, 0, 1)`, `confidence = 0.6*speed_c + 0.4*acc_c`. Unlock when mean confidence of reachable unlocked keys ≥ 0.7 and no reachable unlocked key has accuracy < 0.93. Max 3 unlocks per session (session resets after 30 idle minutes). Initial unlocked set = first 4 entries of the unlock order.
- `target_interval_ms = 60000 / (5 * target_wpm)`, default `target_wpm = 80`.

## Review Focus

Inputs the spec implies but no feature test exercises. Each has its test in the owning task.

1. **Bracketed paste of a whole paragraph** — a 200-character paste must not register as 200 instant keystrokes inflating WPM; expect it ignored (Task 8).
2. **Terminal resized below 80×24 mid-lesson** — expect a compact render or a "too small" notice, never a panic or a garbled layout (Task 13).
3. **`tests.jsonl` with a truncated final line** (crash mid-append) — expect startup to skip that one line with a warning and keep every other record (Task 12).
4. **Corpus files with CRLF endings, a UTF-8 BOM, or accented French words** — expect them parsed as written, not silently dropped or mojibake (Task 6).
5. **Malformed user layout** (a `base` holding two characters, duplicate positions, an `unlock_order` character absent from the layout) — expect a named error with the offending value, never a panic (Task 3).

---

### Task 1: Workspace, paths, and `keytle doctor`

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `keytle-core/Cargo.toml`, `keytle-core/src/lib.rs`, `keytle-core/src/paths.rs`, `keytle/Cargo.toml`, `keytle/src/main.rs`, `.gitignore`

**Interfaces:**
- Consumes: nothing.
- Produces: `keytle_core::paths::{Paths, paths}` where `Paths { config_dir: PathBuf, data_dir: PathBuf }` and `paths() -> Paths` resolves env override → XDG → `$HOME` fallback. `Paths::config_file() -> PathBuf` (`config_dir/config.toml`), `Paths::tests_file()`, `Paths::profile_file(layout: &str, lang: &str)`.

- [ ] **Step 1: Write the failing test** — `keytle-core/src/paths.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn env_override_wins() {
        std::env::set_var("KEYTLE_DATA_DIR", "/tmp/kd");
        assert_eq!(paths().data_dir, std::path::PathBuf::from("/tmp/kd"));
        assert_eq!(paths().profile_file("colemak-dh", "fr"), std::path::PathBuf::from("/tmp/kd/profiles/colemak-dh.fr.json"));
    }
}
```

- [ ] **Step 2: Run it and watch it fail** — `cargo test -p keytle-core paths` → FAIL (no `paths`).

- [ ] **Step 3: Implement `paths()`** with `std::env::var_os` for `KEYTLE_CONFIG_DIR`/`KEYTLE_DATA_DIR`.
- [ ] **Step 4: Add the CLI** — `clap` derive with `#[command(name = "keytle", version)]` and a `doctor` subcommand printing resolved paths, binary version, and `$TERM`. `cargo run -p keytle -- doctor` must exit 0.
- [ ] **Step 5: Verify** — `cargo test && cargo run -p keytle -- doctor`.
- [ ] **Step 6: Commit** — `git commit -m "feat: workspace skeleton with paths and doctor"`.

### Task 2: Config

**Files:** Create `keytle-core/src/config.rs`; modify `keytle-core/src/lib.rs`, `keytle/src/main.rs` (doctor prints resolved config).

**Interfaces:**
- Consumes: `PathProvider`/`paths()` from Task 1.
- Produces: `Config { layout: String, host_layout: String, language: String, mode: Mode, emulate: bool, strict: bool, lesson_words: usize, target_wpm: f32 }` with `Default` (colemak-dh / qwerty / en / Pseudo / true / true / 20 / 80.0); `Config::load(path: &Path) -> Result<Config, ConfigError>` (missing file = default, unknown fields rejected via `#[serde(deny_unknown_fields)]`); `Config::target_interval_ms(&self) -> f32`; `Mode` enum (serde `rename_all = "lowercase"`).

- [ ] **Step 1: Write failing tests** — defaults; partial file keeps other defaults; `target_wpm = 60.0` → `target_interval_ms() == 200.0`; unknown key `foo = 1` errors mentioning `foo`.
- [ ] **Step 2: Run** — `cargo test -p keytle-core config` → FAIL.
- [ ] **Step 3: Implement** `Config`, `Mode`, `ConfigError` (`thiserror`).
- [ ] **Step 4: Run** — same command → PASS. Then `cargo fmt && cargo clippy -- -D warnings`.
- [ ] **Step 5: Commit** — `feat: config loading with defaults`.

### Task 3: Layout model and TOML parsing

**Files:** Create `keytle-core/src/layout.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct Position { pub row: u8, pub col: u8 }            // derives Ord/Eq/Hash/Copy
pub enum Finger { LeftPinky, LeftRing, LeftMiddle, LeftIndex, Thumb, RightIndex, RightMiddle, RightRing, RightPinky }
pub struct Key { pub pos: Position, pub base: char, pub shift: Option<char>, pub altgr: Option<char>, pub finger: Option<Finger> }
pub struct Layout { pub id: String, pub name: String, pub description: Option<String>, pub rows: u8, pub row_offset: Vec<u8>, pub keys: std::collections::BTreeMap<Position, Key>, pub unlock_order: std::collections::BTreeMap<String, Vec<char>> }
impl Layout {
    pub fn from_toml(src: &str) -> Result<Layout, LayoutError>;
    pub fn key_at(&self, pos: Position) -> Option<&Key>;
    pub fn position_of_base(&self, c: char) -> Option<Position>;   // exact, base or shift or altgr
    pub fn unlock_order_for(&self, lang: &str) -> Vec<char>;       // lang entry, else "default", else []
    pub fn contains_char(&self, c: char) -> bool;
}
```
- `LayoutError` is `thiserror` with variants naming the offending value: `DuplicatePosition{row,col}`, `MultiCharBase{key}`, `UnknownUnlockChar{char}`, `Toml(String)`, `BadGeometry(String)`.

- [ ] **Step 1: Write failing tests** — parse a 3-key inline TOML and assert `key_at`/`position_of_base` round-trip; `from_toml` on duplicate `(1,2)` → `DuplicatePosition`; `base = "ab"` → `MultiCharBase`; `unlock_order.default = ["x"]` on a layout without `x` → `UnknownUnlockChar`; `unlock_order_for("fr")` prefers `fr` over `default`.
- [ ] **Step 2: Run** — `cargo test -p keytle-core layout` → FAIL.
- [ ] **Step 3: Implement** with `toml::from_str` into a `RawLayout` then validate into `Layout` (TOML uses `[[key]]` with `row`, `col`, `base`, `shift`, `altgr`, `finger`, and `[unlock_order]` tables; `Finger` maps from kebab-case strings).
- [ ] **Step 4: Run** → PASS. `cargo fmt && cargo clippy -- -D warnings`.
- [ ] **Step 5: Commit** — `feat: layout model with validated TOML parsing`.

### Task 4: Bundled layouts and user overrides

**Files:** Create `keytle-core/src/layouts.rs`, `keytle-core/src/data/layouts/{qwerty,azerty,colemak-dh,bepo,gallium}.toml`; modify `lib.rs`.

**Interfaces:**
- Produces: `layouts::BUNDLED: &[(&str, &str)]` (`id`, source); `layouts::load(id: &str, user_dir: &Path) -> Result<Layout, LayoutError>` (user file `<user_dir>/<id>.toml` replaces the bundled one); `layouts::available(user_dir: &Path) -> Vec<String>`; `layouts::host_ids() -> &'static [&'static str]` (`["qwerty", "azerty"]`).

- [ ] **Step 1: Write failing tests** — every `BUNDLED` id parses and validates; each of the five reference layouts contains all 26 ASCII letters; `colemak-dh` has `f` at `(3,2)` and `u` at `(2,6)`; `bepo` has `é` on the home row; `available()` lists a user `.toml` not in `BUNDLED`; a user file with id `qwerty` wins over the embedded source.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Author the five layout files** — rows 0–4, `row_offset = [0,1,1,1,2]`, one `[[key]]` per occupied position, `finger` on every letter key, `[unlock_order]` with `default` (home row first: `f j d k s l a ;` for qwerty-family, `q s d f g h j k` style for azerty/bépo as appropriate) and a `fr` override for `azerty`/`bepo`. Include `shift` for every letter and `altgr` where the layout defines it.
- [ ] **Step 4: Implement `load`/`available`** with `include_str!` and a `read_dir` scan of the user dir.
- [ ] **Step 5: Verify** — `cargo test -p keytle-core layouts`, plus `cargo run -p keytle -- layouts list` printing five ids with names.
- [ ] **Step 6: Commit** — `feat: five bundled layouts with user override`.

### Task 5: Emulation and reachability

**Files:** Create `keytle-core/src/emulate.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct Emulator { host: Layout, target: Layout, reachable: BTreeSet<Position> }
impl Emulator {
    pub fn new(host: Layout, target: Layout) -> Emulator;      // computes reachable
    pub fn reachable(&self) -> &BTreeSet<Position>;
    pub fn is_char_reachable(&self, expected: char) -> bool;    // expected char sits on a reachable position
    pub fn translate(&self, typed: char) -> Option<char>;       // host char -> target char at same position
    pub fn host_char_for(&self, expected: char) -> Option<char>;// legend subscript
    pub fn unreachable_chars(&self) -> Vec<char>;
}
```

- [ ] **Step 1: Write failing tests** — hand-checked table: qwerty host → colemak-dh target, `typed 'w'` → `Some('w')`, `typed 'e'` → `Some('f')`, `typed 'j'` → `Some('n')`, `typed 'k'` → `Some('e')`; qwerty → gallium `typed 'd'` → `Some('b')`; qwerty → bepo `typed 'e'` → `Some('é')` (accents reachable by position); azerty host → colemak-dh `typed 'a'` → `Some('q')`; a target char on a position the host lacks is not `is_char_reachable` and is listed by `unreachable_chars()`; `host_char_for('é')` on qwerty→bepo is `Some('e')`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** reachability as "host has a key at the same `(row,col)` with a `base` (or `shift`/`altgr` whose composed char is deliverable)", and `translate` via `host.position_of_base(typed)` → `target.key_at(pos)`.
- [ ] **Step 4: Run** — `cargo test -p keytle-core emulate` → PASS.
- [ ] **Step 5: Commit** — `feat: positional emulation with reachability`.

### Task 6: Corpus parsing, merge and coverage

**Files:** Create `keytle-core/src/corpus.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct Corpus { pub words: Vec<Word>, pub sentences: Vec<String> }   // Word { text: String, freq: u64 }
impl Corpus {
    pub fn parse_words(src: &str) -> Result<Vec<Word>, CorpusError>;
    pub fn parse_sentences(src: &str) -> Result<Vec<String>, CorpusError>;
    pub fn merge(&mut self, other: Corpus);
    pub fn words_covered_by(&self, allowed: &BTreeSet<char>) -> Vec<&Word>;
    pub fn sentences_covered_by(&self, allowed: &BTreeSet<char>) -> Vec<&str>;
    pub fn bigrams(&self) -> BTreeMap<(char, char), u32>;
}
```
- `CorpusError::BadLine { line: usize, reason: String }`.

- [ ] **Step 1: Write failing tests** — `"the 100\ncat 50\n"` parses to two words with frequencies; a line with a combining mark or two spaces in the word → `BadLine` naming the line number; CRLF endings and a leading `\ufeff` parse identically to LF/no-BOM (this also covers Review Focus 4); `"Été à Paris."` survives as one sentence and is *not* covered by ASCII-only `allowed`; `merge` unions words and concatenates sentences; `bigrams()` counts `th`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** with `lines()`, `trim_end_matches('\r')`, BOM strip, `.split_whitespace()`, and `chars().next()`-based coverage (every char of the word must be in `allowed`).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: corpus parsing, merge and coverage filter`.

### Task 7: Bundled corpora

**Files:** Create `keytle-core/src/corpora.rs`, `keytle-core/src/data/corpora/{en,fr}/words.txt`, `.../sentences.txt`, `keytle-core/src/data/corpora/NOTICE.md`; modify `lib.rs`.

**Interfaces:**
- Produces: `corpora::load(lang: &str, user_dir: &Path) -> Result<Corpus, CorpusError>` merging embedded + `<user_dir>/<lang>/words.txt` + `sentences.txt`; `corpora::languages() -> &'static [&'static str]`.

- [ ] **Step 1: Write failing tests** — `load("en")` yields ≥ 5000 words and ≥ 500 sentences; ids are lowercase and ASCII-letter-only for `en`; `load("fr")` contains `é`; a user `fr/words.txt` with one extra word increases the count by one; unknown language → `CorpusError` naming it.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Add the data** — trim each wordlist to ~10k entries with a `head` of a frequency-sorted file, keep sentences short (≤ 80 chars); record source and license per file in `NOTICE.md`.
- [ ] **Step 4: Implement** `load` with `include_str!` + optional user files.
- [ ] **Step 5: Run** → PASS. `cargo clippy -- -D warnings`.
- [ ] **Step 6: Commit** — `feat: bundled en/fr corpora with license provenance`.

### Task 8: Typing state machine

**Files:** Create `keytle-core/src/typing.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub enum Outcome { Hit, Miss { expected: char, typed: char }, Ignored, Unreachable }
pub struct TypingSession { /* text: Vec<char>, cursor, strict, errors_at_cursor, hits, misses, keystrokes, started_ms, last_hit_ms */ }
impl TypingSession {
    pub fn new(text: &str, strict: bool, now_ms: u64) -> TypingSession;
    pub fn apply(&mut self, typed: char, now_ms: u64) -> Outcome;   // time injected: deterministic tests
    pub fn ignore_input(&mut self) -> Outcome;                       // paste / modifier
    pub fn is_complete(&self) -> bool;
    pub fn stats(&self) -> SessionStats;   // net_wpm, raw_wpm, accuracy, duration_ms, per_key: BTreeMap<char, KeyAgg>
}
pub struct KeyAgg { pub hits: u32, pub misses: u32, pub mean_dt: u32 }
```

- [ ] **Step 1: Write failing tests** — strict: correct keys advance, a wrong key returns `Miss` and leaves the cursor; typing the same wrong key twice only counts one miss (`errors_at_cursor` guard); the wrong key then the right one recovers; lenient: a wrong key advances and counts a miss; `ignore_input()` returns `Ignored` and changes nothing (paste, Review Focus 1); `'T'` where `'t'` is expected is a `Miss`; a backwards timestamp (`now_ms` lower than the previous hit) contributes no interval rather than a negative one; 5-letter text typed at 200 ms/char computes `net_wpm` from the formula `(chars/5) / (minutes)`; `is_complete()` only after the last char.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** — advance on `Hit`; in strict mode on `Miss` record but do not advance; intervals accumulate only on first-hit-after-advance and ignore gaps > 2000 ms.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: typing session state machine`.

### Task 9: Per-key statistics and confidence

**Files:** Create `keytle-core/src/engine/mod.rs`, `keytle-core/src/engine/key.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct KeyStat { pub hits: u32, pub misses: u32, pub shift_misses: u32, pub mean_interval_ms: f32 }
impl KeyStat {
    pub fn accuracy(&self) -> f32;                                  // hits/(hits+misses), 1.0 when untouched
    pub fn confidence(&self, target_interval_ms: f32) -> f32;       // formula from Global Constraints
    pub fn record(&mut self, hit: bool, interval_ms: Option<f32>);  // EMA α = 0.1
    pub fn record_shift_miss(&mut self);
}
```

- [ ] **Step 1: Write failing tests** — untouched stat: `accuracy() == 1.0`, `confidence(...) == 0.0` (no speed data); `target_interval_ms = 150.0` with `mean_interval_ms = 150.0` and all hits → `1.0`; `mean_interval_ms = 300.0` → `0.6*0.5 + 0.4*1.0 = 0.7`; accuracy 0.90 → `acc_c == 0.0`; accuracy 0.94 → `0.5`; a single wrong-case key bumps `shift_misses` and leaves `hits`/`misses` alone; EMA: after `record(true, Some(400.0))` then `record(true, Some(100.0))`, `mean_interval_ms == 370.0`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the two formulas exactly as written in the spec.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: per-key confidence model`.

### Task 10: Profile, unlock rule, session cap

**Files:** Create `keytle-core/src/engine/profile.rs`; modify `engine/mod.rs`, `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct UnlockEvent { pub char: char, pub ts_ms: u64, pub how: UnlockHow }   // UnlockHow { Auto, Manual, Lock }
pub struct Profile { pub layout: String, pub lang: String, pub unlocked: BTreeSet<char>, pub keys: BTreeMap<char, KeyStat>, pub lesson_index: u32, pub tests_completed: u32, pub unlocks_this_session: u8, pub session_started_ms: u64, pub last_unlock: Option<UnlockEvent> }
pub enum Verdict { Unlock(char), Blocked(BlockReason) }        // BlockReason::{NoNextKey, SessionCap, LowConfidence{f32}, Accuracy{char,f32}}
impl Profile {
    pub fn new(layout: &Layout, lang: &str, now_ms: u64) -> Profile;   // first 4 unlock-order chars
    pub fn load(path: &Path) -> Result<Profile, ProfileError>;
    pub fn save(&self, path: &Path) -> std::io::Result<()>;
    pub fn mean_confidence(&self, reachable: &BTreeSet<char>, target_interval_ms: f32) -> Option<f32>;
    pub fn verdict(&self, layout: &Layout, lang: &str, reachable: &BTreeSet<char>, target_interval_ms: f32) -> Verdict;
    pub fn apply(&mut self, verdict: Verdict, now_ms: u64) -> Option<UnlockEvent>;
    pub fn absorb(&mut self, stats: &SessionStats);               // fold a finished lesson in
    pub fn tick_session(&mut self, now_ms: u64);                  // reset cap after 30 idle minutes
}
```

- [ ] **Step 1: Write failing tests** — new profile has 4 unlocked letters and confidence gate `Blocked(LowConfidence)`; **deadlock regression**: a layout where two unlocked characters are unreachable and have zero stats still reaches `Unlock` once the reachable characters are strong; accuracy 0.90 on one unlocked key → `Blocked(Accuracy{..})`; three `apply(Unlock(..))` all consume the cap and the fourth returns `Blocked(SessionCap)`; `tick_session` 31 minutes later restores the cap and bumps `session_started_ms`; save→load round-trips `unlocked`, `keys`, `last_unlock`; `absorb` adds hits/misses and increments `tests_completed`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** with `serde` JSON; creates the parent dir on save.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: profile with unlock rule and session cap`.

### Task 11: Lesson generator

**Files:** Create `keytle-core/src/engine/generate.rs`; modify `engine/mod.rs`, `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct Lesson { pub text: String, pub words: usize, pub note: Option<Note> }   // Note::{FallbackToPseudo, FullAlphabetSentences, SentencesDropped{dropped, total}}
pub fn generate(layout: &Layout, corpus: &Corpus, lang: &str, unlocked: &BTreeSet<char>, reachable: &BTreeSet<char>, mode: Mode, words: usize, rng: &mut impl rand::Rng) -> Lesson;
```

- [ ] **Step 1: Write failing tests** (all with `SmallRng::seed_from_u64`) — 4 unlocked letters produce CVC/VCVC template words, never a letter outside the set, and at least 3 distinct letters appear over 40 words; 8 unlocked letters use the mixed model: every word inside the set, no letter three times consecutively; 14 unlocked letters: no word outside the set; `words` mode returns only real corpus words fully covered by the set, with `note: None`, and falls back to pseudo with `Note::FallbackToPseudo` when fewer than 50 candidates exist; `sentences` mode returns 3 sentences joined by a space with `Note::FullAlphabetSentences` when corpora coverage is below 20; same seed → identical `Lesson` text (determinism); reachable-only: a word containing an unreachable char never appears.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the three-tier backoff ladder, weights `w = 0.15 + (1 - confidence)` passed in as `&BTreeMap<char, KeyStat>`, word length 3–8, and the pseudo/real/sentence selection rules.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: lesson generation with tiny-alphabet backoff`.

### Task 12: Stats store and aggregation

**Files:** Create `keytle-core/src/stats.rs`; modify `lib.rs`.

**Interfaces:**
- Produces:
```rust
pub struct TestRecord { pub v: u8, pub ts: String, pub layout: String, pub host: String, pub emulated: bool, pub lang: String, pub mode: Mode, pub lesson: u32, pub wpm: f32, pub raw_wpm: f32, pub accuracy: f32, pub duration_ms: u64, pub keys: BTreeMap<char, KeyAgg>, pub unlocked_before: u32, pub unlocked_after: u32 }
pub fn append(path: &Path, rec: &TestRecord) -> std::io::Result<()>;
pub fn read_all(path: &Path) -> Result<Vec<TestRecord>, StatsError>;   // skips a truncated last line, warns to stderr
pub struct Filter { pub layout: Option<String>, pub lang: Option<String>, pub mode: Option<Mode> }
pub struct Point { pub index: u32, pub wpm: f32, pub raw_wpm: f32, pub accuracy: f32 }
pub fn series(records: &[TestRecord], filter: &Filter) -> Vec<Point>;   // index = 1..n, no date bucketing
pub fn personal_best(records: &[TestRecord], filter: &Filter) -> Option<f32>;
pub fn consistency(wpms: &[f32]) -> f32;                                 // (1 - cv).clamp(0,1) * 5.0
pub fn key_rows(records: &[TestRecord], filter: &Filter, last: usize, target_interval_ms: f32) -> Vec<KeyRow>; // KeyRow { ch, accuracy, confidence, trend: Trend, samples }
pub enum Trend { Up, Flat, Down }
```

- [ ] **Step 1: Write failing tests** — append then `read_all` round-trips two records; a hand-written file whose last line is half a JSON object returns the first record and an error-free `Ok` (Review Focus 3); `series` on 3 records with timestamps spanning one day and one day later returns indices `[1,2,3]` with no gaps; `series` filters by mode; `personal_best` is per-filter (sentences PB ≠ pseudo PB); `consistency` of identical WPMs is `5.0`; `key_rows` marks a key whose second-half mean interval improved as `Trend::Up`; `key_rows(last = 20)` reads only the last 20 matching records.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** — one `serde_json` line per record, `OpenOptions::append`, `BufWriter` read loop with a per-line `serde_json::from_str` and a `continue` on the final line only.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: JSONL stats store and test-index series`.

### Task 13: TUI shell, event loop, resize safety

**Files:** Create `keytle/src/app.rs`, `keytle/src/ui/mod.rs`, `keytle/src/ui/typing.rs`; modify `keytle/src/main.rs`.

**Interfaces:**
- Consumes: `Config`, `Layout`, `Profile`, `TypingSession`, `Emulator`.
- Produces: `App { /* config, layout, host, profile, session: Option<TypingSession>, screen: Screen, notice: Option<String> */ }`, `Screen { Typing, Stats, Settings, Help, Results }`; `App::handle_key(KeyEvent) -> Action` (`Action::{Redraw, Quit, StartLesson, Finish}`) — pure, no terminal I/O; `ui::typing::render(frame: &mut Frame, app: &App)`.

- [ ] **Step 1: Write failing tests** — `App::handle_key` on `Esc` mid-lesson aborts without finishing; `q` on the typing screen quits; `render` into `ratatui::backend::TestBackend` at 80×24 and 120×40 produces frames with the lesson text; at 40×12 it renders a "terminal too small (80×24 minimum)" notice and does not panic (Review Focus 2); `insta` snapshot of the 80×24 frame.
- [ ] **Step 2: Run** — `cargo test -p keytle` → FAIL.
- [ ] **Step 3: Implement** the loop: `crossterm::event::poll(16ms)` → `read` → `app.handle_key`; `Event::Resize` just redraws; `Event::Paste` and non-`Char` keys call `session.ignore_input()`.
- [ ] **Step 4: Run** → PASS, then `cargo run -p keytle` shows a static lesson.
- [ ] **Step 5: Commit** — `feat: TUI shell with resize-safe rendering`.

### Task 14: Live typing view and keyboard

**Files:** Modify `keytle/src/ui/typing.rs`; create `keytle/src/ui/keyboard.rs`.

**Interfaces:**
- Produces: `ui::keyboard::render(frame: &mut Frame, area: Rect, layout: &Layout, next: Option<char>, emulator: Option<&Emulator>, reachable: &BTreeSet<Position>)`; typing view shows elapsed, net/raw WPM, accuracy, the text with typed/current/upcoming styling, and the status notice line.

- [ ] **Step 1: Write failing tests** — keyboard rendered for `colemak-dh` at 80 columns contains `n` and highlights exactly one key cell for `next = Some('e')`; with `emulate = true` on a qwerty host the `é` cell contains both `é` and `e` (subscript); unreachable cells render with `·`; typing view after 3 hits shows `100%` accuracy and a nonzero WPM; after one miss the offending character renders with the error style and accuracy drops below 100%.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** geometry from `row_offset` (half-key stagger) using `unicode_width` for cell math.
- [ ] **Step 4: Run** → PASS + `insta` snapshots updated.
- [ ] **Step 5: Commit** — `feat: live typing view with keyboard and host legend`.

### Task 15: Results screen and stats dashboard

**Files:** Create `keytle/src/ui/stats.rs`, `keytle/src/ui/results.rs`; modify `keytle/src/app.rs`, `ui/mod.rs`.

**Interfaces:**
- Consumes: `stats::{series, personal_best, consistency, key_rows}`, `engine::Profile`.
- Produces: `App::finish_lesson()` writing a `TestRecord` via `stats::append`, absorbing stats into the profile, and setting `Screen::Results`; stats view keys: `s`/`S` cycle layout, `l` cycle language, `m` merge modes, `a` all layouts, `tab` back.

- [ ] **Step 1: Write failing tests** — finishing a lesson appends exactly one JSONL line and leaves `Screen::Results`; results screen shows net WPM, raw WPM, accuracy, and the 3 weakest keys; stats view over a fixture file draws `tests 10 … 20 … 30` ticks (X axis is test count, not dates) and shows the PB and consistency; pressing `m` merges modes and marks the header `(merged)`; empty stats file renders an "no tests yet" state without panic.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** charts with block glyphs (`▁▂▃▄▅▆▇█`) for the series, a 2-row braille/block line chart for speed, and the key table sorted by weakness.
- [ ] **Step 4: Run** → PASS + snapshots.
- [ ] **Step 5: Commit** — `feat: results and stats dashboard by test index`.

### Task 16: Settings, help, and in-app switching

**Files:** Create `keytle/src/ui/settings.rs`, `keytle/src/ui/help.rs`; modify `keytle/src/app.rs`, `ui/mod.rs`.

**Interfaces:**
- Produces: settings screen editing `Config` in memory (layout, host, language, mode, emulate, strict, lesson_words, target_wpm) with `w`/`s` to save to `config.toml`; help screen listing keys; `App::switch_layout(id)` reloading the profile for the new (layout, language) pair.

- [ ] **Step 1: Write failing tests** — toggling `emulate` and saving writes the file and reloads to the same value; `switch_layout("bepo")` loads `bepo.fr.json` when language is `fr` and does not carry over colemak unlocks; help screen lists `esc`, `tab`, `u`, `l`, `s`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: settings, help and in-app layout switching`.

### Task 17: CLI subcommands and end-to-end smoke test

**Files:** Modify `keytle/src/main.rs`; create `keytle/tests/cli.rs`.

**Interfaces:**
- Produces: `keytle run` (default), `keytle stats [--layout L] [--lang L] [--mode M] [--last N]` printing a text dashboard to stdout, `keytle export [--format jsonl|tsv] [-o PATH]`, `keytle layouts list`, `keytle doctor` (paths, resolved config, corpus counts, reachability report listing unreachable chars and the reason).

- [ ] **Step 1: Write failing tests** — `assert_cmd`-free version using `std::process::Command::new(env!("CARGO_BIN_EXE_keytle"))`: `doctor` exit 0 and stdout contains `reachable` and the data dir; `stats` with an empty `KEYTLE_DATA_DIR` exits 0 and prints `no tests yet`; `export --format tsv` on a two-record fixture prints a header line plus two rows.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: stats, export, layouts and doctor subcommands`.

### Task 18: Nix flake, modules and checks

**Files:** Create `flake.nix`, `nix/module.nix`, `nix/hm-module.nix`, `.github/workflows/ci.yml`; modify `Cargo.toml` (release profile).

**Interfaces:**
- Produces: `packages.<system>.default` and `.keytle`, `overlays.default`, `devShells.<system>.default`, `checks.<system>.{fmt,clippy,test}`, `apps.<system>.default`, `nixosModules.default`, `homeModules.default`.

- [ ] **Step 1: Write the flake** using `rustPlatform.buildRustPackage` with `cargoLock.lockFile = ./Cargo.lock` and no runtime data deps.
- [ ] **Step 2: Verify locally** — `nix build .#` then `./result/bin/keytle doctor`; `nix develop -c cargo test`; `nix flake check`.
- [ ] **Step 3: Add the modules** — NixOS module takes `enable`, `package`, `settings` (rendered to `config.toml` via `pkgs.formats.toml`); the HM module does the same with `xdg.configFile`.
- [ ] **Step 4: CI** — `nix flake check` plus `nix build`, on `x86_64-linux`.
- [ ] **Step 5: Commit** — `feat: nix flake, nixos and home-manager modules`.

---

## Self-Review

- **Spec coverage:** intent/goals → Tasks 1–18; layouts & extensibility → 3–4; emulation & reachability → 5; corpora & languages → 6–7; progression → 9–11; stats by test index → 12, 15; config → 2, 16; Nix first-class → 18; performance budget → release profile (18) plus no-alloc-per-keystroke in 8; testing matrix → each task's tests. Deferred by spec and therefore absent here: `export --html`, `--by-date`.
- **Types cross-checked:** `Position`, `Key`, `Layout`, `Emulator`, `TypingSession::apply`, `SessionStats`, `KeyAgg`, `KeyStat`, `Profile`, `Verdict`, `TestRecord`, `Filter`, `Point`, `key_rows`, `generate` — each defined once and used with the same signature in later tasks.
- **Review Focus** lines 1–5 are pinned in Tasks 8, 13, 12, 6, 3 respectively.
- **Proportion:** 18 tasks; no task body is transcribed where a signature plus test decides it.
