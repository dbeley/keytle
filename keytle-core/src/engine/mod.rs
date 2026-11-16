//! The progression engine: what the typist has earned, and what to practise next.
//!
//! The engine is pure and terminal-free - it turns keystrokes into state and state
//! into decisions, with no clock, no filesystem and no UI. Everything it needs from
//! the outside arrives as an argument, which is what makes the unlock rules
//! testable at the boundaries.
//!
//! It holds three things:
//!
//! * the per-key evidence, [`KeyStat`]: hits, misses, case mistakes and a smoothed
//!   interval, from which [`confidence`](KeyStat::confidence) is derived;
//! * the [`Profile`]: what is unlocked, the accumulated evidence, and the unlock
//!   rule with its session cap;
//! * the lesson [`generate`]r: the text to practise next, built from the letters
//!   the typist has unlocked and can reach, weighted towards the shaky ones.
//!
//! `KeyStat` is keyed by the **lowercase** letter throughout, so `T` and `t` feed
//! one entry; a wrong case lands in [`shift_misses`](KeyStat::shift_misses) rather
//! than being charged to the letter.

pub mod generate;
pub mod key;
pub mod profile;

pub use generate::{generate, Lesson, Note};
pub use key::KeyStat;
pub use profile::{BlockReason, Profile, ProfileError, UnlockEvent, UnlockHow, Verdict};
