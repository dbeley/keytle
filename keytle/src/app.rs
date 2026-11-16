//! The TUI state machine: what the app knows and what a key means to it.
//!
//! Everything here is terminal-free. [`App::handle_key`] takes a key event and a
//! timestamp and returns an [`Action`], so the whole interaction can be driven
//! from a test without a TTY, and the event loop in `main` stays a thin adapter
//! that turns `Action`s into drawing and quitting.
//!
//! The key map is deliberately small: `esc` abandons a lesson without recording
//! it, `tab` restarts, `q` quits only from the idle typing screen (while a lesson
//! runs, `q` is a letter to type), and `ctrl-c` always quits. Every other
//! character is fed to the session — after positional translation when emulation
//! is on, because the terminal reports the *host* layout's character.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keytle_core::config::{Config, ConfigError, Mode};
use keytle_core::corpora;
use keytle_core::corpus::{Corpus, CorpusError};
use keytle_core::emulate::Emulator;
use keytle_core::engine::generate::{generate, Lesson, Note};
use keytle_core::engine::profile::{Profile, Verdict};
use keytle_core::layout::{Layout, LayoutError, Position};
use keytle_core::layouts;
use keytle_core::paths::Paths;
use keytle_core::stats::{self, Filter, KeyRow, TestRecord};
use keytle_core::typing::{SessionStats, TypingSession};

/// Which screen is in front.
///
/// `Typing` is home: `s` opens the dashboard from it, `?` the help, and `esc`
/// or `q` closes both again — always back to typing, because that is the screen
/// a lesson starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Typing,
    Stats,
    Settings,
    Help,
    Results,
}

/// What the event loop should do after a key has been handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing beyond the usual redraw.
    Redraw,
    /// Leave the alternate screen and exit.
    Quit,
    /// A lesson was started (or restarted): redraw from the top.
    StartLesson,
    /// The lesson's last character has been typed; the loop should record it.
    Finish,
}

/// Why the app could not start.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("layout \"{id}\": {source}")]
    Layout { id: String, source: LayoutError },
    #[error("corpus \"{lang}\": {source}")]
    Corpus { lang: String, source: CorpusError },
}

/// Which series the dashboard's chart draws. `c` cycles speed → accuracy →
/// both, and the toggle is remembered for the session, not written anywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChartMode {
    /// Net words per minute, one column per test.
    #[default]
    Speed,
    /// Per-test accuracy, on a fixed 0–100% scale.
    Accuracy,
    /// Both, speed on top.
    Both,
}

impl ChartMode {
    /// The next mode in the cycle, wrapping back to speed.
    pub fn next(self) -> ChartMode {
        match self {
            ChartMode::Speed => ChartMode::Accuracy,
            ChartMode::Accuracy => ChartMode::Both,
            ChartMode::Both => ChartMode::Speed,
        }
    }

    /// The one word the chart's title uses.
    pub fn label(self) -> &'static str {
        match self {
            ChartMode::Speed => "speed",
            ChartMode::Accuracy => "accuracy",
            ChartMode::Both => "speed + accuracy",
        }
    }
}

/// What the stats dashboard is showing. `None` means "every one in the file".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsFilter {
    pub layout: Option<String>,
    pub lang: Option<String>,
    /// Fold pseudo/words/sentences into one line; the header says `(merged)`.
    pub merged: bool,
    /// Which series the chart plots; `c` cycles it.
    pub chart: ChartMode,
}

impl StatsFilter {
    /// The store filter this selection means. Merging modes is exactly "no mode
    /// predicate", so the view never folds anything itself.
    pub fn to_filter(&self, mode: Mode) -> Filter {
        Filter {
            layout: self.layout.clone(),
            lang: self.lang.clone(),
            mode: (!self.merged).then_some(mode),
        }
    }
}

/// What the unlock rule said, with the letter it was talking about, so a screen
/// can explain a block instead of only naming the verdict.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnlockOutcome {
    /// The next letter of the teaching order, if any is left.
    pub next: Option<char>,
    pub verdict: Verdict,
    /// Mean confidence of the reachable unlocked keys, at the same instant.
    pub mean: Option<f32>,
}

/// The settings screen's editable rows, in the order they are drawn. The index
/// into this table is the cursor the screen moves, and each row's values come
/// from [`App::settings_values`].
pub const SETTINGS_ROWS: [&str; 8] = [
    "layout",
    "host layout",
    "language",
    "mode",
    "emulate",
    "strict",
    "lesson words",
    "target wpm",
];

/// The lesson sizes `settings_cycle` steps through, smallest first.
const LESSON_WORDS: [usize; 5] = [10, 20, 30, 40, 50];
/// The target speeds `settings_cycle` steps through, slowest first.
const TARGET_WPMS: [f32; 7] = [40.0, 60.0, 80.0, 100.0, 120.0, 150.0, 200.0];

