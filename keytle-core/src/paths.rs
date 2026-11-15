//! Where keytle keeps its files.
//!
//! Resolution order per directory is: explicit `KEYTLE_*_DIR` override, then the
//! XDG variable plus `keytle`, then `$HOME/.config` / `$HOME/.local/share` plus
//! `keytle`. The override is taken verbatim so tests can point at a scratch dir.
//!
//! When neither `HOME` nor an override/XDG variable is set, the XDG suffix is
//! joined to the current working directory, so the result is still an absolute
//! path (unless the working directory itself is gone, in which case `.` is used).

use std::env;
use std::ffi::OsString;
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

    /// `<config_dir>/layouts`, where user layout files live.
    pub fn layouts_dir(&self) -> PathBuf {
        self.config_dir.join("layouts")
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

    /// Resolve the directories from `get` instead of the process environment.
    ///
    /// `get` is asked for each variable name and returns `None` when it is unset;
    /// empty values are treated as unset. `paths()` is this with `env::var_os`,
    /// which lets precedence be tested without mutating global process state.
    fn from_getter(get: impl Fn(&str) -> Option<OsString>) -> Paths {
        Paths {
            config_dir: dir(&get, "KEYTLE_CONFIG_DIR", "XDG_CONFIG_HOME", ".config"),
            data_dir: dir(&get, "KEYTLE_DATA_DIR", "XDG_DATA_HOME", ".local/share"),
        }
    }
}

/// Resolve the directories keytle reads config from and writes data to.
pub fn paths() -> Paths {
    Paths::from_getter(|key| env::var_os(key))
}

/// One directory, resolved from its override, its XDG variable, or `$HOME`.
fn dir(
    get: &impl Fn(&str) -> Option<OsString>,
    override_var: &str,
    xdg_var: &str,
    home_suffix: &str,
) -> PathBuf {
    let non_empty = |var: &str| get(var).filter(|value| !value.is_empty());
    if let Some(dir) = non_empty(override_var) {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = non_empty(xdg_var) {
        return PathBuf::from(xdg).join("keytle");
    }
    // No HOME at all: stay absolute by anchoring on the working directory rather
    // than returning a path that silently resolves relative to wherever we run.
    let base = match non_empty("HOME") {
        Some(home) => PathBuf::from(home),
        None => env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    base.join(home_suffix).join("keytle")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Resolve against a fixed set of "environment" variables.
    fn resolve(vars: &[(&str, &str)]) -> Paths {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(key, value)| (key.to_string(), OsString::from(value)))
            .collect();
        Paths::from_getter(|key| vars.get(key).cloned())
    }

    #[test]
    fn env_override_wins() {
        let dirs = resolve(&[
            ("KEYTLE_CONFIG_DIR", "/tmp/kc"),
            ("KEYTLE_DATA_DIR", "/tmp/kd"),
            ("XDG_CONFIG_HOME", "/tmp/xdg-config"),
            ("XDG_DATA_HOME", "/tmp/xdg-data"),
            ("HOME", "/tmp/home"),
        ]);
        assert_eq!(dirs.config_dir, PathBuf::from("/tmp/kc"));
        assert_eq!(dirs.data_dir, PathBuf::from("/tmp/kd"));
        assert_eq!(
            dirs.profile_file("colemak-dh", "fr"),
            PathBuf::from("/tmp/kd/profiles/colemak-dh.fr.json")
        );
    }

    #[test]
    fn xdg_wins_over_home() {
        let dirs = resolve(&[
            ("XDG_CONFIG_HOME", "/tmp/xdg-config"),
            ("XDG_DATA_HOME", "/tmp/xdg-data"),
            ("HOME", "/tmp/home"),
        ]);
        assert_eq!(dirs.config_dir, PathBuf::from("/tmp/xdg-config/keytle"));
        assert_eq!(dirs.data_dir, PathBuf::from("/tmp/xdg-data/keytle"));
    }

    #[test]
    fn home_fallback_when_xdg_unset() {
        let dirs = resolve(&[
            ("HOME", "/tmp/home"),
            ("XDG_CONFIG_HOME", ""),
            ("XDG_DATA_HOME", ""),
        ]);
        assert_eq!(dirs.config_dir, PathBuf::from("/tmp/home/.config/keytle"));
        assert_eq!(
            dirs.data_dir,
            PathBuf::from("/tmp/home/.local/share/keytle")
        );
    }

    #[test]
    fn no_home_stays_absolute_from_the_working_directory() {
        let dirs = resolve(&[]);
        let cwd = env::current_dir().expect("a working directory");
        assert_eq!(dirs.config_dir, cwd.join(".config/keytle"));
        assert_eq!(dirs.data_dir, cwd.join(".local/share/keytle"));
        assert!(dirs.config_dir.is_absolute());
        assert!(dirs.data_dir.is_absolute());
    }

    #[test]
    fn file_helpers_hang_off_their_dir() {
        let paths = Paths {
            config_dir: PathBuf::from("/cfg"),
            data_dir: PathBuf::from("/data"),
        };
        assert_eq!(paths.config_file(), PathBuf::from("/cfg/config.toml"));
        assert_eq!(paths.tests_file(), PathBuf::from("/data/tests.jsonl"));
        assert_eq!(paths.layouts_dir(), PathBuf::from("/cfg/layouts"));
    }
}
