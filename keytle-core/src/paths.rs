//! Where keytle keeps its files.
//!
//! Resolution order per directory is: explicit `KEYTLE_*_DIR` override, then the
//! XDG variable plus `keytle`, then `$HOME/.config` / `$HOME/.local/share` plus
//! `keytle`. The override is taken verbatim so tests can point at a scratch dir.

use std::env;
use std::path::PathBuf;

/// Resolved locations of keytle's config and data files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    /// `<config_dir>/config.toml`
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// `<data_dir>/tests.jsonl`
    pub fn tests_file(&self) -> PathBuf {
        self.data_dir.join("tests.jsonl")
    }

    /// `<data_dir>/profiles/<layout>.<lang>.json`
    pub fn profile_file(&self, layout: &str, lang: &str) -> PathBuf {
        self.data_dir
            .join("profiles")
            .join(format!("{layout}.{lang}.json"))
    }
}

/// Resolve the directories keytle reads config from and writes data to.
pub fn paths() -> Paths {
    Paths {
        config_dir: dir("KEYTLE_CONFIG_DIR", "XDG_CONFIG_HOME", ".config"),
        data_dir: dir("KEYTLE_DATA_DIR", "XDG_DATA_HOME", ".local/share"),
    }
}

/// One directory, resolved from its override, its XDG variable, or `$HOME`.
fn dir(override_var: &str, xdg_var: &str, home_suffix: &str) -> PathBuf {
    if let Some(dir) = non_empty(override_var) {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = non_empty(xdg_var) {
        return PathBuf::from(xdg).join("keytle");
    }
    match non_empty("HOME") {
        Some(home) => PathBuf::from(home).join(home_suffix).join("keytle"),
        // No HOME at all: stay relative to the working directory rather than panic.
        None => PathBuf::from(home_suffix).join("keytle"),
    }
}

/// The value of `var`, unless it is unset or empty.
fn non_empty(var: &str) -> Option<std::ffi::OsString> {
    env::var_os(var).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        std::env::set_var("KEYTLE_DATA_DIR", "/tmp/kd");
        assert_eq!(paths().data_dir, std::path::PathBuf::from("/tmp/kd"));
        assert_eq!(
            paths().profile_file("colemak-dh", "fr"),
            std::path::PathBuf::from("/tmp/kd/profiles/colemak-dh.fr.json")
        );
    }

    #[test]
    fn file_helpers_hang_off_their_dir() {
        let paths = Paths {
            config_dir: PathBuf::from("/cfg"),
            data_dir: PathBuf::from("/data"),
        };
        assert_eq!(paths.config_file(), PathBuf::from("/cfg/config.toml"));
        assert_eq!(paths.tests_file(), PathBuf::from("/data/tests.jsonl"));
    }
}
