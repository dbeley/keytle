//! The typing state machine: one lesson, one cursor, one decision per keystroke.
//!
//! Time is injected (`now_ms`) rather than read from the clock, so a session is a
//! pure function of its input events and every rule below can be tested without
//! sleeping. The rules come straight from the design:
//!
//! * comparison is **exact** - `T` where `t` is expected is an [`Outcome::Miss`],
//!   and in strict mode it does not advance the cursor either;
//! * a wrong case is still an error for the session's accuracy, but it is *not*
//!   charged to the letter: it lands in [`KeyAgg::shift_misses`], because the
//!   letter was known and the modifier was not;
//! * in strict mode a miss records an error and the cursor stays put, so the
//!   right key must be typed before the lesson continues (keybr's feel); lenient
//!   mode records the miss and moves on;
//! * only one miss is charged per cursor position: a held-down wrong key
//!   auto-repeats, and every repeat after the first is [`Outcome::Ignored`];
//! * modifiers, function keys and pasted text never reach the scorer, and
//!   [`TypingSession::ignore_input`] is the door they leave through;
//! * intervals are measured between consecutive *correct* keystrokes; a gap over
//!   two seconds is a thinking pause and is dropped, and a timestamp that goes
//!   backwards contributes nothing at all.
//!
//! Per-key aggregates are keyed by the *lowercase expected* character, so `T` and
//! `t` feed one bucket - the same normalisation the progression engine uses. Inside
//! that bucket [`KeyAgg::hits`], [`KeyAgg::misses`] and [`KeyAgg::shift_misses`] are
//! what the unlock rule reads.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Gaps longer than this are pauses, not typing speed.
const MAX_INTERVAL_MS: u64 = 2_000;
/// Keystrokes per word, the WPM convention.
const CHARS_PER_WORD: f32 = 5.0;

/// What one keystroke meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The expected character arrived; the cursor advanced.
    Hit,
    /// Something else arrived. The cursor may or may not have advanced.
    Miss { expected: char, typed: char },
    /// A repeat of an already-charged miss, or input after the end. Costs nothing.
    Ignored,
    /// A character no key can produce (control codes); never scored.
    Unreachable,
}

/// Per-key totals for one session: how often the key was right, how often it was
/// wrong, and how fast it was hit on average.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyAgg {
    #[serde(default)]
    pub hits: u32,
    #[serde(default)]
    pub misses: u32,
    /// Wrong case typed where this key's character was expected: `T` for `t`.
    /// Kept apart from `misses` so the unlock rule can tell a letter that is not
    /// known from a modifier that is not known.
    #[serde(default)]
    pub shift_misses: u32,
    /// Rounded mean interval in ms between consecutive correct keystrokes.
    #[serde(default)]
    pub mean_dt: u32,
}

/// Running totals used to derive [`KeyAgg::mean_dt`].
#[derive(Debug, Clone, Copy, Default)]
struct Agg {
    hits: u32,
    misses: u32,
    shift_misses: u32,
    dt_sum: u64,
    dt_count: u32,
}

impl Agg {
    fn mean_dt(&self) -> u32 {
        if self.dt_count == 0 {
            return 0;
        }
        let count = u64::from(self.dt_count);
        u32::try_from((self.dt_sum + count / 2) / count).unwrap_or(u32::MAX)
    }
}

/// Everything the results screen shows for one finished (or in-flight) lesson.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStats {
    /// Correct characters over elapsed time: `chars / 5 / minutes`.
    pub net_wpm: f32,
    /// All scored keystrokes over elapsed time, mistakes included.
    pub raw_wpm: f32,
    /// `hits / (hits + misses)`, `1.0` before anything is typed.
    pub accuracy: f32,
    /// Wall time from session start to the last scored keystroke.
    pub duration_ms: u64,
    /// Per-key totals, keyed by the lowercase expected character.
    pub per_key: BTreeMap<char, KeyAgg>,
}

