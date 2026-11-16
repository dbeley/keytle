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
use keytle_core::config::Config;
use keytle_core::corpora;
use keytle_core::corpus::{Corpus, CorpusError};
use keytle_core::emulate::Emulator;
use keytle_core::engine::generate::{generate, Lesson, Note};
use keytle_core::engine::profile::Profile;
use keytle_core::layout::{Layout, LayoutError, Position};
use keytle_core::layouts;
use keytle_core::paths::Paths;
use keytle_core::typing::{SessionStats, TypingSession};

/// Which screen is in front.
///
/// Only `Typing` and `Results` are reachable until the stats, settings and help
/// tasks land; the other three exist so those tasks add a body, not an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "stats, settings and help screens arrive with later tasks")]
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
    /// Target positions the host board can actually press.
    reachable_positions: BTreeSet<Position>,
    /// Target characters the host board can actually produce.
    reachable_chars: BTreeSet<char>,
    profile_path: PathBuf,
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
        let profile = match Profile::load(&profile_path) {
            Ok(profile) => profile,
            Err(_) => {
                let profile = Profile::new(&layout, &config.language, now_ms);
                let _ = profile.save(&profile_path);
                profile
            }
        };
        let reachable_positions = positions_reachable(&layout, &host, config.emulate);
        let reachable_chars = chars_reachable(&layout, &reachable_positions);
        Ok(App {
            config,
            layout,
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
        })
    }

    /// Handle one key. Pure: no terminal is touched, and the clock comes in.
    pub fn handle_key(&mut self, key: KeyEvent, now_ms: u64) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Action::Quit;
        }
        if self.screen != Screen::Typing {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.screen = Screen::Typing;
                    self.notice = None;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    self.start_lesson(now_ms);
                    return Action::StartLesson;
                }
                _ => {}
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

    /// Record a completed lesson: fold its totals into the profile and show the
    /// results screen. A session that is not complete is left alone.
    pub fn finish_lesson(&mut self, now_ms: u64) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.is_complete() {
            return;
        }
        let stats = session.stats();
        self.profile.absorb(&stats);
        self.profile.tick_session(now_ms);
        let mut notice = format!(
            "lesson done — {:.0} wpm, {:.0}% accuracy",
            stats.net_wpm,
            stats.accuracy * 100.0
        );
        if let Err(err) = self.profile.save(&self.profile_path) {
            notice.push_str(&format!(" (profile not saved: {err})"));
        }
        self.notice = Some(notice);
        self.last_stats = Some(stats);
        self.session = None;
        self.screen = Screen::Results;
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
