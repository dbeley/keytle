//! The profile: what the typist has unlocked, and the rule that unlocks more.
//!
//! A profile is the durable half of the progression engine. It remembers which
//! letters are unlocked, the evidence accumulated per key, and how many tests have
//! been finished, so the app can be closed at any moment and resume exactly where
//! it stopped.
//!
//! The unlock rule is doubly gated:
//!
//! * the mean [`confidence`](KeyStat::confidence) of the **reachable** unlocked
//!   keys must reach 0.6, and
//! * no reachable unlocked key may sit below 0.9 accuracy,
//!
//! with no ceiling on how many letters one session may unlock: a typist who
//! already knows the layout should be able to climb to their real level in an
//! afternoon rather than three letters at a time. Keys the host keyboard cannot
//! produce are *excluded* from both checks: an emulated layout may unlock a
//! letter the host cannot type at all, and leaving it in the average would
//! deadlock progression forever.
//!
//! Serialisation is JSON: a profile is small, human-inspectable and versioned by
//! its own field set.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::key::KeyStat;
use crate::layout::Layout;
use crate::typing::SessionStats;

/// Letters unlocked on a brand-new profile.
const INITIAL_UNLOCKED: usize = 4;
/// Idle time after which the session counter restarts.
pub const SESSION_IDLE_MS: u64 = 30 * 60 * 1000;
/// Mean confidence the reachable unlocked keys must reach.
pub const UNLOCK_CONFIDENCE: f32 = 0.6;
/// Hard accuracy floor: below this, nothing unlocks.
pub const UNLOCK_ACCURACY: f32 = 0.90;
/// The rung after the letters: punctuation prose needs, whatever the language.
const PUNCTUATION: &[char] = &['\'', ',', '.', ';', ':', '!', '?'];
/// The rung after the punctuation for French: its accented letters.
const FRENCH_EXTRAS: &[char] = &['é', 'à', 'è', 'ç', 'ù', 'ê', 'î', 'ô', 'û'];

