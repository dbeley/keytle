//! The corpora that ship with keytle, and the user files that extend them.
//!
//! The bundled lists are compiled in with `include_str!`, so the trainer works
//! offline and out of the box. A user may drop `<user_dir>/<lang>/words.txt`
//! and/or `<user_dir>/<lang>/sentences.txt` next to them; those are merged on
//! top, which is how a learner adds their own vocabulary without touching the
//! shipped data. A missing user file is normal, not an error.
//!
//! Provenance and licences of the bundled data are in `NOTICE.md` beside
//! `src/data/corpora`.

use std::fs;
use std::io;
use std::path::Path;

use crate::corpus::{Corpus, CorpusError};

const EN_WORDS: &str = include_str!("data/corpora/en/words.txt");
const EN_SENTENCES: &str = include_str!("data/corpora/en/sentences.txt");
const FR_WORDS: &str = include_str!("data/corpora/fr/words.txt");
const FR_SENTENCES: &str = include_str!("data/corpora/fr/sentences.txt");

/// The languages with a bundled corpus, in config-file spelling.
pub fn languages() -> &'static [&'static str] {
    &["en", "fr"]
}

/// The bundled words and sentences for `lang`, if there are any.
fn embedded(lang: &str) -> Option<(&'static str, &'static str)> {
    match lang {
        "en" => Some((EN_WORDS, EN_SENTENCES)),
        "fr" => Some((FR_WORDS, FR_SENTENCES)),
        _ => None,
    }
}

/// Load the corpus for `lang`, merging any user files under `<user_dir>/<lang>`.
///
/// An unknown language is an error naming it; a missing user file is not.
pub fn load(lang: &str, user_dir: Option<&Path>) -> Result<Corpus, CorpusError> {
    let (words, sentences) = embedded(lang).ok_or_else(|| CorpusError::UnknownLanguage {
        lang: lang.to_string(),
    })?;
    let mut corpus = Corpus::parse_words(words)?;
    corpus.merge(Corpus::parse_sentences(sentences)?);

    if let Some(dir) = user_dir {
        let dir = dir.join(lang);
        merge_user_file(&mut corpus, &dir.join("words.txt"), File::Words)?;
        merge_user_file(&mut corpus, &dir.join("sentences.txt"), File::Sentences)?;
    }
    Ok(corpus)
}

/// Which parser a user file needs.
#[derive(Clone, Copy)]
enum File {
    Words,
    Sentences,
}

/// Merge one optional user file, labelling any bad line with the file it came
/// from so the error points at the file the user actually edited.
fn merge_user_file(corpus: &mut Corpus, path: &Path, kind: File) -> Result<(), CorpusError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(CorpusError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let label = path.display().to_string();
    let parsed = match kind {
        File::Words => Corpus::parse_words(&text),
        File::Sentences => Corpus::parse_sentences(&text),
    };
    corpus.merge(parsed.map_err(|err| err.in_file(&label))?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;
    use std::process;

    /// A scratch directory named after the test, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn with(name: &str) -> Scratch {
            let path = env::temp_dir().join(format!("keytle-corpora-{}-{name}", process::id()));
            let _ = fs::remove_dir_all(&path);
            for lang in ["en", "fr"] {
                fs::create_dir_all(path.join(lang)).expect("create the scratch corpus dir");
            }
            Scratch(path)
        }

        fn write(&self, relative: &str, contents: &str) {
            fs::write(self.0.join(relative), contents).expect("write the scratch file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn both_languages_are_listed() {
        assert_eq!(languages(), ["en", "fr"]);
    }

    #[test]
    fn bundled_english_is_large_and_ascii() {
        let corpus = load("en", None).expect("the bundled English corpus");
        assert!(corpus.words.len() >= 5000, "{} words", corpus.words.len());
        assert!(
            corpus.sentences.len() >= 500,
            "{} sentences",
            corpus.sentences.len()
        );
        // Every id in the English corpus is ASCII: no accents to type into a
        // layout that may not have them.
        assert!(corpus.words.iter().all(|word| word.text.is_ascii()));
        assert!(corpus.sentences.iter().all(|line| line.is_ascii()));
        assert!(corpus
            .words
            .iter()
            .all(|word| word.text.chars().all(|ch| ch.is_ascii_alphabetic())));
        assert!(corpus.words[0].freq >= corpus.words[10].freq);
    }

    #[test]
    fn bundled_french_contains_accented_words() {
        let corpus = load("fr", None).expect("the bundled French corpus");
        assert!(corpus.words.len() >= 5000);
        assert!(corpus.sentences.len() >= 500);
        assert!(
            corpus.words.iter().any(|word| !word.text.is_ascii()),
            "the French list should hold accented words"
        );
        assert!(
            corpus.sentences.iter().any(|line| !line.is_ascii()),
            "the French sentences should hold accented words"
        );
    }

    #[test]
    fn a_user_words_file_adds_one_word() {
        let scratch = Scratch::with("words");
        let base = load("fr", None).expect("the bundled French corpus");
        scratch.write("fr/words.txt", "zibzib 1\n");
        let merged = load("fr", Some(&scratch.0)).expect("the French corpus plus a user word");
        assert_eq!(merged.words.len(), base.words.len() + 1);
        assert_eq!(merged.words.last().map(|word| word.text.as_str()), Some("zibzib"));
        assert_eq!(merged.sentences.len(), base.sentences.len());
    }

    #[test]
    fn a_user_sentences_file_adds_a_sentence() {
        let scratch = Scratch::with("sentences");
        let base = load("en", None).expect("the bundled English corpus");
        scratch.write("en/sentences.txt", "Ahoy there.\n");
        let merged = load("en", Some(&scratch.0)).expect("the English corpus plus a user line");
        assert_eq!(merged.sentences.len(), base.sentences.len() + 1);
        assert_eq!(merged.sentences.last().map(String::as_str), Some("Ahoy there."));
    }

    #[test]
    fn a_user_word_already_in_the_list_just_gains_frequency() {
        let scratch = Scratch::with("bump");
        let base = load("en", None).expect("the bundled English corpus");
        let first = base.words[0].clone();
        scratch.write("en/words.txt", &format!("{} 1000\n", first.text));
        let merged = load("en", Some(&scratch.0)).expect("merged");
        assert_eq!(merged.words.len(), base.words.len());
        assert_eq!(merged.words[0].freq, first.freq + 1000);
    }

    #[test]
    fn a_bad_user_line_names_the_file_and_line() {
        let scratch = Scratch::with("badline");
        scratch.write("fr/words.txt", "bonjour 4\nnope! 2\n");
        let err = load("fr", Some(&scratch.0)).expect_err("punctuation in a user word file");
        let message = err.to_string();
        assert!(message.contains("line 2"), "{message}");
        assert!(message.contains("fr/words.txt"), "{message}");
    }

    #[test]
    fn a_missing_user_dir_is_fine() {
        let missing = env::temp_dir().join(format!("keytle-absent-{}/nope", process::id()));
        assert!(load("en", Some(&missing)).is_ok());
    }

    #[test]
    fn an_unknown_language_is_named_in_the_error() {
        let err = load("xx", None).expect_err("no such language");
        assert!(matches!(err, CorpusError::UnknownLanguage { .. }));
        assert!(err.to_string().contains("xx"), "{err}");
    }
}
