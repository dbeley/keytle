//! The profile: what the typist has unlocked, and the rule that unlocks more.
//!
//! A profile is the durable half of the progression engine. It remembers which
//! letters are unlocked, the evidence accumulated per key, and how many tests have
//! been finished, so the app can be closed at any moment and resume exactly where
//! it stopped.
//!
//! The unlock rule is deliberately conservative and doubly gated:
//!
//! * the mean [`confidence`](KeyStat::confidence) of the **reachable** unlocked
//!   keys must reach 0.7, and
//! * no reachable unlocked key may sit below 0.93 accuracy,
//!
//! with at most three unlocks per session (a session expires after thirty idle
//! minutes). Keys the host keyboard cannot produce are *excluded* from both
//! checks: an emulated layout may unlock a letter the host cannot type at all, and
//! leaving it in the average would deadlock progression forever.
//!
//! Serialisation is JSON: a profile is small, human-inspectable and versioned by
//! its own field set.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::key::KeyStat;
use crate::layout::Layout;
use crate::typing::SessionStats;

/// Letters unlocked on a brand-new profile.
const INITIAL_UNLOCKED: usize = 4;
/// Unlocks allowed in one session.
const MAX_UNLOCKS_PER_SESSION: u8 = 3;
/// Idle time after which the session (and its unlock budget) restarts.
const SESSION_IDLE_MS: u64 = 30 * 60 * 1000;
/// Mean confidence the reachable unlocked keys must reach.
const UNLOCK_CONFIDENCE: f32 = 0.7;
/// Hard accuracy floor: below this, nothing unlocks.
const UNLOCK_ACCURACY: f32 = 0.93;

/// How an unlock came about. `Auto` is the rule below; `Manual` and `Lock` exist
/// for the UI's "unlock this anyway" and "pin this set" escape hatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockHow {
    Auto,
    Manual,
    Lock,
}

/// The last letter that was unlocked, kept for the "new key" celebration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockEvent {
    pub ch: char,
    pub ts_ms: u64,
    pub how: UnlockHow,
}

/// Why the rule did not unlock anything this time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlockReason {
    /// Every letter of the teaching order is already unlocked.
    NoNextKey,
    /// Three unlocks already happened in this session.
    SessionCap,
    /// The reachable unlocked keys are not confident enough yet.
    LowConfidence(f32),
    /// One reachable unlocked key is not accurate enough: `(key, accuracy)`.
    Accuracy(char, f32),
}

/// What the progression rule decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Unlock(char),
    Blocked(BlockReason),
}

/// One layout+language pair's worth of progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub layout: String,
    pub lang: String,
    pub unlocked: BTreeSet<char>,
    pub keys: BTreeMap<char, KeyStat>,
    pub lesson_index: u32,
    pub tests_completed: u32,
    pub unlocks_this_session: u8,
    pub session_started_ms: u64,
    pub last_unlock: Option<UnlockEvent>,
}

impl Profile {
    /// A fresh profile: the first [`INITIAL_UNLOCKED`] letters of `layout`'s
    /// teaching order for `lang`, no evidence, no unlocks this session.
    pub fn new(layout: &Layout, lang: &str, now_ms: u64) -> Profile {
        Profile {
            layout: layout.id.clone(),
            lang: lang.to_string(),
            unlocked: layout
                .unlock_order_for(lang)
                .into_iter()
                .take(INITIAL_UNLOCKED)
                .collect(),
            keys: BTreeMap::new(),
            lesson_index: 0,
            tests_completed: 0,
            unlocks_this_session: 0,
            session_started_ms: now_ms,
            last_unlock: None,
        }
    }