/// Everything the UI reads: settings, the two boards, the profile, the lesson.
pub struct App {
    pub config: Config,
    /// The layout being taught.
    pub layout: Layout,
    /// Host board mapped positionally onto the target board. It keeps the host
    /// layout too; `emulator.host()` is the way to read it.
    pub emulator: Emulator,
    pub profile: Profile,
    pub corpus: Corpus,
    pub session: Option<TypingSession>,
    pub lesson: Option<Lesson>,
    pub screen: Screen,
    /// One line under the keyboard: an abort, a note, a save failure.
    pub notice: Option<String>,
    pub lesson_started_ms: u64,
    /// The last finished lesson's totals, for the results screen.
    pub last_stats: Option<SessionStats>,
    /// The character that was just mistyped, for error styling. Cleared by the
    /// next hit.
    pub last_miss: Option<char>,
    /// The lesson history, in file order. Loaded at start-up, appended on finish.
    pub records: Vec<TestRecord>,
    /// Why the history could not be read, when it could not. Kept so the screens
    /// that show history can say so instead of quietly showing none.
    history_error: Option<String>,
    /// The three weakest keys of the last finished lesson, weakest first.
    pub last_weak: Vec<KeyRow>,
    /// The unlock rule's answer after the last finished lesson.
    pub last_unlock: Option<UnlockOutcome>,
    /// What the stats dashboard is currently filtered to.
    pub stats: StatsFilter,
    /// Target positions the host board can actually press.
    reachable_positions: BTreeSet<Position>,
    /// Target characters the host board can actually produce.
    reachable_chars: BTreeSet<char>,
    profile_path: PathBuf,
    tests_path: PathBuf,
    /// Where layouts, profiles and the history live. A layout or language
    /// switch has to resolve the new layout and the new profile file from here.
    paths: Paths,
    /// The host board's layout, kept so reachability can be recomputed when the
    /// settings change the host or turn emulation on and off.
    host: Layout,
    /// Which row the settings cursor is on, as an index into [`SETTINGS_ROWS`].
    settings_row: usize,
    /// The settings as they were when the screen opened: `esc` puts them back.
    settings_backup: Option<Config>,
    /// The screen `?` was pressed on, so help closes back onto it.
    help_return: Screen,
}

