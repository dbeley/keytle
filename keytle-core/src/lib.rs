//! Core of keytle: layouts, corpora, the progression engine and the stats store.
//!
//! Everything here is terminal-free and pure so it can be tested without a TTY.

pub mod config;
pub mod corpora;
pub mod corpus;
pub mod emulate;
pub mod engine;
pub mod layout;
pub mod layouts;
pub mod paths;
pub mod typing;
