//! A corpus: a frequency-ordered word list plus a list of sentences.
//!
//! The on-disk formats are deliberately dumb so they can be edited by hand:
//!
//! * words - one `word frequency` pair per line, e.g. `the 22761659`,
//! * sentences - one sentence per line.
//!
//! Both formats tolerate a leading UTF-8 BOM and CRLF line endings, because
//! hand-edited files on Windows will have them. Blank lines are skipped. A word
//! must be made only of letters: whitespace, punctuation, digits and combining
//! marks are rejected with the offending line number, so a broken file fails
//! loudly at load time instead of producing an untypable lesson later.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

/// One entry of a frequency list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    /// Occurrences in the source text, as published by the list.
    pub freq: u64,
}

impl Word {
    /// A word with its frequency.
    pub fn new(text: impl Into<String>, freq: u64) -> Word {
        Word {
            text: text.into(),
            freq,
        }
    }
}

/// Words in frequency order plus the sentences the sentence mode drills.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Corpus {
    pub words: Vec<Word>,
    pub sentences: Vec<String>,
}

/// Why a corpus could not be read. Bad lines carry the 1-based line number.
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("bad corpus line {line}: {reason}")]
    BadLine { line: usize, reason: String },
    #[error("no corpus for language {lang:?}")]
    UnknownLanguage { lang: String },
    #[error("cannot read {path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl CorpusError {
    /// The same bad line, with `prefix` in front of the reason so the caller can
    /// say which file the line was in.
    pub fn in_file(self, prefix: &str) -> CorpusError {
        match self {
            CorpusError::BadLine { line, reason } => CorpusError::BadLine {
                line,
                reason: format!("{prefix}: {reason}"),
            },
            other => other,
        }
    }
}

impl Corpus {
    /// Parse a `word frequency` list. Entries keep their file order; a word that
    /// repeats inside one file keeps the larger of the two frequencies.
    pub fn parse_words(text: &str) -> Result<Corpus, CorpusError> {
        let mut words: Vec<Word> = Vec::new();
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (line, body) in lines(text).enumerate() {
            let number = line + 1;
            let body = body.trim();
            if body.is_empty() {
                continue;
            }
            let mut fields = body.split_whitespace();
            let (Some(text), Some(freq), None) = (fields.next(), fields.next(), fields.next())
            else {
                let count = body.split_whitespace().count();
                return Err(bad_line(
                    number,
                    format!("expected a word and a frequency, found {count} field(s)"),
                ));
            };
            if let Some(reason) = word_reason(text) {
                return Err(bad_line(number, reason));
            }
            let freq: u64 = freq.parse().map_err(|_| {
                bad_line(number, format!("{freq:?} is not a frequency for {text:?}"))
            })?;
            match seen.get(text) {
                Some(&at) => {
                    if freq > words[at].freq {
                        words[at].freq = freq;
                    }
                }
                None => {
                    seen.insert(text, words.len());
                    words.push(Word::new(text, freq));
                }
            }
        }
        Ok(Corpus {
            words,
            sentences: Vec::new(),
        })
    }

    /// Parse one sentence per line. Surrounding whitespace is trimmed and blank
    /// lines are skipped; a tab is rejected since it cannot be typed as written.
    pub fn parse_sentences(text: &str) -> Result<Corpus, CorpusError> {
        let mut sentences = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (line, body) in lines(text).enumerate() {
            let body = body.trim();
            if body.is_empty() {
                continue;
            }
            if body.contains('\t') {
                return Err(bad_line(line + 1, "sentences may not contain a tab"));
            }
            if seen.insert(body) {
                sentences.push(body.to_string());
            }
        }
        Ok(Corpus {
            words: Vec::new(),
            sentences,
        })
    }

    /// Fold `other` into this corpus: words are matched by text and their
    /// frequencies added, sentences are appended once.
    pub fn merge(&mut self, other: Corpus) {
        let mut at: BTreeMap<String, usize> = self
            .words
            .iter()
            .enumerate()
            .map(|(index, word)| (word.text.clone(), index))
            .collect();
        for word in other.words {
            match at.get(&word.text) {
                Some(&index) => {
                    self.words[index].freq = self.words[index].freq.saturating_add(word.freq);
                }
                None => {
                    at.insert(word.text.clone(), self.words.len());
                    self.words.push(word);
                }
            }
        }
        let mut seen: BTreeSet<String> = self.sentences.iter().cloned().collect();
        for sentence in other.sentences {
            if seen.insert(sentence.clone()) {
                self.sentences.push(sentence);
            }
        }
    }

