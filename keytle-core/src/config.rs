//! `config.toml`: the settings the trainer starts from.
//!
//! Every field is optional in the file, so a partial (or absent) config still
//! yields a usable setup. Unknown keys are rejected rather than ignored, so a
//! typo like `targt_wpm` fails loudly instead of being silently dropped.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The slowest and fastest speed a config may ask for, in words per minute.
/// Below the floor the target interval runs away; above the ceiling it is
/// untouchable, so both are treated as a typo rather than a wish.
pub const MIN_TARGET_WPM: f32 = 10.0;
pub const MAX_TARGET_WPM: f32 = 250.0;

/// The shortest and longest lesson a config may ask for. Zero would generate an
/// empty lesson and leave the drill with nothing to type.
pub const MIN_LESSON_WORDS: usize = 1;
pub const MAX_LESSON_WORDS: usize = 200;

/// What the drill is made of. Written lowercase in the file: `pseudo`, `words`
/// or `sentences`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Start the next lesson automatically once a results screen has been left
    /// alone for [`AUTO_NEXT_MS`](crate::config::AUTO_NEXT_MS).
    pub auto_next: bool,
}

/// How long a results screen waits, untouched, before the next lesson starts.
/// Lengthy enough to read the three numbers, short enough that a rhythm of
/// short bursts is not broken by reaching for `enter`.
pub const AUTO_NEXT_MS: u64 = 3_000;

impl Default for Config {
    fn default() -> Self {
        Config {
            layout: "colemak-dh".to_string(),
            host_layout: "qwerty".to_string(),
            language: "en".to_string(),
            // Real words from the bundled dictionary, never generated text, is
            // what a lesson is made of unless the learner asks for otherwise.
            mode: Mode::Words,
            emulate: true,
            strict: true,
            lesson_words: 20,
            target_wpm: 80.0,
            auto_next: true,
        }
    }
}