/// How an unlock came about.
///
/// Every unlock the trainer makes is [`UnlockHow::Auto`]: there is no manual
/// route through the rule. The other two variants are kept so a profile written
/// by an older build (which had `u` unlock and `l` pin bindings) still reads;
/// nothing writes them any more.
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
    ///
    /// The write is atomic: the JSON goes to `<path>.tmp` in the same directory,
    /// is flushed to disk, and only then renamed over the target. A crash
    /// mid-write therefore leaves the old profile intact - a truncated durable
    /// profile would lose every unlocked key.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        // An existing tmp file from an interrupted save is simply overwritten.
        let write = || -> io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        };
        if let Err(err) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
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
        let Some(next) = self.next_key(layout, lang, reachable) else {
            return Verdict::Blocked(BlockReason::NoNextKey);
        };
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

    /// The next key of the teaching order this profile has not unlocked, or the
    /// next rung once the letters are done.
    ///
    /// The letters come from the layout's `unlock_order`. When every one of them
    /// is unlocked the rung after them is the punctuation prose needs, then the
    /// accented letters French needs; a character the layout has no key for, or
    /// the host keyboard cannot press, is skipped rather than blocking the rung.
    pub fn next_key(
        &self,
        layout: &Layout,
        lang: &str,
        reachable: &BTreeSet<char>,
    ) -> Option<char> {
        if let Some(ch) = layout
            .unlock_order_for(lang)
            .into_iter()
            .find(|ch| !self.unlocked.contains(ch))
        {
            return Some(ch);
        }
        let extras: &[char] = if lang == "fr" { FRENCH_EXTRAS } else { &[] };
        PUNCTUATION.iter().chain(extras.iter()).copied().find(|ch| {
            !self.unlocked.contains(ch) && reachable.contains(ch) && layout.contains_char(*ch)
        })
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
        // The order is frequency-first: the four most common English letters
        // that Colemak-DH can type, so real words form from lesson one.
        assert_eq!(profile.unlocked, chars("etao"));
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
            Verdict::Blocked(BlockReason::LowConfidence(c)) => {
                assert!(c < UNLOCK_CONFIDENCE, "{c}");
            }
            other => panic!("expected low confidence, got {other:?}"),
        }
    }

    #[test]
    fn strong_reachable_keys_unlock_the_next_letter() {
        let profile = profile_with("tnse");
        assert_eq!(
            profile.verdict(&colemak(), "en", &chars("tnse"), TARGET),
            Verdict::Unlock('i')
        );
    }

    #[test]
    fn two_dead_unreachable_keys_cannot_deadlock_progression() {
        // `t` and `o` are unlocked but the host keyboard cannot produce them, so
        // they have no stats at all. They must not be averaged in, or the mean
        // confidence of the remaining letters could never lift above the gate.
        let profile = profile_with("ea");
        assert!(!profile.keys.contains_key(&'t'));
        let reachable = chars("ea");
        let mean = profile
            .mean_confidence(&reachable, TARGET)
            .expect("two keys");
        assert!((mean - 1.0).abs() < 1e-5, "mean was {mean}");
        assert_eq!(
            profile.verdict(&colemak(), "en", &reachable, TARGET),
            Verdict::Unlock('i')
        );
    }

    #[test]
    fn a_sloppy_reachable_key_blocks_the_unlock() {
        let mut profile = profile_with("tea");
        profile.keys.insert(
            'e',
            KeyStat {
                hits: 8,
                misses: 2,
                shift_misses: 0,
                mean_interval_ms: 150.0,
            },
        );
        match profile.verdict(&colemak(), "en", &chars("tea"), TARGET) {
            Verdict::Blocked(BlockReason::Accuracy(ch, acc)) => {
                assert_eq!(ch, 'e');
                assert!((acc - 0.8).abs() < 1e-5, "accuracy was {acc}");
            }
            other => panic!("expected an accuracy block, got {other:?}"),
        }
    }

    #[test]
    fn unlocks_keep_coming_past_three_in_one_session() {
        let mut profile = profile_with("tns");
        let layout = colemak();
        let reachable = chars("tns");
        let mut chs = Vec::new();
        for (i, now) in [10, 20, 30, 40].into_iter().enumerate() {
            let verdict = profile.verdict(&layout, "en", &reachable, TARGET);
            let event = profile
                .apply(verdict, now)
                .unwrap_or_else(|| panic!("unlock {i} was refused: {verdict:?}"));
            assert_eq!(event.how, UnlockHow::Auto);
            assert_eq!(event.ts_ms, now);
            chs.push(event.ch);
        }
        assert_eq!(chs, vec!['i', 'n', 's', 'h'], "straight down the order");
        assert_eq!(
            profile.unlocks_this_session, 4,
            "the count is a statistic now"
        );
    }

    #[test]
    fn thirty_idle_minutes_restart_the_session_counter() {
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
        let layout = colemak();
        let mut profile = profile_with("tnseria");
        profile.unlocked = layout.unlock_order_for("en").into_iter().collect();
        let reachable: BTreeSet<char> = profile
            .unlocked
            .iter()
            .copied()
            .chain(PUNCTUATION.iter().copied())
            .collect();
        // The letters are done, but the punctuation rung is still to come.
        assert!(profile.next_key(&layout, "en", &reachable).is_some());
        profile.unlocked.extend(
            PUNCTUATION
                .iter()
                .copied()
                .filter(|ch| layout.contains_char(*ch) && reachable.contains(ch)),
        );
        assert_eq!(
            profile.verdict(&layout, "en", &reachable, TARGET),
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
            Verdict::Unlock('i')
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "keytle-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn tmp_path(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".tmp");
        PathBuf::from(name)
    }

    #[test]
    fn the_punctuation_rung_follows_the_letters() {
        let layout = colemak();
        let mut set: Vec<char> = layout.unlock_order_for("en");
        assert_eq!(
            set.len(),
            layout
                .keys
                .values()
                .filter(|key| key.base.is_alphabetic())
                .count(),
            "the order covers every letter the board can type"
        );
        let mut profile = profile_with(&set.iter().collect::<String>());
        profile.unlocked = set.iter().copied().collect();
        let reachable: BTreeSet<char> = set
            .iter()
            .copied()
            .chain(PUNCTUATION.iter().copied())
            .collect();

        assert_eq!(
            profile.verdict(&layout, "en", &reachable, TARGET),
            Verdict::Unlock('\'')
        );
        assert_eq!(
            profile.apply(Verdict::Unlock('\''), 5).expect("unlock").ch,
            '\''
        );
        profile.keys.extend(strong("'"));
        set.push('\'');
        assert_eq!(
            profile.verdict(&layout, "en", &reachable, TARGET),
            Verdict::Unlock(',')
        );
    }

    #[test]
    fn the_french_rung_unlocks_the_extras_in_order() {
        let layout = colemak();
        let extras: Vec<char> = FRENCH_EXTRAS
            .iter()
            .copied()
            .filter(|ch| layout.contains_char(*ch))
            .collect();
        assert_eq!(extras, vec!['é', 'ç'], "the bundled layout has two of them");
        let mut set: Vec<char> = layout.unlock_order_for("fr");
        set.extend(PUNCTUATION.iter().copied());
        let mut profile = profile_with(&set.iter().collect::<String>());
        profile.unlocked = set.iter().copied().collect();
        let reachable: BTreeSet<char> = set.iter().copied().chain(extras.iter().copied()).collect();

        for (i, expected) in extras.iter().copied().enumerate() {
            assert_eq!(
                profile.verdict(&layout, "fr", &reachable, TARGET),
                Verdict::Unlock(expected),
                "extra {i}"
            );
            profile.apply(Verdict::Unlock(expected), i as u64);
            profile.keys.extend(strong(&expected.to_string()));
        }
        assert_eq!(
            profile.verdict(&layout, "fr", &reachable, TARGET),
            Verdict::Blocked(BlockReason::NoNextKey),
            "the layout cannot type the rest, so the rung ends"
        );
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp_behind() {
        let dir = scratch_dir("profile-atomic");
        let path = dir.join("colemak-dh.en.json");
        let mut profile = profile_with("tnseriao");
        profile.unlocked = chars("tnseriao");
        profile.save(&path).expect("first save");
        let good = Profile::load(&path).expect("load");

        // A truncated tmp file left by an interrupted save must not be trusted or
        // appended to: the next save replaces it and the target stays valid.
        std::fs::write(tmp_path(&path), "{\"truncated\":").expect("plant a tmp");
        profile.lesson_index = 42;
        profile.save(&path).expect("second save");

        let back = Profile::load(&path).expect("the profile is still valid JSON");
        assert_eq!(back.lesson_index, 42);
        assert_eq!(back.unlocked, good.unlocked);
        assert!(
            !tmp_path(&path).exists(),
            "a .tmp file was left behind by {}",
            path.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
