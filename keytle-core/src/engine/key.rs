//! One key's track record, and the confidence the progression engine reads.
//!
//! `KeyStat` is deliberately tiny and serde-friendly: it is what the profile
//! persists and what the unlock rule averages over. Two qualities are tracked
//! separately and then mixed, because they fix different problems - a key can be
//! hit reliably but slowly, or fast but sloppily:
//!
//! ```text
//! speed_c    = clamp(target_interval_ms / mean_interval_ms, 0, 1)
//! acc_c      = clamp((accuracy - 0.90) / 0.08, 0, 1)
//! confidence = 0.6 * speed_c + 0.4 * acc_c
//! ```
//!
//! Accuracy below 90 % earns nothing; accuracy reaches full marks at 98 %. Speed
//! reaches full marks at the target interval (≈150 ms ≈ 80 wpm) and decays
//! smoothly below it. Speed carries the larger weight because unlock progress is
//! about fluency, while accuracy has its own hard floor in the unlock rule.
//!
//! A key with no data must not look perfect, so an untouched stat reports
//! `accuracy() == 1.0` (nothing has gone wrong yet) but `confidence() == 0.0`
//! (nothing has gone right either). Both interval and accuracy are smoothed with
//! an EMA rather than a plain mean, so a single hitch cannot swing a key's
//! standing *and* an old one can be outgrown: `accuracy()` is the last
//! [`ACC_WINDOW`]-odd keystrokes, not the whole history. A lifetime ratio would
//! make a letter with a thousand hits and four early typos sit under the floor
//! for ever, however cleanly it is typed now.

use serde::{Deserialize, Serialize};

/// EMA weight for a new interval sample.
const ALPHA: f32 = 0.1;
/// Roughly how many keystrokes `accuracy()` remembers: `1 / ACC_ALPHA`.
const ACC_WINDOW: u32 = 20;
/// EMA weight for a new correctness sample.
const ACC_ALPHA: f32 = 1.0 / ACC_WINDOW as f32;
/// Accuracy that earns no credit and the width of the band up to full credit.
const ACC_FLOOR: f32 = 0.90;
const ACC_SPAN: f32 = 0.08;
/// Weights of speed and accuracy in the mix.
const SPEED_WEIGHT: f32 = 0.6;
const ACC_WEIGHT: f32 = 0.4;

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Accumulated evidence about one (lowercased) key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyStat {
    pub hits: u32,
    pub misses: u32,
    /// Wrong case where this key's character was expected: `T` for `t`.
    pub shift_misses: u32,
    /// EMA of the interval between consecutive correct keystrokes, in ms.
    pub mean_interval_ms: f32,
    /// EMA of right/wrong over the recent keystrokes, `None` until the first
    /// one after the field existed: a profile written before it seeds from the
    /// lifetime ratio, which is the only evidence there is.
    #[serde(default)]
    pub accuracy_ema: Option<f32>,
}

impl KeyStat {
    /// How accurate this key is *lately*, in `0.0..=1.0`: an EMA over roughly
    /// the last [`ACC_WINDOW`] keystrokes, so a clean run can outgrow old
    /// mistakes. `1.0` when the key has never been seen.
    pub fn accuracy(&self) -> f32 {
        if self.hits + self.misses == 0 {
            return 1.0;
        }
        self.accuracy_ema
            .unwrap_or_else(|| self.lifetime_accuracy())
    }