    /// The words spelled entirely with characters the layout can produce, in
    /// corpus order.
    pub fn words_covered_by(&self, allowed: &BTreeSet<char>) -> Vec<&Word> {
        self.words
            .iter()
            .filter(|word| covered(&word.text, allowed))
            .collect()
    }

    /// The sentences spelled entirely with characters the layout can produce.
    pub fn sentences_covered_by(&self, allowed: &BTreeSet<char>) -> Vec<&str> {
        self.sentences
            .iter()
            .filter(|sentence| covered(sentence, allowed))
            .map(String::as_str)
            .collect()
    }

    /// How often each adjacent pair of letters occurs across the word list.
    ///
    /// Counts are per occurrence and unweighted by frequency: a list of 10 000
    /// words weighted by millions of OpenSubtitles hits would overflow and would
    /// only sharpen what the frequency order already encodes.
    pub fn bigrams(&self) -> BTreeMap<(char, char), u32> {
        let mut pairs = BTreeMap::new();
        for word in &self.words {
            let letters: Vec<char> = word.text.chars().collect();
            for pair in letters.windows(2) {
                *pairs.entry((pair[0], pair[1])).or_insert(0) += 1;
            }
        }
        pairs
    }
}

/// The lines of `text`, without a leading BOM and without CRLF's carriage
/// return, so the two line endings parse to the same corpus.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// Why `word` is not typable as a word, if it is not.
fn word_reason(word: &str) -> Option<String> {
    if word.is_empty() {
        return Some("the word is empty".to_string());
    }
    for ch in word.chars() {
        if ch.is_whitespace() {
            return Some(format!("{word:?} contains whitespace"));
        }
        if is_combining_mark(ch) {
            return Some(format!(
                "{word:?} contains the combining mark U+{:04X}",
                ch as u32
            ));
        }
        if !ch.is_alphabetic() {
            return Some(format!("{word:?} contains {ch:?}, which is not a letter"));
        }
    }
    None
}