    /// Read a profile from JSON. A missing file is an error here, not a silent
    /// default: the caller decides whether that means "start fresh".
    pub fn load(path: &Path) -> Result<Profile, ProfileError> {
        let text = fs::read_to_string(path).map_err(|source| ProfileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| ProfileError::Json {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Write the profile as JSON, creating the parent directory if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        fs::write(path, text)
    }

    /// Mean confidence of the unlocked keys that `reachable` allows, or `None`
    /// when there is nothing to average.
    ///
    /// Unreachable keys are excluded, not counted as zero: a letter the host
    /// keyboard cannot type would otherwise hold the mean down for ever.
    pub fn mean_confidence(
        &self,
        reachable: &BTreeSet<char>,
        target_interval_ms: f32,
    ) -> Option<f32> {
        let mut sum = 0.0;
        let mut count = 0u32;
        for ch in self.unlocked.iter().filter(|ch| reachable.contains(ch)) {
            let stat = self.keys.get(ch).copied().unwrap_or_default();
            sum += stat.confidence(target_interval_ms);
            count += 1;
        }
        (count > 0).then(|| sum / count as f32)
    }

    /// Apply the unlock rule. Pure: it answers, [`apply`](Profile::apply) acts.
    pub fn verdict(
        &self,
        layout: &Layout,
        lang: &str,
        reachable: &BTreeSet<char>,
        target_interval_ms: f32,
    ) -> Verdict {
        let Some(next) = self.next_key(layout, lang) else {
            return Verdict::Blocked(BlockReason::NoNextKey);
        };
        if self.unlocks_this_session >= MAX_UNLOCKS_PER_SESSION {
            return Verdict::Blocked(BlockReason::SessionCap);
        }
        // A single sloppy key stops the line: speed is worthless if the letter is
        // being missed. The worst reachable key is reported, ties by letter.
        let mut worst: Option<(char, f32)> = None;
        for ch in self.unlocked.iter().filter(|ch| reachable.contains(ch)) {
            let accuracy = self.keys.get(ch).copied().unwrap_or_default().accuracy();
            if accuracy < UNLOCK_ACCURACY && worst.is_none_or(|(_, acc)| accuracy < acc) {
                worst = Some((*ch, accuracy));
            }
        }
        if let Some((ch, accuracy)) = worst {
            return Verdict::Blocked(BlockReason::Accuracy(ch, accuracy));
        }
        match self.mean_confidence(reachable, target_interval_ms) {
            Some(mean) if mean >= UNLOCK_CONFIDENCE => Verdict::Unlock(next),
            // No reachable key at all is genuinely no confidence, not a pass.
            Some(mean) => Verdict::Blocked(BlockReason::LowConfidence(mean)),
            None => Verdict::Blocked(BlockReason::LowConfidence(0.0)),
        }
    }

    /// Act on a verdict. An [`Verdict::Unlock`] is recorded as an automatic
    /// unlock and returned; a block changes nothing.
    pub fn apply(&mut self, verdict: Verdict, now_ms: u64) -> Option<UnlockEvent> {
        let Verdict::Unlock(ch) = verdict else {
            return None;
        };
        self.unlocked.insert(ch);
        self.unlocks_this_session = self.unlocks_this_session.saturating_add(1);
        let event = UnlockEvent {
            ch,
            ts_ms: now_ms,
            how: UnlockHow::Auto,
        };
        self.last_unlock = Some(event);
        Some(event)
    }

    /// Fold one finished lesson's per-key totals into the profile.
    ///
    /// A session's mean interval is replayed once per hit, so a long session
    /// outweighs a short one by exactly the number of samples it collected, while
    /// [`KeyStat`]'s own exponential smoothing keeps old evidence alive.
    pub fn absorb(&mut self, stats: &SessionStats) {
        for (ch, agg) in &stats.per_key {
            let stat = self.keys.entry(*ch).or_default();
            let interval = (agg.mean_dt > 0).then_some(agg.mean_dt as f32);
            for _ in 0..agg.hits {
                stat.record(true, interval);
            }
            for _ in 0..agg.misses {
                stat.record(false, None);
            }
            stat.shift_misses = stat.shift_misses.saturating_add(agg.shift_misses);
        }
        self.tests_completed = self.tests_completed.saturating_add(1);
    }

    /// Start a new session once [`SESSION_IDLE_MS`] has passed, restoring the
    /// unlock budget.
    ///
    /// The clock is measured from the session start because the profile carries no
    /// per-keystroke timestamp: a caller ticking this on every render would
    /// otherwise reset the budget while the typist is still typing.
    pub fn tick_session(&mut self, now_ms: u64) {
        if now_ms.saturating_sub(self.session_started_ms) >= SESSION_IDLE_MS {
            self.unlocks_this_session = 0;
            self.session_started_ms = now_ms;
        }
    }

    /// The first letter of the teaching order this profile has not unlocked.
    fn next_key(&self, layout: &Layout, lang: &str) -> Option<char> {
        layout
            .unlock_order_for(lang)
            .into_iter()
            .find(|ch| !self.unlocked.contains(ch))
    }
}

/// Why a profile could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("bad profile JSON in {}: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;
    use crate::typing::{KeyAgg, SessionStats};
    use std::collections::{BTreeMap, BTreeSet};

    const TARGET: f32 = 150.0;

    fn colemak() -> Layout {
        let src = crate::layouts::bundled_source("colemak-dh").expect("bundled layout");
        Layout::from_toml(src).expect("valid bundled layout")
    }

    fn chars(s: &str) -> BTreeSet<char> {
        s.chars().collect()
    }

    fn strong(keys: &str) -> BTreeMap<char, KeyStat> {
        keys.chars()
            .map(|c| {
                (
                    c,
                    KeyStat {
                        hits: 100,
                        misses: 0,
                        shift_misses: 0,
                        mean_interval_ms: 150.0,
                    },
                )
            })
            .collect()
    }

    fn profile_with(keys: &str) -> Profile {
        let mut profile = Profile::new(&colemak(), "en", 1_000);
        profile.keys = strong(keys);
        profile
    }

    #[test]
    fn new_profile_unlocks_the_first_four_teaching_letters() {
        let profile = Profile::new(&colemak(), "en", 1_000);
        assert_eq!(profile.unlocked, chars("tnse"));
        assert_eq!(profile.layout, "colemak-dh");
        assert_eq!(profile.lang, "en");
        assert!(profile.keys.is_empty());
        assert_eq!(profile.lesson_index, 0);
        assert_eq!(profile.tests_completed, 0);
        assert_eq!(profile.unlocks_this_session, 0);
        assert_eq!(profile.session_started_ms, 1_000);
        assert_eq!(profile.last_unlock, None);
    }

    #[test]
    fn a_fresh_profile_is_blocked_for_low_confidence() {
        let profile = Profile::new(&colemak(), "en", 0);
        match profile.verdict(&colemak(), "en", &chars("tnse"), TARGET) {
            Verdict::Blocked(BlockReason::LowConfidence(c)) => assert!(c < 0.7, "{c}"),
            other => panic!("expected low confidence, got {other:?}"),
        }
    }

    #[test]
    fn strong_reachable_keys_unlock_the_next_letter() {
        let profile = profile_with("tnse");
        assert_eq!(
            profile.verdict(&colemak(), "en", &chars("tnse"), TARGET),
            Verdict::Unlock('r')
        );
    }

    #[test]
    fn two_dead_unreachable_keys_cannot_deadlock_progression() {
        // `t` and `e` are unlocked but the host keyboard cannot produce them, so
        // they have no stats at all. They must not be averaged in, or the mean
        // confidence of the remaining letters could never lift above the gate.
        let profile = profile_with("ns");
        assert!(!profile.keys.contains_key(&'t'));
        let reachable = chars("ns");
        let mean = profile
            .mean_confidence(&reachable, TARGET)
            .expect("two keys");
        assert!((mean - 1.0).abs() < 1e-5, "mean was {mean}");
        assert_eq!(
            profile.verdict(&colemak(), "en", &reachable, TARGET),
            Verdict::Unlock('r')
        );
    }

    #[test]
    fn a_sloppy_reachable_key_blocks_the_unlock() {
        let mut profile = profile_with("tns");
        profile.keys.insert(
            'e',
            KeyStat {
                hits: 9,
                misses: 1,
                shift_misses: 0,
                mean_interval_ms: 150.0,
            },
        );
        match profile.verdict(&colemak(), "en", &chars("tnse"), TARGET) {
            Verdict::Blocked(BlockReason::Accuracy(ch, acc)) => {
                assert_eq!(ch, 'e');
                assert!((acc - 0.9).abs() < 1e-5, "accuracy was {acc}");
            }
            other => panic!("expected an accuracy block, got {other:?}"),
        }
    }

    #[test]
    fn three_unlocks_exhaust_the_session_and_the_fourth_is_capped() {
        let mut profile = profile_with("tnse");
        let layout = colemak();
        let reachable = chars("tns");
        let mut chs = Vec::new();
        for (i, now) in [10, 20, 30].into_iter().enumerate() {
            let verdict = profile.verdict(&layout, "en", &reachable, TARGET);
            let event = profile
                .apply(verdict, now)
                .unwrap_or_else(|| panic!("unlock {i}"));
            assert_eq!(event.how, UnlockHow::Auto);
            assert_eq!(event.ts_ms, now);
            chs.push(event.ch);
        }
        assert_eq!(chs, vec!['r', 'i', 'a']);
        assert_eq!(profile.unlocks_this_session, 3);
        assert_eq!(
            profile.verdict(&layout, "en", &reachable, TARGET),
            Verdict::Blocked(BlockReason::SessionCap)
        );
        assert_eq!(
            profile.apply(Verdict::Blocked(BlockReason::SessionCap), 40),
            None
        );
    }

    #[test]
    fn thirty_idle_minutes_restore_the_cap() {
        let mut profile = profile_with("tns");
        profile.unlocks_this_session = 3;
        profile.session_started_ms = 1_000;
        profile.tick_session(1_000 + 29 * 60_000);
        assert_eq!(
            profile.unlocks_this_session, 3,
            "29 minutes is not idle yet"
        );
        assert_eq!(profile.session_started_ms, 1_000);
        profile.tick_session(1_000 + 31 * 60_000);
        assert_eq!(profile.unlocks_this_session, 0);
        assert_eq!(profile.session_started_ms, 1_000 + 31 * 60_000);
    }

    #[test]
    fn a_fully_unlocked_profile_has_no_next_key() {
        let mut profile = profile_with("tnseria");
        profile.unlocked = chars("tnseriao");
        assert_eq!(
            profile.verdict(&colemak(), "en", &chars("tnseriao"), TARGET),
            Verdict::Blocked(BlockReason::NoNextKey)
        );
    }

    #[test]
    fn absorb_folds_the_session_and_counts_the_test() {
        let mut profile = Profile::new(&colemak(), "en", 0);
        let mut per_key = BTreeMap::new();
        per_key.insert(
            't',
            KeyAgg {
                hits: 3,
                misses: 1,
                shift_misses: 2,
                mean_dt: 200,
            },
        );
        per_key.insert(
            'n',
            KeyAgg {
                hits: 2,
                misses: 0,
                shift_misses: 0,
                mean_dt: 0,
            },
        );
        profile.absorb(&SessionStats {
            per_key,
            ..SessionStats::default()
        });

        let t = profile.keys[&'t'];
        assert_eq!((t.hits, t.misses, t.shift_misses), (3, 1, 2));
        assert!((t.mean_interval_ms - 200.0).abs() < 1e-3, "{t:?}");
        let n = profile.keys[&'n'];
        assert_eq!((n.hits, n.misses), (2, 0));
        assert_eq!(n.mean_interval_ms, 0.0, "no interval data, no speed claim");
        assert_eq!(profile.tests_completed, 1);
    }

    #[test]
    fn a_shift_miss_does_not_sink_the_letters_accuracy() {
        let mut stat = KeyStat::default();
        for _ in 0..10 {
            stat.record(true, Some(150.0));
        }
        stat.record_shift_miss();
        let mut profile = Profile::new(&colemak(), "en", 0);
        profile.keys = [('t', stat), ('n', stat), ('s', stat), ('e', stat)]
            .into_iter()
            .collect();
        assert_eq!(
            profile.verdict(&colemak(), "en", &chars("tnse"), TARGET),
            Verdict::Unlock('r')
        );
    }

    #[test]
    fn save_then_load_round_trips_the_whole_profile() {
        let dir = std::env::temp_dir().join(format!(
            "keytle-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("nested/profiles/colemak-dh.en.json");
        let mut profile = profile_with("tnse");
        profile.apply(Verdict::Unlock('r'), 42);
        profile.lesson_index = 7;
        profile.tests_completed = 3;

        profile.save(&path).expect("save creates the parent dirs");
        let back = Profile::load(&path).expect("load");
        assert_eq!(back, profile);
        assert_eq!(back.last_unlock.expect("an unlock").ch, 'r');
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loading_a_missing_file_is_an_error_for_the_caller_to_judge() {
        let path = std::env::temp_dir().join("keytle-no-such-profile-xyz.json");
        assert!(Profile::load(&path).is_err());
    }
}