/// One run through one piece of text.
#[derive(Debug, Clone)]
pub struct TypingSession {
    text: Vec<char>,
    cursor: usize,
    strict: bool,
    hits: u32,
    misses: u32,
    /// All scored input: hits plus misses. This is what `raw_wpm` counts.
    keystrokes: u32,
    started_ms: u64,
    last_event_ms: u64,
    /// Timestamp of the last correct keystroke, the base for the next interval.
    last_hit_ms: Option<u64>,
    /// The wrong character already charged at the current cursor, if any.
    /// Auto-repeat of that same key is ignored; a different wrong key is a new
    /// miss, because it is a new decision by the typist.
    last_miss: Option<char>,
    per_key: BTreeMap<char, Agg>,
}

impl TypingSession {
    /// Start typing `text`. `now_ms` starts the clock; nothing else is implicit.
    pub fn new(text: &str, strict: bool, now_ms: u64) -> TypingSession {
        TypingSession {
            text: text.chars().collect(),
            cursor: 0,
            strict,
            hits: 0,
            misses: 0,
            keystrokes: 0,
            started_ms: now_ms,
            last_event_ms: now_ms,
            last_hit_ms: None,
            last_miss: None,
            per_key: BTreeMap::new(),
        }
    }

    /// Score one keystroke delivered at `now_ms`.
    pub fn apply(&mut self, typed: char, now_ms: u64) -> Outcome {
        if self.is_complete() {
            return Outcome::Ignored;
        }
        if typed.is_control() {
            return Outcome::Unreachable;
        }
        let expected = self.text[self.cursor];
        if typed == expected {
            self.hit(expected, now_ms);
            return Outcome::Hit;
        }
        // A repeat of the same wrong key while the cursor has not moved is
        // auto-repeat, not a second error.
        if self.last_miss == Some(typed) {
            return Outcome::Ignored;
        }
        self.misses += 1;
        self.keystrokes += 1;
        let key = expected.to_lowercase().next().unwrap_or(expected);
        // `T` where `t` was expected differs from the expected character only by
        // case: the letter is known, the shift is not. Charge it to the shift
        // counter so the letter keeps its earned accuracy.
        let case_only = typed.to_lowercase().next().unwrap_or(typed) == key;
        let agg = self.per_key.entry(key).or_default();
        if case_only {
            agg.shift_misses += 1;
        } else {
            agg.misses += 1;
        }
        self.last_miss = Some(typed);
        self.last_event_ms = self.last_event_ms.max(now_ms);
        if !self.strict {
            self.advance();
        }
        Outcome::Miss { expected, typed }
    }

    /// Modifiers, function keys, bracketed paste: seen and thrown away.
    pub fn ignore_input(&mut self) -> Outcome {
        Outcome::Ignored
    }

    /// Every character of the lesson, in order. A view needs the whole text to
    /// draw what is typed, what is current and what is still ahead.
    pub fn text(&self) -> &[char] {
        &self.text
    }

    /// How many characters are already behind the cursor.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The character the typist must press next, or `None` at the end.
    pub fn expected(&self) -> Option<char> {
        self.text.get(self.cursor).copied()
    }

    /// True once every character has been typed, including on empty text.
    pub fn is_complete(&self) -> bool {
        self.cursor >= self.text.len()
    }