    /// `hits / (hits + misses)`: the whole history, kept for display and as the
    /// seed a profile written by an older build starts from.
    pub fn lifetime_accuracy(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            1.0
        } else {
            self.hits as f32 / total as f32
        }
    }

    /// How ready this key is to be unlocked / trusted, in `0.0..=1.0`.
    ///
    /// `0.0` until the key has been typed at least once: no data is not
    /// evidence of mastery.
    pub fn confidence(&self, target_interval_ms: f32) -> f32 {
        if self.hits + self.misses == 0 {
            return 0.0;
        }
        // No interval recorded yet means no speed data, not infinite speed.
        let speed_c = if self.mean_interval_ms > 0.0 {
            clamp01(target_interval_ms / self.mean_interval_ms)
        } else {
            0.0
        };
        let acc_c = clamp01((self.accuracy() - ACC_FLOOR) / ACC_SPAN);
        SPEED_WEIGHT * speed_c + ACC_WEIGHT * acc_c
    }

    /// Fold in one keystroke. `interval_ms` is the gap since the previous correct
    /// keystroke; callers pass `Some` only for hits, and `None` when the gap is
    /// unknown or a pause.
    pub fn record(&mut self, hit: bool, interval_ms: Option<f32>) {
        if hit {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        // Seed from the lifetime ratio when this is the first update after a
        // load: on a brand-new key the ratio *is* this sample, so the average
        // starts exact rather than at an invented 1.0.
        let previous = self
            .accuracy_ema
            .unwrap_or_else(|| self.lifetime_accuracy());
        let sample = if hit { 1.0 } else { 0.0 };
        self.accuracy_ema = Some((1.0 - ACC_ALPHA) * previous + ACC_ALPHA * sample);
        if let Some(dt) = interval_ms {
            self.mean_interval_ms = if self.mean_interval_ms > 0.0 {
                (1.0 - ALPHA) * self.mean_interval_ms + ALPHA * dt
            } else {
                dt // first sample seeds the EMA
            };
        }
    }

    /// A wrong case was typed where this key was expected. It is tracked
    /// separately and leaves `hits`/`misses` alone: the letter was known, the
    /// modifier was not, and blaming the key would hide that distinction.
    pub fn record_shift_miss(&mut self) {
        self.shift_misses += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    fn stat(hits: u32, misses: u32, mean_interval_ms: f32) -> KeyStat {
        KeyStat {
            hits,
            misses,
            shift_misses: 0,
            mean_interval_ms,
            accuracy_ema: None,
        }
    }

    #[test]
    fn untouched_key_is_accurate_but_not_confident() {
        let s = KeyStat::default();
        assert!(close(s.accuracy(), 1.0));
        assert!(close(s.confidence(150.0), 0.0));
    }

    #[test]
    fn on_target_and_perfect_is_full_confidence() {
        let s = stat(10, 0, 150.0);
        assert!(close(s.confidence(150.0), 1.0));
    }

    #[test]
    fn half_the_target_speed_costs_only_the_speed_half() {
        // speed_c = 150/300 = 0.5, acc_c = 1.0 → 0.6*0.5 + 0.4*1.0
        let s = stat(10, 0, 300.0);
        assert!(close(s.confidence(150.0), 0.7));
    }

    #[test]
    fn faster_than_target_is_clamped() {
        let s = stat(10, 0, 50.0);
        assert!(close(s.confidence(150.0), 1.0));
    }

    #[test]
    fn accuracy_floor_and_midpoint() {
        // 90 % accuracy earns no accuracy credit: 0.6*1.0.
        let s = stat(9, 1, 150.0);
        assert!(close(s.accuracy(), 0.9));
        assert!(close(s.confidence(150.0), 0.6));
        // 94 % sits halfway up the band: 0.6*1.0 + 0.4*0.5.
        let s = stat(94, 6, 150.0);
        assert!(close(s.accuracy(), 0.94));
        assert!(close(s.confidence(150.0), 0.8));
    }

    #[test]
    fn accuracy_below_the_floor_earns_no_acc_credit() {
        let s = stat(85, 15, 150.0);
        assert!(close(s.confidence(150.0), 0.6));
    }

    #[test]
    fn accuracy_above_the_band_is_clamped_to_full() {
        let s = stat(99, 1, 150.0);
        assert!(close(s.confidence(150.0), 1.0));
    }

    #[test]
    fn hits_without_an_interval_have_no_speed_credit() {
        let s = stat(3, 0, 0.0);
        assert!(close(s.confidence(150.0), 0.4));
    }

    #[test]
    fn confidence_is_bounded() {
        for (hits, misses, mean) in [(0, 3, 0.0), (1, 1, 900.0), (1000, 0, 1.0)] {
            let c = stat(hits, misses, mean).confidence(150.0);
            assert!((0.0..=1.0).contains(&c), "confidence {c} out of range");
        }
    }

    #[test]
    fn record_counts_both_outcomes() {
        let mut s = KeyStat::default();
        s.record(true, None);
        s.record(false, None);
        s.record(false, None);
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 2);
        assert!(close(s.lifetime_accuracy(), 1.0 / 3.0));
        // accuracy() is the recent window: 1.0, then 0.95, then 0.9025.
        assert!(close(s.accuracy(), 0.95 * 0.95));
    }

    #[test]
    fn accuracy_is_the_recent_window_not_the_lifetime_ratio() {
        let mut s = KeyStat::default();
        for _ in 0..20 {
            s.record(true, None);
        }
        for _ in 0..10 {
            s.record(false, None);
        }
        assert!(s.lifetime_accuracy() < 0.7, "the history really is poor");
        assert!(s.accuracy() < 0.7, "and recent typing still reflects it");
        // Sixty clean keystrokes later the window has forgotten the bad run,
        // long before the lifetime ratio could climb.
        for _ in 0..60 {
            s.record(true, None);
        }
        assert!(
            s.lifetime_accuracy() < 0.90,
            "the lifetime ratio is still under the floor: {}",
            s.lifetime_accuracy()
        );
        assert!(
            s.accuracy() > 0.97,
            "but the recent window is clean: {}",
            s.accuracy()
        );
    }

    #[test]
    fn a_stat_without_the_recent_accuracy_seeds_from_the_ratio() {
        // The shape an older keytle wrote, before `accuracy_ema`.
        let json = r#"{"hits":9,"misses":1,"shift_misses":0,"mean_interval_ms":150.0}"#;
        let mut stat: KeyStat = serde_json::from_str(json).expect("the old shape parses");
        assert!(stat.accuracy_ema.is_none());
        assert!(close(stat.accuracy(), 0.9), "seeded from hits/misses");
        stat.record(true, None);
        assert!(stat.accuracy_ema.is_some(), "the first stroke starts it");
        assert!(stat.accuracy() > 0.9, "and it moves with the evidence");
    }

    #[test]
    fn interval_ema_uses_alpha_point_one() {
        let mut s = KeyStat::default();
        s.record(true, Some(400.0));
        assert!(
            close(s.mean_interval_ms, 400.0),
            "first sample seeds the EMA"
        );
        s.record(true, Some(100.0));
        assert!(close(s.mean_interval_ms, 370.0));
        s.record(true, Some(100.0));
        assert!(close(s.mean_interval_ms, 343.0));
    }

    #[test]
    fn a_miss_without_an_interval_leaves_the_speed_alone() {
        let mut s = KeyStat::default();
        s.record(true, Some(200.0));
        s.record(false, None);
        assert!(close(s.mean_interval_ms, 200.0));
    }

    #[test]
    fn shift_miss_bumps_only_its_own_counter() {
        let mut s = stat(5, 1, 150.0);
        s.record_shift_miss();
        assert_eq!(s.shift_misses, 1);
        assert_eq!(s.hits, 5);
        // The letter was known, so the key's own counters are untouched.
        assert_eq!(s.misses, 1);
        assert!(close(s.accuracy(), 5.0 / 6.0));
    }

    #[test]
    fn survives_a_toml_round_trip() {
        // The profile persists these, so the serde shape is part of the contract.
        let original = stat(7, 2, 143.5);
        let text = toml::to_string(&original).expect("serialise");
        assert!(text.contains("shift_misses"), "{text}");
        let back: KeyStat = toml::from_str(&text).expect("deserialise");
        assert_eq!(back, original);
    }
}