impl App {
    /// Load the layout, the host board, the profile and the corpus.
    ///
    /// A profile that cannot be read is not an error on first run: a fresh one is
    /// created, and saved best-effort so the next start finds it. Layout and
    /// corpus failures are real errors — without them there is no lesson.
    pub fn new(config: Config, paths: &Paths, now_ms: u64) -> Result<App, AppError> {
        let layout_dir = paths.layouts_dir();
        let layout = layouts::load(&config.layout, &layout_dir).map_err(|source| {
            AppError::Layout {
                id: config.layout.clone(),
                source,
            }
        })?;
        let host = layouts::load(&config.host_layout, &layout_dir).map_err(|source| {
            AppError::Layout {
                id: config.host_layout.clone(),
                source,
            }
        })?;
        let corpus = corpora::load(&config.language, None).map_err(|source| AppError::Corpus {
            lang: config.language.clone(),
            source,
        })?;
        let emulator = Emulator::new(host.clone(), layout.clone());
        let profile_path = paths.profile_file(&config.layout, &config.language);
        // A fresh profile stands in until `load_profile` below reads the file;
        // on first run it is the one that gets saved.
        let profile = Profile::new(&layout, &config.language, now_ms);
        let reachable_positions = positions_reachable(&layout, &host, config.emulate);
        let reachable_chars = chars_reachable(&layout, &reachable_positions);
        let tests_path = paths.tests_file();
        // An unreadable or corrupted history must not stop the trainer from
        // starting: the error is kept and the history screens say so, while the
        // typing works and every new test is still appended.
        let (records, history_error) = match stats::read_all(&tests_path) {
            Ok(records) => (records, None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        let stats_filter = StatsFilter {
            layout: Some(config.layout.clone()),
            lang: Some(config.language.clone()),
            merged: false,
            chart: ChartMode::Speed,
        };
        let mut app = App {
            config,
            layout,
            host,
            emulator,
            profile,
            corpus,
            session: None,
            lesson: None,
            screen: Screen::Typing,
            notice: None,
            lesson_started_ms: now_ms,
            last_stats: None,
            last_miss: None,
            reachable_positions,
            reachable_chars,
            profile_path,
            tests_path,
            records,
            history_error,
            last_weak: Vec::new(),
            last_unlock: None,
            stats: stats_filter,
            paths: paths.clone(),
            settings_row: 0,
            settings_backup: None,
            help_return: Screen::Typing,
        };
        // The file's progress wins over the placeholder whenever there is one.
        app.load_profile(now_ms);
        Ok(app)
    }

    /// Handle one key. Pure: no terminal is touched, and the clock comes in.
    pub fn handle_key(&mut self, key: KeyEvent, now_ms: u64) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Action::Quit;
        }
        // Settings and help own their keys outright: a stale `enter` must not
        // start a lesson behind a panel.
        match self.screen {
            Screen::Settings => return self.settings_key(key, now_ms),
            Screen::Help => return self.help_key(key),
            _ => {}
        }
        if self.screen != Screen::Typing {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.screen = Screen::Typing;
                    self.notice = None;
                }
                // `tab` is the dashboard's way back to the keyboard; on results
                // it is the same as enter, because there is nothing to filter.
                KeyCode::Tab if self.screen == Screen::Stats => self.screen = Screen::Typing,
                KeyCode::Enter | KeyCode::Tab => {
                    self.start_lesson(now_ms);
                    return Action::StartLesson;
                }
                _ => self.other_screen_char(key.code),
            }
            return Action::Redraw;
        }
        match key.code {
            KeyCode::Esc => {
                if self.session.take().is_some() {
                    // Abandoned lessons are not recorded: no profile totals, no
                    // stats line. The screen stays idle and ready.
                    self.lesson = None;
                    self.last_miss = None;
                    self.notice = Some("lesson abandoned — nothing recorded".to_string());
                }
                Action::Redraw
            }
            KeyCode::Tab => {
                self.start_lesson(now_ms);
                Action::StartLesson
            }
            KeyCode::Char('q') if self.is_idle() => Action::Quit,
            KeyCode::Char('s') if self.is_idle() => {
                self.open_stats();
                Action::Redraw
            }
            KeyCode::Char('?') if self.is_idle() => {
                self.open_help();
                Action::Redraw
            }
            KeyCode::Char(',') if self.is_idle() => {
                self.open_settings();
                Action::Redraw
            }
            KeyCode::Char('u') if self.is_idle() => {
                self.unlock_next(now_ms);
                Action::Redraw
            }
            KeyCode::Char('l') if self.is_idle() => {
                self.lock_current();
                Action::Redraw
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.type_char(c, now_ms)
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.is_idle() => {
                self.start_lesson(now_ms);
                Action::StartLesson
            }
            _ => {
                if let Some(session) = &mut self.session {
                    session.ignore_input();
                }
                Action::Redraw
            }
        }
    }

    /// Score one character: translate it when emulating, then feed the session.
    fn type_char(&mut self, typed: char, now_ms: u64) -> Action {
        if self.session.is_none() {
            self.start_lesson(now_ms);
            if self.session.is_none() {
                return Action::Redraw;
            }
        }
        let typed = if self.config.emulate {
            match self.emulator.translate(typed) {
                Some(mapped) => mapped,
                None => {
                    // The host has no key at that level: not a typo, just not a
                    // keystroke the trainer can charge.
                    if let Some(session) = &mut self.session {
                        session.ignore_input();
                    }
                    return Action::Redraw;
                }
            }
        } else {
            typed
        };
        let Some(session) = &mut self.session else {
            return Action::Redraw;
        };
        let outcome = session.apply(typed, now_ms);
        self.last_miss = match outcome {
            keytle_core::typing::Outcome::Miss { typed, .. } => Some(typed),
            keytle_core::typing::Outcome::Hit => None,
            _ => self.last_miss,
        };
        if session.is_complete() {
            Action::Finish
        } else {
            Action::Redraw
        }
    }

    /// Generate a lesson for the profile's current alphabet and start typing it.
    pub fn start_lesson(&mut self, now_ms: u64) {
        let mut rng = rand::rng();
        let lesson = generate(
            &self.layout,
            &self.corpus,
            &self.config.language,
            &self.profile.keys,
            &self.profile.unlocked,
            &self.reachable_chars,
            self.config.mode,
            self.config.lesson_words,
            &mut rng,
        );
        self.last_miss = None;
        self.lesson_started_ms = now_ms;
        self.screen = Screen::Typing;
        if lesson.text.is_empty() {
            self.session = None;
            self.lesson = None;
            self.notice = Some("no typable text for these keys — check the layout".to_string());
            return;
        }
        self.notice = lesson.note.map(note_text);
        self.session = Some(TypingSession::new(&lesson.text, self.config.strict, now_ms));
        self.lesson = Some(lesson);
    }

    /// Record a completed lesson: append one line to the stats file, fold the
    /// totals into the profile, act on the unlock rule, and show the results.
    ///
    /// A session that is not complete is left alone. The append happens before
    /// the profile save so a crash between the two costs an unlock, not a test:
    /// the stats file is the history, and losing a finished lesson is worse.
    pub fn finish_lesson(&mut self, now_ms: u64) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.is_complete() {
            return;
        }
        let stats = session.stats();
        let unlocked_before = u32::try_from(self.profile.unlocked.len()).unwrap_or(u32::MAX);
        // The evidence from this lesson is what the rule should judge, so the
        // fold comes before the verdict.
        self.profile.absorb(&stats);
        self.profile.tick_session(now_ms);
        let outcome = self.unlock_outcome();
        let event = self.profile.apply(outcome.verdict, now_ms);
        let unlocked_after = u32::try_from(self.profile.unlocked.len()).unwrap_or(u32::MAX);
        let record = TestRecord {
            v: keytle_core::stats::RECORD_VERSION,
            ts: rfc3339(now_ms),
            layout: self.config.layout.clone(),
            host: self.config.host_layout.clone(),
            emulated: self.config.emulate,
            lang: self.config.language.clone(),
            mode: self.config.mode,
            lesson: self.profile.lesson_index,
            wpm: stats.net_wpm,
            raw_wpm: stats.raw_wpm,
            accuracy: stats.accuracy,
            duration_ms: stats.duration_ms,
            keys: stats.per_key.clone(),
            unlocked_before,
            unlocked_after,
        };
        let mut notice = format!(
            "lesson done — {:.0} wpm, {:.0}% accuracy",
            stats.net_wpm,
            stats.accuracy * 100.0
        );
        if let Some(event) = event {
            notice.push_str(&format!(" · unlocked {}", event.ch));
        }
        match stats::append(&self.tests_path, &record) {
            Ok(()) => self.records.push(record),
            Err(err) => notice.push_str(&format!(" (test not recorded: {err})")),
        }
        self.profile.lesson_index = self.profile.lesson_index.saturating_add(1);
        if let Err(err) = self.profile.save(&self.profile_path) {
            notice.push_str(&format!(" (profile not saved: {err})"));
        }
        let weak = self.weakest_keys();
        self.last_weak = weak;
        self.last_unlock = Some(outcome);
        self.notice = Some(notice);
        self.last_stats = Some(stats);
        self.session = None;
        self.screen = Screen::Results;
    }

    /// Switch the layout being taught, and load the profile that belongs to the
    /// new (layout, language) pair.
    ///
    /// Nothing carries over: the profile file *is* a pair's whole memory, so
    /// switching to `bepo` reads `bepo.<lang>.json` and can never show the
    /// unlocks earned on the board that was on screen a moment ago.
    pub fn switch_layout(&mut self, id: &str, now_ms: u64) -> Result<(), AppError> {
        let previous = std::mem::replace(&mut self.config.layout, id.to_string());
        if let Err(err) = self.rebuild(now_ms) {
            self.config.layout = previous;
            return Err(err);
        }
        Ok(())
    }

    /// Switch the corpus language, loading the profile for the new pair.
    pub fn switch_language(&mut self, lang: &str, now_ms: u64) -> Result<(), AppError> {
        let previous = std::mem::replace(&mut self.config.language, lang.to_string());
        if let Err(err) = self.rebuild(now_ms) {
            self.config.language = previous;
            return Err(err);
        }
        Ok(())
    }

    /// Re-derive everything that depends on the config: both boards, the
    /// emulator, the reachable alphabet, the corpus and the profile.
    ///
    /// Every load happens before any assignment, so a layout or corpus that does
    /// not resolve leaves the app exactly as it was.
    fn rebuild(&mut self, now_ms: u64) -> Result<(), AppError> {
        let dir = self.paths.layouts_dir();
        let layout = layouts::load(&self.config.layout, &dir).map_err(|source| AppError::Layout {
            id: self.config.layout.clone(),
            source,
        })?;
        let host = layouts::load(&self.config.host_layout, &dir).map_err(|source| AppError::Layout {
            id: self.config.host_layout.clone(),
            source,
        })?;
        let corpus = corpora::load(&self.config.language, None).map_err(|source| AppError::Corpus {
            lang: self.config.language.clone(),
            source,
        })?;
        self.emulator = Emulator::new(host.clone(), layout.clone());
        self.layout = layout;
        self.host = host;
        self.corpus = corpus;
        self.refresh_reachability();
        self.load_profile(now_ms);
        // A lesson in flight was generated for the old alphabet.
        self.session = None;
        self.lesson = None;
        self.last_miss = None;
        Ok(())
    }

    /// Which target positions, and therefore characters, the host board can type.
    fn refresh_reachability(&mut self) {
        self.reachable_positions =
            positions_reachable(&self.layout, &self.host, self.config.emulate);
        self.reachable_chars = chars_reachable(&self.layout, &self.reachable_positions);
    }

    /// Load the profile for the current (layout, language) pair, creating and
    /// saving a fresh one when there is no file yet.
    fn load_profile(&mut self, now_ms: u64) {
        let path = self
            .paths
            .profile_file(&self.config.layout, &self.config.language);
        self.profile = match Profile::load(&path) {
            Ok(profile) => profile,
            Err(_) => {
                let profile = Profile::new(&self.layout, &self.config.language, now_ms);
                let _ = profile.save(&path);
                profile
            }
        };
        self.profile_path = path;
    }

    /// Change one config field and rebuild; a value that does not resolve is
    /// rolled back rather than left half-applied.
    fn edit_config(&mut self, now_ms: u64, change: impl FnOnce(&mut Config)) {
        let previous = self.config.clone();
        change(&mut self.config);
        if let Err(err) = self.rebuild(now_ms) {
            self.config = previous;
            self.notice = Some(err.to_string());
        }
    }

    /// Open the settings screen, remembering the settings `esc` puts back.
    pub fn open_settings(&mut self) {
        self.settings_backup = Some(self.config.clone());
        self.settings_row = 0;
        self.screen = Screen::Settings;
        self.notice = None;
    }

    /// Leave the settings screen without saving: restore the opening snapshot.
    pub fn discard_settings(&mut self, now_ms: u64) {
        if let Some(config) = self.settings_backup.take() {
            self.config = config;
            if let Err(err) = self.rebuild(now_ms) {
                self.notice = Some(format!("settings could not be restored: {err}"));
            }
        }
        self.screen = Screen::Typing;
    }

    /// Write the edited settings to the config file, then read them back from
    /// it — so what the screen shows is exactly what a restart would load.
    pub fn save_settings(&mut self, now_ms: u64) -> Result<PathBuf, ConfigError> {
        let path = self.paths.config_file();
        self.config.save(&path)?;
        self.config = Config::load(&path)?;
        self.settings_backup = Some(self.config.clone());
        match self.rebuild(now_ms) {
            Ok(()) => self.notice = Some(format!("settings saved to {}", path.display())),
            // The file is good; only the live reload failed. Say which.
            Err(err) => self.notice = Some(format!("saved to {}, but: {err}", path.display())),
        }
        Ok(path)
    }

    /// Where a saved config would go, as the settings screen states it.
    pub fn config_file(&self) -> PathBuf {
        self.paths.config_file()
    }

    /// Which row the settings cursor is on.
    pub fn settings_row(&self) -> usize {
        self.settings_row
    }

    /// The value of every settings row, in [`SETTINGS_ROWS`] order.
    pub fn settings_values(&self) -> Vec<String> {
        vec![
            self.config.layout.clone(),
            self.config.host_layout.clone(),
            self.config.language.clone(),
            self.config.mode.to_string(),
            on_off(self.config.emulate),
            on_off(self.config.strict),
            self.config.lesson_words.to_string(),
            format!("{:.0}", self.config.target_wpm),
        ]
    }

    /// Move the settings cursor by `delta` rows, wrapping at either end.
    pub fn settings_move(&mut self, delta: isize) {
        let rows = SETTINGS_ROWS.len() as isize;
        self.settings_row = ((self.settings_row as isize + delta).rem_euclid(rows)) as usize;
    }

    /// Step the selected setting to its next (or previous) value and apply it to
    /// the live app, so the typing screen matches what the settings show.
    pub fn settings_cycle(&mut self, now_ms: u64, forward: bool) {
        match self.settings_row {
            0 => {
                let ids = layouts::available(&self.paths.layouts_dir());
                let next = step_in(&ids, &self.config.layout, forward);
                if let Err(err) = self.switch_layout(&next, now_ms) {
                    self.notice = Some(err.to_string());
                }
            }
            1 => {
                let ids: Vec<String> = layouts::host_ids().iter().map(|id| (*id).to_string()).collect();
                let next = step_in(&ids, &self.config.host_layout, forward);
                self.edit_config(now_ms, |config| config.host_layout = next);
            }
            2 => {
                let langs: Vec<String> = corpora::languages().iter().map(|l| (*l).to_string()).collect();
                let next = step_in(&langs, &self.config.language, forward);
                if let Err(err) = self.switch_language(&next, now_ms) {
                    self.notice = Some(err.to_string());
                }
            }
            3 => {
                const MODES: [Mode; 3] = [Mode::Pseudo, Mode::Words, Mode::Sentences];
                let at = MODES.iter().position(|mode| *mode == self.config.mode).unwrap_or(0);
                let next = if forward { (at + 1) % MODES.len() } else { (at + 2) % MODES.len() };
                self.config.mode = MODES[next];
            }
            4 => {
                self.config.emulate = !self.config.emulate;
                // Emulation is what makes a position out of reach in the first
                // place, so the alphabet depends on this flag.
                self.refresh_reachability();
            }
            5 => self.config.strict = !self.config.strict,
            6 => {
                self.config.lesson_words = step_in(&LESSON_WORDS, &self.config.lesson_words, forward);
            }
            _ => {
                self.config.target_wpm = step_in(&TARGET_WPMS, &self.config.target_wpm, forward);
            }
        }
    }

    /// Unlock the next letter of the teaching order on the user's say-so, past
    /// the rule. The `u` binding the help screen advertises.
    pub fn unlock_next(&mut self, now_ms: u64) {
        let Some(ch) = self
            .profile
            .next_key(&self.layout, &self.config.language, &self.reachable_chars)
        else {
            self.notice = Some("every letter is unlocked".to_string());
            return;
        };
        self.profile.unlock_manually(ch, now_ms);
        self.notice = Some(match self.profile.save(&self.profile_path) {
            Ok(()) => format!("unlocked {ch} (manual)"),
            Err(err) => format!("unlocked {ch} (not saved: {err})"),
        });
    }

    /// Pin the current unlocks by writing the profile now, so closing the
    /// terminal mid-lesson cannot lose them. The `l` binding.
    pub fn lock_current(&mut self) {
        let count = self.profile.unlocked.len();
        self.notice = Some(match self.profile.save(&self.profile_path) {
            Ok(()) => format!("locked {count} keys"),
            Err(err) => format!("profile not saved: {err}"),
        });
    }

    /// Open the help screen, remembering where to return to.
    pub fn open_help(&mut self) {
        self.help_return = self.screen;
        self.screen = Screen::Help;
        self.notice = None;
    }

    /// Close help, back where it was opened from.
    pub fn close_help(&mut self) {
        self.screen = self.help_return;
    }

    /// Show the dashboard, re-reading the history first so a lesson finished in
    /// another window (or by another tool) is not invisible.
    pub fn open_stats(&mut self) {
        match stats::read_all(&self.tests_path) {
            Ok(records) => {
                self.records = records;
                self.history_error = None;
            }
            Err(err) => {
                self.records = Vec::new();
                self.history_error = Some(err.to_string());
            }
        }
        self.screen = Screen::Stats;
    }

    /// The history-read failure, phrased for every screen that draws it, or
    /// `None` when the file read cleanly.
    pub fn history_error(&self) -> Option<String> {
        self.history_error
            .as_ref()
            .map(|err| format!("history could not be read: {err} — new tests are still recorded"))
    }

    /// What the unlock rule would say right now, with the letter it is about.
    pub fn unlock_outcome(&self) -> UnlockOutcome {
        let target = self.config.target_interval_ms();
        UnlockOutcome {
            next: self
                .profile
                .next_key(&self.layout, &self.config.language, &self.reachable_chars),
            verdict: self.profile.verdict(
                &self.layout,
                &self.config.language,
                &self.reachable_chars,
                target,
            ),
            mean: self.profile.mean_confidence(&self.reachable_chars, target),
        }
    }

    /// The three weakest keys of the last recorded lesson, weakest first.
    fn weakest_keys(&self) -> Vec<KeyRow> {
        let filter = Filter {
            layout: Some(self.config.layout.clone()),
            lang: Some(self.config.language.clone()),
            mode: Some(self.config.mode),
        };
        // A window of one: this lesson is what the results screen is about.
        stats::key_rows(&self.records, &filter, 1, self.config.target_interval_ms())
            .into_iter()
            .take(3)
            .collect()
    }

    /// Every choice `s` cycles: the configured layout, then the others the file
    /// mentions, then "all layouts".
    pub fn layout_choices(&self) -> Vec<Option<String>> {
        let mut choices = vec![Some(self.config.layout.clone())];
        choices.extend(self.seen("layout").filter(|id| *id != self.config.layout).map(Some));
        choices.push(None);
        choices
    }

    /// Every choice `l` cycles, the same way as [`App::layout_choices`].
    pub fn lang_choices(&self) -> Vec<Option<String>> {
        let mut choices = vec![Some(self.config.language.clone())];
        choices.extend(self.seen("lang").filter(|id| *id != self.config.language).map(Some));
        choices.push(None);
        choices
    }

    /// The distinct layouts or languages the history mentions, in file order.
    fn seen(&self, field: &str) -> impl Iterator<Item = String> + '_ {
        let mut seen: Vec<String> = Vec::new();
        for rec in &self.records {
            let value = if field == "layout" { &rec.layout } else { &rec.lang };
            if !value.is_empty() && !seen.contains(value) {
                seen.push(value.clone());
            }
        }
        seen.into_iter()
    }

    /// Keys the settings screen owns: `w` saves, `esc`/`q` throw the edits away,
    /// `j`/`k` (or the up/down arrows) move the cursor, and `space`, `enter` or
    /// the left/right arrows step the selected value.
    fn settings_key(&mut self, key: KeyEvent, now_ms: u64) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.discard_settings(now_ms),
            KeyCode::Char('w') => {
                if let Err(err) = self.save_settings(now_ms) {
                    self.notice = Some(format!("not saved: {err}"));
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.settings_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.settings_move(-1),
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Right => self.settings_cycle(now_ms, true),
            KeyCode::Left | KeyCode::Backspace => self.settings_cycle(now_ms, false),
            _ => {}
        }
        Action::Redraw
    }

    /// Keys the help screen owns: any of the ways out closes it.
    fn help_key(&mut self, key: KeyEvent) -> Action {
        if matches!(
            key.code,
            KeyCode::Esc
                | KeyCode::Char('q')
                | KeyCode::Char('?')
                | KeyCode::Tab
                | KeyCode::Enter
        ) {
            self.close_help();
        }
        Action::Redraw
    }

    /// Handle the letters the non-typing screens own.
    fn other_screen_char(&mut self, code: KeyCode) {
        match (self.screen, code) {
            (Screen::Results, KeyCode::Char('s')) => self.open_stats(),
            (Screen::Results | Screen::Stats, KeyCode::Char('?')) => self.open_help(),
            (Screen::Stats, KeyCode::Char('c')) => self.stats.chart = self.stats.chart.next(),
            (Screen::Stats, KeyCode::Char('s')) => self.cycle_layout(true),
            (Screen::Stats, KeyCode::Char('S')) => self.cycle_layout(false),
            (Screen::Stats, KeyCode::Char('l')) => self.cycle_lang(),
            (Screen::Stats, KeyCode::Char('m')) => self.stats.merged = !self.stats.merged,
            (Screen::Stats, KeyCode::Char('a')) => self.stats.layout = None,
            _ => {}
        }
    }

    /// Step the layout choice forward (`s`) or back (`S`).
    fn cycle_layout(&mut self, forward: bool) {
        let list = self.layout_choices();
        self.stats.layout = step(&list, &self.stats.layout, forward);
    }

    /// Step the language choice, in the same order.
    fn cycle_lang(&mut self) {
        let list = self.lang_choices();
        self.stats.lang = step(&list, &self.stats.lang, true);
    }

    /// Bracketed paste and other non-keystroke input: seen and discarded.
    pub fn ignore_input(&mut self) {
        if let Some(session) = &mut self.session {
            session.ignore_input();
        }
    }

    /// True when no lesson is in flight on the typing screen.
    pub fn is_idle(&self) -> bool {
        self.session.is_none()
    }

    /// The character the typist must press next, if a lesson is running.
    pub fn expected(&self) -> Option<char> {
        self.session.as_ref().and_then(TypingSession::expected)
    }

    /// Running totals of the current session, if any.
    pub fn stats(&self) -> Option<SessionStats> {
        self.session.as_ref().map(TypingSession::stats)
    }

    /// The host legend to draw under the target characters, when emulating.
    pub fn legend(&self) -> Option<&Emulator> {
        self.config.emulate.then_some(&self.emulator)
    }

    /// The target positions the host board can reach; everything else is drawn
    /// as unreachable.
    pub fn reachable_positions(&self) -> &BTreeSet<Position> {
        &self.reachable_positions
    }
}