impl Config {
    /// Read the config at `path`, clamped to the ranges in [`Config::clamp`].
    /// A file that does not exist is not an error: the defaults are returned
    /// instead. The adjustments a clamp made are dropped; use
    /// [`Config::load_reporting`] to hear about them.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        Self::load_reporting(path).map(|(config, _adjusted)| config)
    }

    /// Read the config at `path` and say which values had to be clamped, so a
    /// caller can tell the typist that the file and the trainer now differ.
    ///
    /// Each entry reads `field old → new`, e.g. `target_wpm 0 → 10`.
    pub fn load_reporting(path: &Path) -> Result<(Config, Vec<String>), ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok((Config::default(), Vec::new()))
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let mut config: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let adjusted = config.clamp();
        Ok((config, adjusted))
    }

    /// Force every setting into a range the trainer can honour, describing each
    /// value that changed.
    ///
    /// A `target_wpm` of 0 makes [`target_interval_ms`](Config::target_interval_ms)
    /// divide by zero, and `lesson_words` of 0 makes `generate` hand back an
    /// empty lesson. A hand-edited config is repaired here rather than left to
    /// break the drill; the report is what lets the screen say so.
    pub fn clamp(&mut self) -> Vec<String> {
        let mut adjusted = Vec::new();
        let wpm = if self.target_wpm.is_nan() {
            MIN_TARGET_WPM
        } else {
            self.target_wpm.clamp(MIN_TARGET_WPM, MAX_TARGET_WPM)
        };
        if wpm != self.target_wpm {
            adjusted.push(format!(
                "target_wpm {} → {}",
                show_f32(self.target_wpm),
                show_f32(wpm)
            ));
            self.target_wpm = wpm;
        }
        let words = self.lesson_words.clamp(MIN_LESSON_WORDS, MAX_LESSON_WORDS);
        if words != self.lesson_words {
            adjusted.push(format!("lesson_words {} → {words}", self.lesson_words));
            self.lesson_words = words;
        }
        adjusted
    }

    /// Write every field to `path` as TOML, creating the directory if needed.
    ///
    /// Every field is written, not just the ones that differ from the defaults:
    /// a saved config is a complete statement of what the trainer was set to,
    /// so reading it back cannot depend on what the defaults happened to be.
    ///
    /// The write is atomic, the way the profile's is: the TOML goes to
    /// `<path>.tmp` in the same directory, is flushed to disk, and only then is
    /// renamed over the target. Truncating in place would leave a crash with a
    /// half-written file - that is, with no settings at all.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string(self).map_err(|err| ConfigError::Encode {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        }
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        // An existing tmp left by an interrupted save is simply overwritten.
        let write = || -> io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        };
        if let Err(source) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(ConfigError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        fs::rename(&tmp, path)
            .inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
            })
            .map_err(|source| ConfigError::Write {
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

/// One line naming every setting a clamp changed, for the screen to show.
pub fn adjusted_notice(adjusted: &[String]) -> Option<String> {
    (!adjusted.is_empty()).then(|| format!("settings adjusted: {}", adjusted.join(", ")))
}

/// A number as a person writes it: `10`, not `10.0`, and `NaN` rather than a
/// panic when the file held something absurd.
fn show_f32(value: f32) -> String {
    if value.is_finite() && value == value.trunc() && value.abs() < 1e9 {
        format!("{value:.0}")
    } else {
        format!("{value}")
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
    #[error("cannot write {path}: {source}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot encode config for {path}: {reason}", path = .path.display())]
    Encode { path: PathBuf, reason: String },
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
    fn defaults_are_a_real_word_drill_at_target_speed() {
        let config = Config::default();
        assert_eq!(config.layout, "colemak-dh");
        assert_eq!(config.host_layout, "qwerty");
        assert_eq!(config.language, "en");
        assert_eq!(config.mode, Mode::Words, "lessons use the real dictionary");
        assert!(config.auto_next, "the next lesson starts on its own");
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
    fn a_saved_config_reads_back_as_the_same_values() {
        let path = env::temp_dir().join(format!("keytle-config-{}-saved.toml", process::id()));
        let original = Config {
            layout: "bepo".to_string(),
            host_layout: "azerty".to_string(),
            language: "fr".to_string(),
            mode: Mode::Sentences,
            emulate: false,
            strict: false,
            lesson_words: 35,
            target_wpm: 62.0,
            auto_next: false,
        };
        original.save(&path).expect("save");
        assert_eq!(
            Config::load(&path).expect("reload"),
            original,
            "every field survives the round trip"
        );
        // The directory is created, so a first save from a fresh install works.
        let nested = path.with_file_name("nested/config.toml");
        original.save(&nested).expect("save into a new directory");
        assert!(nested.is_file());
        let _ = fs::remove_dir_all(path.with_file_name("nested"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_zero_target_wpm_is_clamped_to_the_floor_and_reported() {
        let file = ScratchFile::with("wpm0", "target_wpm = 0.0\n");
        let (config, adjusted) = Config::load_reporting(file.path()).expect("a valid config");
        assert_eq!(config.target_wpm, MIN_TARGET_WPM);
        assert_eq!(adjusted, vec!["target_wpm 0 → 10".to_string()]);
        assert_eq!(
            config.target_interval_ms(),
            60_000.0 / (5.0 * MIN_TARGET_WPM),
            "the interval is finite, not a division by zero"
        );
        assert!(config.target_interval_ms().is_finite());
        assert_eq!(
            Config::load(file.path()).unwrap(),
            config,
            "load clamps too"
        );
        assert_eq!(
            adjusted_notice(&adjusted).as_deref(),
            Some("settings adjusted: target_wpm 0 → 10")
        );
    }

    #[test]
    fn a_target_wpm_above_the_ceiling_is_clamped_to_it() {
        let file = ScratchFile::with("wpm999", "target_wpm = 999.0\n");
        let (config, adjusted) = Config::load_reporting(file.path()).expect("a valid config");
        assert_eq!(config.target_wpm, MAX_TARGET_WPM);
        assert_eq!(adjusted, vec!["target_wpm 999 → 250".to_string()]);
    }

    #[test]
    fn a_zero_lesson_length_is_clamped_to_one_word_and_reported() {
        let file = ScratchFile::with("words0", "lesson_words = 0\n");
        let (config, adjusted) = Config::load_reporting(file.path()).expect("a valid config");
        assert_eq!(config.lesson_words, MIN_LESSON_WORDS);
        assert_eq!(adjusted, vec!["lesson_words 0 → 1".to_string()]);
    }

    #[test]
    fn a_lesson_longer_than_the_ceiling_is_clamped_to_it() {
        let file = ScratchFile::with("words999", "lesson_words = 999\n");
        let (config, adjusted) = Config::load_reporting(file.path()).expect("a valid config");
        assert_eq!(config.lesson_words, MAX_LESSON_WORDS);
        assert_eq!(adjusted, vec!["lesson_words 999 → 200".to_string()]);
    }

    #[test]
    fn a_value_inside_the_range_passes_through_untouched() {
        let file = ScratchFile::with("inrange", "target_wpm = 62.0\nlesson_words = 35\n");
        let (config, adjusted) = Config::load_reporting(file.path()).expect("a valid config");
        assert_eq!(config.target_wpm, 62.0);
        assert_eq!(config.lesson_words, 35);
        assert!(
            adjusted.is_empty(),
            "nothing was changed, so nothing is said"
        );
        assert_eq!(adjusted_notice(&adjusted), None);
        let defaults = Config::default();
        assert!(defaults.target_wpm >= MIN_TARGET_WPM && defaults.target_wpm <= MAX_TARGET_WPM);
        assert!(
            defaults.lesson_words >= MIN_LESSON_WORDS && defaults.lesson_words <= MAX_LESSON_WORDS
        );
    }

    #[test]
    fn a_degenerate_config_breaks_neither_the_interval_nor_the_lesson() {
        // Both at once, as a hand-edited file could easily have them.
        let mut config = Config {
            target_wpm: 0.0,
            lesson_words: 0,
            ..Config::default()
        };
        let adjusted = config.clamp();
        assert_eq!(adjusted.len(), 2, "{adjusted:?}");
        assert_eq!(config.target_wpm, MIN_TARGET_WPM);
        assert_eq!(config.lesson_words, MIN_LESSON_WORDS);
        assert!(config.target_interval_ms().is_finite());
    }

    #[test]
    fn the_clamp_is_idempotent_so_it_never_reports_twice() {
        let mut config = Config {
            target_wpm: 0.0,
            lesson_words: 0,
            ..Config::default()
        };
        assert_eq!(config.clamp().len(), 2);
        assert!(
            config.clamp().is_empty(),
            "already in range, nothing to say"
        );
    }

    #[test]
    fn a_nan_target_wpm_is_repaired_rather_than_panicking() {
        let mut config = Config {
            target_wpm: f32::NAN,
            ..Config::default()
        };
        let adjusted = config.clamp();
        assert_eq!(config.target_wpm, MIN_TARGET_WPM);
        assert_eq!(adjusted, vec!["target_wpm NaN → 10".to_string()]);
    }

    #[test]
    fn an_existing_config_survives_a_save_and_no_tmp_is_left_behind() {
        let dir = env::temp_dir().join(format!("keytle-config-{}-atomic", process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        let first = Config::default();
        first.save(&path).expect("first save");
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);

        // A tmp left by an interrupted save must not be trusted: the next save
        // replaces it, and the config that was on disk stays readable.
        fs::write(&tmp, "target_wpm = [").expect("plant a tmp");
        let second = Config {
            target_wpm: 120.0,
            lesson_words: 40,
            ..Config::default()
        };
        second.save(&path).expect("second save");

        assert_eq!(
            Config::load(&path).expect("the config is still valid TOML"),
            second,
            "the new values are all there"
        );
        assert!(
            !tmp.exists(),
            "a .tmp file was left behind by {}",
            path.display()
        );
        let _ = fs::remove_dir_all(&dir);
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
