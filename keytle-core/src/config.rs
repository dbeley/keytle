//! `config.toml`: the settings the trainer starts from.
//!
//! Every field is optional in the file, so a partial (or absent) config still
//! yields a usable setup. Unknown keys are rejected rather than ignored, so a
//! typo like `targt_wpm` fails loudly instead of being silently dropped.

use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What the drill is made of. Written lowercase in the file: `pseudo`, `words`
/// or `sentences`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Pseudo,
    Words,
    Sentences,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Pseudo => "pseudo",
            Mode::Words => "words",
            Mode::Sentences => "sentences",
        })
    }
}

/// The resolved settings. Missing keys fall back to [`Config::default`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub layout: String,
    pub host_layout: String,
    pub language: String,
    pub mode: Mode,
    pub emulate: bool,
    pub strict: bool,
    pub lesson_words: usize,
    pub target_wpm: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            layout: "colemak-dh".to_string(),
            host_layout: "qwerty".to_string(),
            language: "en".to_string(),
            mode: Mode::Pseudo,
            emulate: true,
            strict: true,
            lesson_words: 20,
            target_wpm: 80.0,
        }
    }
}

impl Config {
    /// Read the config at `path`. A file that does not exist is not an error:
    /// the defaults are returned instead.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// How long one keystroke may take to match the target speed, counting five
    /// characters per word.
    pub fn target_interval_ms(&self) -> f32 {
        60_000.0 / (5.0 * self.target_wpm)
    }
}

/// Why the config could not be loaded. Both cases name the file, and the parse
/// case carries the offending field and line.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("bad config in {path}: {source}", path = .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;

    /// A scratch config file named after the test, removed on drop.
    struct ScratchFile(PathBuf);

    impl ScratchFile {
        fn with(name: &str, contents: &str) -> ScratchFile {
            let path = env::temp_dir().join(format!("keytle-config-{}-{name}.toml", process::id()));
            fs::write(&path, contents).expect("write the scratch config");
            ScratchFile(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn defaults_are_a_pseudo_letter_drill_at_target_speed() {
        let config = Config::default();
        assert_eq!(config.layout, "colemak-dh");
        assert_eq!(config.host_layout, "qwerty");
        assert_eq!(config.language, "en");
        assert_eq!(config.mode, Mode::Pseudo);
        assert!(config.emulate);
        assert!(config.strict);
        assert_eq!(config.lesson_words, 20);
        assert_eq!(config.target_wpm, 80.0);
        assert_eq!(config.target_interval_ms(), 150.0);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let missing = env::temp_dir().join(format!("keytle-config-{}-absent.toml", process::id()));
        let _ = fs::remove_file(&missing);
        assert_eq!(
            Config::load(&missing).expect("a missing file falls back to the defaults"),
            Config::default()
        );
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let file = ScratchFile::with("partial", "layout = \"workman\"\n");
        let config = Config::load(file.path()).expect("a valid partial config");
        assert_eq!(config.layout, "workman");
        assert_eq!(config.host_layout, "qwerty");
        assert_eq!(config.lesson_words, 20);
        assert_eq!(config.target_wpm, 80.0);
    }

    #[test]
    fn a_slower_target_widens_the_interval() {
        let file = ScratchFile::with("wpm60", "target_wpm = 60.0\n");
        let config = Config::load(file.path()).expect("a valid config");
        assert_eq!(config.target_interval_ms(), 200.0);
    }

    #[test]
    fn mode_is_written_in_lowercase() {
        let file = ScratchFile::with("mode", "mode = \"words\"\n");
        let config = Config::load(file.path()).expect("a valid config");
        assert_eq!(config.mode, Mode::Words);
        assert_eq!(Mode::Sentences.to_string(), "sentences");
    }

    #[test]
    fn an_unknown_field_is_rejected_by_name() {
        let file = ScratchFile::with("unknown", "foo = 1\n");
        let err = Config::load(file.path()).expect_err("unknown fields are rejected");
        let message = err.to_string();
        assert!(message.contains("foo"), "{message}");
        assert!(
            message.contains(file.path().to_str().expect("a utf-8 scratch path")),
            "{message}"
        );
    }
}
