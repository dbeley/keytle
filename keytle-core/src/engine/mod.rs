//! The progression engine: what the typist has earned, and what to practise next.
//!
//! The engine is pure and terminal-free — it turns keystrokes into state and state
//! into decisions, with no clock, no filesystem and no UI. Everything it needs from
//! the outside arrives as an argument, which is what makes the unlock rules
//! testable at the boundaries.
//!
//! Today it holds the per-key evidence, [`KeyStat`]: hits, misses, case mistakes
//! and a smoothed interval, from which
//! [`confidence`](KeyStat::confidence) is derived.
//!
//! `KeyStat` is keyed by the **lowercase** letter throughout, so `T` and `t` feed
//! one entry; a wrong case lands in [`shift_misses`](KeyStat::shift_misses) rather
//! than being charged to the letter.

pub mod key;

pub use key::KeyStat;