/// The next value after `current` in `list`, wrapping at either end. A value
/// that is not in the list at all steps to the first one, so a hand-edited
/// config with an unusual speed still cycles to something the screen offers.
fn step_in<T: Clone + PartialEq>(list: &[T], current: &T, forward: bool) -> T {
    let Some(at) = list.iter().position(|item| item == current) else {
        return list.first().cloned().unwrap_or_else(|| current.clone());
    };
    let next = if forward {
        (at + 1) % list.len()
    } else {
        (at + list.len() - 1) % list.len()
    };
    list[next].clone()
}

/// `on` / `off`, the way the settings screen shows a boolean.
fn on_off(value: bool) -> String {
    if value {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

/// The next choice after `current` in `list`, wrapping at either end.
fn step(list: &[Option<String>], current: &Option<String>, forward: bool) -> Option<String> {
    let at = list.iter().position(|c| c == current).unwrap_or(0);
    let next = if forward {
        (at + 1) % list.len()
    } else {
        (at + list.len() - 1) % list.len()
    };
    list[next].clone()
}

/// The day count of a civil date, and back (Howard Hinnant's `days_from_civil`
/// and `civil_from_days`). Keeping the pair here means a timestamp needs no date
/// crate: the record only ever carries RFC 3339 in UTC.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// An epoch millisecond as RFC 3339 in UTC, the form a `TestRecord` records.
pub(crate) fn rfc3339(ms: u64) -> String {
    let secs = i64::try_from(ms / 1000).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Parse the `YYYY-MM-DDTHH:MM:SSZ` form back to epoch milliseconds. Anything
/// else (a hand-edited line, a future offset form) is `None`, which the caller
/// treats as "no timestamp" rather than as an error.
pub(crate) fn parse_rfc3339(ts: &str) -> Option<u64> {
    let bytes = ts.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        ts.get(from..to)?.parse::<i64>().ok()
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some(u64::try_from(secs.max(0) * 1000).unwrap_or(0))
}

/// Which target positions the host board also has a key for.
fn positions_reachable(layout: &Layout, host: &Layout, emulate: bool) -> BTreeSet<Position> {
    layout
        .keys
        .keys()
        .filter(|pos| !emulate || host.key_at(**pos).is_some())
        .copied()
        .collect()
}

/// Every legend of a reachable position, which is the alphabet a lesson may use.
fn chars_reachable(layout: &Layout, reachable: &BTreeSet<Position>) -> BTreeSet<char> {
    layout
        .keys
        .iter()
        .filter(|(pos, _)| reachable.contains(pos))
        .flat_map(|(_, key)| [Some(key.base), key.shift, key.altgr])
        .flatten()
        .collect()
}

fn note_text(note: Note) -> String {
    match note {
        Note::FallbackToPseudo => "too few usable words — drilling pseudo words".to_string(),
        Note::FullAlphabetSentences => "too few usable sentences — every letter allowed".to_string(),
        Note::SentencesDropped { dropped, total } => {
            format!("{dropped} of {total} sentences skipped: the unlocked keys cannot type them")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A scratch `Paths` under the system temp dir; removed when dropped.
    struct Scratch {
        paths: Paths,
        base: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "keytle-app-{}-{}",
                std::process::id(),
                id
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            Scratch {
                paths: Paths {
                    config_dir: base.join("config"),
                    data_dir: base.join("data"),
                },
                base,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn app(scratch: &Scratch) -> App {
        App::new(Config::default(), &scratch.paths, 0).unwrap()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn esc_mid_lesson_abandons_without_finishing() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.start_lesson(0);
        assert!(app.session.is_some());
        let first = app.lesson.as_ref().unwrap().text.chars().next().unwrap();
        app.handle_key(key(KeyCode::Char(first)), 100);
        let action = app.handle_key(key(KeyCode::Esc), 200);
        assert_ne!(action, Action::Finish);
        assert!(app.is_idle(), "esc must drop the session");
        assert_eq!(app.screen, Screen::Typing);
        assert_eq!(app.profile.tests_completed, 0, "an abort is not recorded");
        assert!(app.notice.as_deref().unwrap().contains("abandoned"));
    }

    #[test]
    fn q_quits_only_when_idle() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        assert_eq!(app.handle_key(key(KeyCode::Char('q')), 0), Action::Quit);
        app.start_lesson(0);
        // While a lesson runs, `q` is a letter, never a quit.
        assert_ne!(app.handle_key(key(KeyCode::Char('q')), 10), Action::Quit);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), 20),
            Action::Quit
        );
    }

    #[test]
    fn tab_restarts_the_lesson() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.start_lesson(0);
        let started = app.lesson_started_ms;
        let action = app.handle_key(key(KeyCode::Tab), 500);
        assert_eq!(action, Action::StartLesson);
        assert_eq!(app.lesson_started_ms, 500);
        assert_ne!(started, app.lesson_started_ms);
        assert!(app.session.is_some());
    }

    #[test]
    fn typing_the_whole_lesson_finishes_exactly_once() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.config.emulate = false;
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        let mut finishers = 0;
        for (i, ch) in text.chars().enumerate() {
            let action = app.handle_key(key(KeyCode::Char(ch)), 100 + i as u64);
            if action == Action::Finish {
                finishers += 1;
                app.finish_lesson(100 + i as u64);
            }
        }
        assert_eq!(finishers, 1);
        assert_eq!(app.screen, Screen::Results);
        assert_eq!(app.profile.tests_completed, 1);
        assert!(app.last_stats.unwrap().accuracy > 0.99);

        // Exactly one JSONL line, and it is a well-formed v1 record that names
        // the test the results screen is showing.
        let path = scratch.paths.tests_file();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 1, "one finished lesson, one line:\n{raw}");
        assert_eq!(raw.matches('\n').count(), 1, "the line ends with one newline");
        let records = stats::read_all(&path).unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.v, keytle_core::stats::RECORD_VERSION);
        assert!(parse_rfc3339(&record.ts).is_some(), "ts is RFC 3339: {}", record.ts);
        assert_eq!(record.layout, app.config.layout);
        assert_eq!(record.host, app.config.host_layout);
        assert_eq!(record.lang, app.config.language);
        assert_eq!(record.mode, app.config.mode);
        assert!(record.wpm > 0.0 && record.duration_ms > 0);
        assert!(!record.keys.is_empty(), "per-key evidence is recorded");
        assert_eq!(record.unlocked_before, 4);
        assert_eq!(record.unlocked_after, u32::try_from(app.profile.unlocked.len()).unwrap());
        assert_eq!(app.records.len(), 1, "the app remembers what it wrote");
    }

    #[test]
    fn emulation_translates_the_host_character_by_position() {
        // qwerty host, Colemak-DH target: `e` sits where Colemak-DH puts `f`.
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        assert_eq!(app.emulator.translate('e'), Some('f'));
        app.start_lesson(0);
        // Force a one-character lesson so the translated char is unambiguous.
        app.session = Some(TypingSession::new("f", app.config.strict, 0));
        let action = app.handle_key(key(KeyCode::Char('e')), 100);
        assert_eq!(action, Action::Finish, "e on qwerty is f on Colemak-DH");
    }

    #[test]
    fn saving_settings_writes_the_file_and_reloads_the_same_values() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.open_settings();
        app.settings_move(4); // emulate
        app.settings_cycle(0, true);
        assert!(!app.config.emulate, "the toggle took in memory");
        app.settings_move(3); // target wpm
        app.settings_cycle(0, true);
        let expected = app.config.clone();
        assert_eq!(expected.target_wpm, 100.0, "80 steps up to 100");

        app.save_settings(0).expect("save");
        let path = scratch.paths.config_file();
        assert!(path.is_file(), "{} was written", path.display());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("target_wpm = 100"), "{raw}");
        assert_eq!(
            Config::load(&path).unwrap(),
            expected,
            "the file holds what the screen showed"
        );
        assert_eq!(app.config, expected, "the app reloaded the same values");
    }

    #[test]
    fn a_corrupt_middle_line_is_surfaced_not_swallowed() {
        let scratch = Scratch::new();
        let path = scratch.paths.tests_file();
        std::fs::create_dir_all(scratch.paths.data_dir.clone()).unwrap();
        let line = r#"{"v":1,"ts":"2026-11-15T10:00:00Z","layout":"colemak-dh","host":"qwerty","emulated":true,"lang":"en","mode":"pseudo","lesson":0,"wpm":40.0,"raw_wpm":44.0,"accuracy":0.96,"duration_ms":30000,"keys":{},"unlocked_before":4,"unlocked_after":5}"#;
        std::fs::write(&path, format!("{line}\nnot a record\n{line}\n")).unwrap();

        let mut app = app(&scratch);
        let err = app
            .history_error()
            .expect("a corrupt middle line is an error, not an empty history");
        assert!(err.contains("history could not be read"), "{err}");
        assert!(err.contains("new tests are still recorded"), "{err}");
        assert!(err.contains("line 2"), "the bad line is named: {err}");

        // The trainer still runs, and the next finished lesson is appended.
        app.config.emulate = false;
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        for (i, ch) in text.chars().enumerate() {
            if app.handle_key(key(KeyCode::Char(ch)), 100 + i as u64) == Action::Finish {
                app.finish_lesson(100 + i as u64);
            }
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 4, "the new test landed:\n{raw}");
        assert_eq!(app.records.len(), 1, "the app keeps what it wrote itself");
    }

    #[test]
    fn esc_on_settings_discards_the_edits() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        let before = app.config.clone();
        app.open_settings();
        app.settings_move(6);
        app.settings_cycle(0, true);
        assert_ne!(app.config.lesson_words, before.lesson_words);
        app.handle_key(key(KeyCode::Esc), 0);
        assert_eq!(app.screen, Screen::Typing);
        assert_eq!(app.config, before, "esc leaves without saving");
        assert!(!scratch.paths.config_file().is_file(), "nothing was written");
    }

    #[test]
    fn switching_layout_loads_the_profile_for_the_new_pair() {
        let scratch = Scratch::new();
        let config = Config {
            language: "fr".to_string(),
            ..Config::default()
        };
        let mut app = App::new(config, &scratch.paths, 0).unwrap();
        // Give colemak-dh a history the bepo profile must not inherit.
        app.profile.unlocked.insert('z');
        app.profile.tests_completed = 7;
        app.profile
            .save(&scratch.paths.profile_file("colemak-dh", "fr"))
            .unwrap();

        app.switch_layout("bepo", 0).expect("bepo is bundled");
        assert_eq!(app.config.layout, "bepo");
        assert_eq!(app.layout.id, "bepo");
        assert_eq!(app.profile.layout, "bepo");
        assert_eq!(app.profile.lang, "fr");
        assert_eq!(app.profile.tests_completed, 0, "no tests carried over");
        let fresh = Profile::new(&app.layout, "fr", 0);
        assert_eq!(app.profile.unlocked, fresh.unlocked, "bepo.fr's own alphabet");
        assert!(!app.profile.unlocked.contains(&'z'), "colemak's unlock stayed behind");
        // The pair's own file exists now, and it is the one the app reads.
        let path = scratch.paths.profile_file("bepo", "fr");
        assert!(path.is_file(), "{} was created", path.display());
        assert_eq!(Profile::load(&path).unwrap().layout, "bepo");
    }

    #[test]
    fn switching_language_loads_that_language_s_profile() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.profile.unlocked.insert('z');
        app.switch_language("fr", 0).expect("fr is bundled");
        assert_eq!(app.config.language, "fr");
        assert_eq!(app.profile.lang, "fr");
        assert!(!app.profile.unlocked.contains(&'z'));
        assert!(scratch.paths.profile_file("colemak-dh", "fr").is_file());
    }

    #[test]
    fn question_mark_opens_help_and_esc_comes_back() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        app.handle_key(key(KeyCode::Char('?')), 0);
        assert_eq!(app.screen, Screen::Help);
        app.handle_key(key(KeyCode::Esc), 0);
        assert_eq!(app.screen, Screen::Typing);
        // …and from the dashboard, it returns to the dashboard.
        app.open_stats();
        app.handle_key(key(KeyCode::Char('?')), 0);
        assert_eq!(app.screen, Screen::Help);
        app.handle_key(key(KeyCode::Char('?')), 0);
        assert_eq!(app.screen, Screen::Stats);
    }

    #[test]
    fn u_unlocks_the_next_key_and_l_saves_the_profile() {
        let scratch = Scratch::new();
        let mut app = app(&scratch);
        let before = app.profile.unlocked.len();
        app.handle_key(key(KeyCode::Char('u')), 0);
        assert_eq!(app.profile.unlocked.len(), before + 1);
        assert!(app.notice.as_deref().unwrap().contains("unlocked"));
        app.handle_key(key(KeyCode::Char('l')), 0);
        assert!(app.notice.as_deref().unwrap().contains("locked"));
        let saved = Profile::load(&scratch.paths.profile_file(&app.config.layout, &app.config.language))
            .expect("the profile was written");
        assert_eq!(saved.unlocked.len(), before + 1);
    }

    #[test]
    fn unreachable_layout_fails_loudly() {
        let scratch = Scratch::new();
        let config = Config {
            layout: "no-such-layout".to_string(),
            ..Config::default()
        };
        match App::new(config, &scratch.paths, 0) {
            Err(AppError::Layout { id, .. }) => assert_eq!(id, "no-such-layout"),
            Err(other) => panic!("wrong error: {other}"),
            Ok(_) => panic!("a missing layout must not start the TUI"),
        }
    }
}