    /// Totals so far. Safe to call mid-lesson.
    pub fn stats(&self) -> SessionStats {
        let duration_ms = self.last_event_ms.saturating_sub(self.started_ms);
        let minutes = duration_ms as f32 / 60_000.0;
        let (net_wpm, raw_wpm) = if minutes > 0.0 {
            (
                (self.hits as f32 / CHARS_PER_WORD) / minutes,
                (self.keystrokes as f32 / CHARS_PER_WORD) / minutes,
            )
        } else {
            (0.0, 0.0)
        };
        let scored = self.hits + self.misses;
        let accuracy = if scored == 0 {
            1.0
        } else {
            self.hits as f32 / scored as f32
        };
        SessionStats {
            net_wpm,
            raw_wpm,
            accuracy,
            duration_ms,
            per_key: self
                .per_key
                .iter()
                .map(|(c, agg)| {
                    (
                        *c,
                        KeyAgg {
                            hits: agg.hits,
                            misses: agg.misses,
                            shift_misses: agg.shift_misses,
                            mean_dt: agg.mean_dt(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Record a correct keystroke, advance, and fold in the interval.
    fn hit(&mut self, expected: char, now_ms: u64) {
        self.hits += 1;
        self.keystrokes += 1;
        // A timestamp that goes backwards is noise: `checked_sub` yields `None`
        // (no interval) and the reference point stays where it was, so the next
        // gap is measured from the last sane value.
        let interval = self.last_hit_ms.and_then(|prev| now_ms.checked_sub(prev));
        self.last_hit_ms = Some(self.last_hit_ms.map_or(now_ms, |prev| prev.max(now_ms)));
        let key = expected.to_lowercase().next().unwrap_or(expected);
        let agg = self.per_key.entry(key).or_default();
        agg.hits += 1;
        if let Some(dt) = interval.filter(|dt| *dt <= MAX_INTERVAL_MS) {
            agg.dt_sum += dt;
            agg.dt_count += 1;
        }
        self.last_event_ms = self.last_event_ms.max(now_ms);
        self.advance();
    }

    fn advance(&mut self) {
        self.cursor += 1;
        self.last_miss = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn strict_hits_advance_and_complete_only_at_the_end() {
        let mut s = TypingSession::new("ab", false, 0);
        assert!(!s.is_complete());
        assert_eq!(s.apply('a', 100), Outcome::Hit);
        assert!(!s.is_complete(), "one character of two is not the end");
        assert_eq!(s.apply('b', 200), Outcome::Hit);
        assert!(s.is_complete());
    }

    #[test]
    fn empty_text_is_complete_immediately() {
        assert!(TypingSession::new("", true, 0).is_complete());
    }

    #[test]
    fn strict_miss_keeps_the_cursor_and_is_charged_once() {
        let mut s = TypingSession::new("ab", true, 0);
        assert_eq!(
            s.apply('x', 100),
            Outcome::Miss {
                expected: 'a',
                typed: 'x'
            }
        );
        // Auto-repeat of the held wrong key: no second error, no movement.
        assert_eq!(s.apply('x', 130), Outcome::Ignored);
        assert_eq!(s.apply('x', 160), Outcome::Ignored);
        let stats = s.stats();
        assert_eq!(stats.per_key[&'a'].misses, 1);
        assert_eq!(stats.per_key[&'a'].hits, 0);
        assert!(close(stats.accuracy, 0.0));
        assert!(!s.is_complete());
        // The right key still works and moves on.
        assert_eq!(s.apply('a', 200), Outcome::Hit);
        assert_eq!(s.apply('b', 300), Outcome::Hit);
        assert!(s.is_complete());
    }

    #[test]
    fn a_different_wrong_key_at_the_same_cursor_is_a_fresh_miss() {
        let mut s = TypingSession::new("a", true, 0);
        assert!(matches!(s.apply('x', 10), Outcome::Miss { .. }));
        assert!(matches!(s.apply('y', 20), Outcome::Miss { .. }));
        assert!(matches!(s.apply('x', 30), Outcome::Miss { .. }));
        assert_eq!(s.stats().per_key[&'a'].misses, 3);
    }

    #[test]
    fn lenient_miss_advances_and_counts() {
        let mut s = TypingSession::new("ab", false, 0);
        assert_eq!(
            s.apply('x', 100),
            Outcome::Miss {
                expected: 'a',
                typed: 'x'
            }
        );
        assert_eq!(s.apply('b', 200), Outcome::Hit);
        assert!(s.is_complete());
        let stats = s.stats();
        assert_eq!(stats.per_key[&'a'].misses, 1);
        assert_eq!(stats.per_key[&'b'].hits, 1);
        assert!(close(stats.accuracy, 0.5));
    }

    #[test]
    fn ignore_input_changes_nothing() {
        let mut s = TypingSession::new("ab", true, 0);
        assert_eq!(s.apply('a', 100), Outcome::Hit);
        let before = s.stats();
        assert_eq!(s.ignore_input(), Outcome::Ignored);
        assert_eq!(s.ignore_input(), Outcome::Ignored);
        let after = s.stats();
        assert_eq!(before, after);
        assert_eq!(s.apply('b', 200), Outcome::Hit);
        assert!(s.is_complete());
    }

    #[test]
    fn wrong_case_is_a_miss_but_a_shift_miss_on_the_key() {
        let mut s = TypingSession::new("t", true, 0);
        assert_eq!(
            s.apply('T', 100),
            Outcome::Miss {
                expected: 't',
                typed: 'T'
            }
        );
        assert!(!s.is_complete(), "strict mode does not advance on a miss");
        assert_eq!(s.apply('t', 200), Outcome::Hit);
        // Both the failed capital and the working lower case feed one bucket.
        let stats = s.stats();
        let agg = stats.per_key[&'t'];
        assert_eq!(agg.hits, 1);
        assert_eq!(agg.shift_misses, 1);
        assert_eq!(agg.misses, 0, "the letter was known, so it is not blamed");
        // The session still counts the keystroke as an error.
        assert!((stats.accuracy - 0.5).abs() < EPS);
        assert!(
            stats.raw_wpm > stats.net_wpm,
            "the shift miss still costs raw speed"
        );
    }

    #[test]
    fn a_genuinely_wrong_letter_is_a_plain_miss_not_a_shift_miss() {
        let mut s = TypingSession::new("t", true, 0);
        assert!(matches!(s.apply('r', 100), Outcome::Miss { .. }));
        let agg = s.stats().per_key[&'t'];
        assert_eq!(agg.misses, 1);
        assert_eq!(agg.shift_misses, 0);
    }

    #[test]
    fn shift_misses_are_counted_per_occurrence_not_per_cursor() {
        // `T` is auto-repeat at one cursor, so it is charged once; the second
        // capital is a fresh decision and is charged again.
        let mut s = TypingSession::new("tt", true, 0);
        assert!(matches!(s.apply('T', 100), Outcome::Miss { .. }));
        assert_eq!(s.apply('T', 130), Outcome::Ignored, "auto-repeat");
        assert_eq!(s.apply('t', 200), Outcome::Hit);
        assert!(matches!(s.apply('T', 300), Outcome::Miss { .. }));
        assert_eq!(s.stats().per_key[&'t'].shift_misses, 2);
        assert_eq!(s.stats().per_key[&'t'].misses, 0);
    }

    #[test]
    fn control_characters_are_unreachable_and_scored_nowhere() {
        let mut s = TypingSession::new("a", true, 0);
        assert_eq!(s.apply('\u{1b}', 10), Outcome::Unreachable);
        assert_eq!(s.apply('\n', 20), Outcome::Unreachable);
        let stats = s.stats();
        assert!(close(stats.accuracy, 1.0));
        assert_eq!(stats.duration_ms, 0, "unreachable input is not an event");
        assert!(stats.per_key.is_empty());
    }

    #[test]
    fn input_after_the_end_is_ignored() {
        let mut s = TypingSession::new("a", false, 0);
        assert_eq!(s.apply('a', 100), Outcome::Hit);
        assert_eq!(s.apply('a', 200), Outcome::Ignored);
        assert_eq!(s.stats().per_key[&'a'].hits, 1);
    }

    #[test]
    fn five_chars_at_200ms_is_sixty_net_wpm() {
        let mut s = TypingSession::new("hello", false, 0);
        for (i, c) in "hello".chars().enumerate() {
            let now = 200 * (i as u64 + 1);
            assert_eq!(s.apply(c, now), Outcome::Hit);
        }
        let stats = s.stats();
        assert_eq!(stats.duration_ms, 1_000);
        assert!(close(stats.net_wpm, 60.0), "net_wpm was {}", stats.net_wpm);
        assert!(close(stats.raw_wpm, 60.0), "raw_wpm was {}", stats.raw_wpm);
        assert!(close(stats.accuracy, 1.0));
    }

    #[test]
    fn raw_wpm_counts_mistakes_that_net_wpm_does_not() {
        // "ab" in lenient mode: 1 hit, 1 miss, 2 keystrokes over 600 ms.
        let mut s = TypingSession::new("ab", false, 0);
        assert!(matches!(s.apply('x', 200), Outcome::Miss { .. }));
        assert_eq!(s.apply('b', 600), Outcome::Hit);
        let stats = s.stats();
        // net: 1 char / 5 / (0.6 s / 60) = 20 wpm; raw: 2 chars → 40 wpm.
        assert!(close(stats.net_wpm, 20.0), "net_wpm was {}", stats.net_wpm);
        assert!(close(stats.raw_wpm, 40.0), "raw_wpm was {}", stats.raw_wpm);
        assert!(close(stats.accuracy, 0.5));
    }

    #[test]
    fn per_key_mean_dt_is_rounded_mean_of_correct_intervals() {
        let mut s = TypingSession::new("aaab", false, 0);
        // 'a' at 100, 300 (dt 200), 700 (dt 400) → mean 300; 'b' at 1000 (dt 300).
        for (c, now) in [('a', 100), ('a', 300), ('a', 700), ('b', 1_000)] {
            assert_eq!(s.apply(c, now), Outcome::Hit);
        }
        let key = s.stats().per_key;
        assert_eq!(key[&'a'].mean_dt, 300);
        assert_eq!(key[&'a'].hits, 3);
        assert_eq!(key[&'b'].mean_dt, 300);
    }

    #[test]
    fn backwards_timestamp_contributes_no_interval() {
        let mut s = TypingSession::new("abc", false, 0);
        assert_eq!(s.apply('a', 500), Outcome::Hit);
        assert_eq!(s.apply('b', 300), Outcome::Hit); // backwards: no interval
        assert_eq!(s.apply('c', 900), Outcome::Hit); // measured from 500 → 400
        let agg = s.stats().per_key;
        assert_eq!(agg[&'a'].mean_dt, 0);
        assert_eq!(agg[&'b'].mean_dt, 0);
        assert_eq!(agg[&'c'].mean_dt, 400);
    }

    #[test]
    fn gaps_over_two_seconds_are_dropped_but_the_clock_keeps_running() {
        let mut s = TypingSession::new("abc", false, 0);
        assert_eq!(s.apply('a', 100), Outcome::Hit);
        assert_eq!(s.apply('b', 4_100), Outcome::Hit); // 4 s pause: ignored
        assert_eq!(s.apply('c', 4_200), Outcome::Hit); // 100 ms: kept
        let agg = s.stats().per_key;
        assert_eq!(agg[&'a'].mean_dt, 0);
        assert_eq!(agg[&'b'].mean_dt, 0);
        assert_eq!(agg[&'c'].mean_dt, 100);
        assert_eq!(s.stats().duration_ms, 4_200);
    }

    #[test]
    fn miss_intervals_are_not_mixed_into_key_speed() {
        let mut s = TypingSession::new("a", true, 0);
        assert!(matches!(s.apply('x', 100), Outcome::Miss { .. }));
        assert_eq!(s.apply('a', 190), Outcome::Hit);
        // The only correct keystroke has no predecessor, so there is no speed.
        assert_eq!(s.stats().per_key[&'a'].mean_dt, 0);
    }
}