/// A combining diacritical mark, which would make a word untypable as written:
/// `e` plus U+0301 looks like `é` but is two keystrokes of ambiguity.
fn is_combining_mark(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn bad_line(line: usize, reason: impl Into<String>) -> CorpusError {
    CorpusError::BadLine {
        line,
        reason: reason.into(),
    }
}

/// True when every character of `text` is in `allowed`. Coverage is per
/// character: a word is usable if the layout can type all of it.
fn covered(text: &str, allowed: &BTreeSet<char>) -> bool {
    text.chars().all(|ch| allowed.contains(&ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(chars: &str) -> BTreeSet<char> {
        chars.chars().collect()
    }

    #[test]
    fn words_parse_in_file_order_with_their_frequencies() {
        let corpus = Corpus::parse_words("the 12\nof 7\n").expect("a valid word list");
        assert_eq!(corpus.words, vec![Word::new("the", 12), Word::new("of", 7)]);
        assert!(corpus.sentences.is_empty());
    }

    #[test]
    fn a_bom_and_crlf_parse_like_plain_lf() {
        let plain = Corpus::parse_words("the 12\nof 7\n").expect("plain");
        let windows = Corpus::parse_words("\u{feff}the 12\r\nof 7\r\n").expect("bom + crlf");
        assert_eq!(plain, windows);

        let sentences = Corpus::parse_sentences("\u{feff}One.\r\nTwo.\r\n").expect("bom + crlf");
        assert_eq!(sentences.sentences, vec!["One.", "Two."]);
    }

    #[test]
    fn blank_lines_are_skipped() {
        let corpus = Corpus::parse_words("\n\nthe 1\n\n   \nof 2\n\n").expect("blank lines");
        assert_eq!(corpus.words.len(), 2);
        let sentences = Corpus::parse_sentences("\nBravo.\n\n  \n").expect("blank lines");
        assert_eq!(sentences.sentences, vec!["Bravo."]);
    }

    #[test]
    fn a_bad_line_names_its_line_number() {
        let err = Corpus::parse_words("the 12\nnot a word 9\n").expect_err("three fields");
        let message = err.to_string();
        assert!(matches!(err, CorpusError::BadLine { line: 2, .. }));
        assert!(message.contains("line 2"), "{message}");
    }

    #[test]
    fn punctuation_is_not_a_word() {
        let err = Corpus::parse_words("hello! 3\n").expect_err("punctuation");
        assert_eq!(
            err.to_string(),
            "bad corpus line 1: \"hello!\" contains '!', which is not a letter"
        );
    }

    #[test]
    fn a_combining_mark_is_rejected_by_name() {
        let err = Corpus::parse_words("cafe\u{301} 5\n").expect_err("a combining accent");
        assert!(matches!(err, CorpusError::BadLine { line: 1, .. }), "{err}");
        assert!(err.to_string().contains("combining mark"), "{err}");
    }

    #[test]
    fn a_non_numeric_frequency_is_a_bad_line() {
        let err = Corpus::parse_words("the many\n").expect_err("no frequency");
        assert!(err.to_string().contains("is not a frequency"), "{err}");
    }

    #[test]
    fn accented_letters_are_words() {
        let corpus = Corpus::parse_words("être 4\nà 9\n").expect("accented words");
        assert_eq!(corpus.words.len(), 2);
    }

    #[test]
    fn a_repeated_word_keeps_the_larger_frequency() {
        let corpus = Corpus::parse_words("the 3\nthe 8\nthe 5\n").expect("repeats");
        assert_eq!(corpus.words, vec![Word::new("the", 8)]);
    }

    #[test]
    fn a_tab_in_a_sentence_is_a_bad_line() {
        let err = Corpus::parse_sentences("Fine.\nTabs\there.\n").expect_err("a tab");
        assert!(err.to_string().contains("line 2"), "{err}");
    }

    #[test]
    fn merge_adds_frequencies_and_appends_new_entries_once() {
        let mut base = Corpus::parse_words("the 2\nof 1\n").expect("base");
        base.sentences.push("One.".to_string());
        let mut extra = Corpus::parse_words("the 3\nzebra 1\n").expect("extra");
        extra.sentences = vec!["Two.".to_string(), "One.".to_string()];

        base.merge(extra);

        assert_eq!(
            base.words,
            vec![
                Word::new("the", 5),
                Word::new("of", 1),
                Word::new("zebra", 1)
            ]
        );
        assert_eq!(base.sentences, vec!["One.".to_string(), "Two.".to_string()]);
    }

    #[test]
    fn coverage_keeps_only_fully_typable_entries() {
        let mut corpus = Corpus::parse_words("as 4\nask 3\nade 1\n").expect("words");
        corpus.sentences = vec!["as ad.".to_string(), "ask dad.".to_string()];

        let allowed = allow("asde ");
        assert_eq!(
            corpus
                .words_covered_by(&allowed)
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["as", "ade"]
        );
        assert_eq!(corpus.sentences_covered_by(&allow("asd. ")), vec!["as ad."]);
        assert!(corpus.words_covered_by(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn accents_are_not_covered_by_an_ascii_only_layout() {
        let mut corpus = Corpus::parse_words("Été 5\nà 4\nete 3\na 2\n").expect("words");
        corpus.sentences = vec!["Été à Paris.".to_string(), "Ete a Paris.".to_string()];

        let allowed = allow("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz. ");

        // `É` and `à` are separate characters from `E`/`e` and `a`: an ASCII
        // layout cannot type them, so the accented entries are excluded.
        assert_eq!(
            corpus
                .words_covered_by(&allowed)
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["ete", "a"]
        );
        assert_eq!(corpus.sentences_covered_by(&allowed), vec!["Ete a Paris."]);
    }

    #[test]
    fn bigrams_count_adjacent_pairs_across_the_words() {
        let corpus = Corpus::parse_words("the 9\nthen 1\n").expect("words");
        let pairs = corpus.bigrams();
        assert_eq!(pairs.get(&('t', 'h')), Some(&2));
        assert_eq!(pairs.get(&('h', 'e')), Some(&2));
        assert_eq!(pairs.get(&('e', 'n')), Some(&1));
        assert_eq!(pairs.len(), 3);
        assert_eq!(Corpus::default().bigrams().len(), 0);
    }
}
