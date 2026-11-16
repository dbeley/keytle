//! Lesson generation: the text the typist practises next.
//!
//! The generator's job is to fill a lesson with letters the typist has unlocked
//! *and* can actually press, weighted towards the letters that are still shaky.
//! Confidence buys a letter a lower chance of appearing (`w = 0.15 + 1 - conf`),
//! so a lesson quietly turns into a drill of whatever is not yet fluent.
//!
//! Pseudo-words come from three generators, because a bigram model trained on a
//! real language has almost no mass left once the alphabet is four letters:
//!
//! * **fewer than 6 letters** — syllable templates (`CVC`, `VCVC`, `CVCVC`) over
//!   the letter's own vowels and consonants; there is nothing else to go on,
//! * **6 to 10 letters** — the corpus bigram model mixed with a uniform floor
//!   (`0.85 * bigram + 0.15 * uniform`), so rare pairs still occur,
//! * **more than 10 letters** — the bigram model as it is.
//!
//! Generated words are 3–8 letters, never repeat a letter three times in a row,
//! and never leave the unlocked ∩ reachable set. Everything is driven by the
//! injected RNG, so a seeded `SmallRng` reproduces a lesson exactly — which is
//! what makes the tests deterministic and a lesson replayable.

use std::collections::{BTreeMap, BTreeSet};

use rand::Rng;

use super::key::KeyStat;
use crate::config::Mode;
use crate::corpus::Corpus;
use crate::layout::Layout;

/// Below this many letters the bigram model is replaced by templates.
const TEMPLATE_TIER_BELOW: usize = 6;
/// Up to this many letters the bigram model is mixed with a uniform floor.
const MIXED_TIER_UP_TO: usize = 10;
/// Share of next letters taken from the uniform floor in the mixed tier.
const FLOOR_MIX: f32 = 0.15;
/// Word length bounds for pseudo words.
const MIN_WORD_LEN: usize = 3;
const MAX_WORD_LEN: usize = 8;
/// A mastered letter is still drilled at this share of a shaky one's weight.
const WEIGHT_FLOOR: f32 = 0.15;
/// `words` mode needs this many typable corpus words before it is worth using.
///
/// Deliberately low: a four-letter alphabet leaves only a dozen real words in a
/// ten-thousand-word dictionary, and repeating real words is a better lesson
/// than switching to pseudo words the moment the list thins out. Below this the
/// list is too small even to look like text, and the caller falls back.
const MIN_WORDS_CANDIDATES: usize = 6;
/// `sentences` mode needs this many typable sentences before it is worth using.
const MIN_SENTENCES: usize = 20;
/// Sentences in one sentences-mode lesson.
const SENTENCES_PER_LESSON: usize = 3;
/// Confidence is measured against the default 80 wpm target: the generator does
/// not receive the configured target interval, and weights only need to rank
/// letters against each other.
const CONFIDENCE_TARGET_MS: f32 = 150.0;
/// Vowel templates, `C` a consonant slot and `V` a vowel slot.
const TEMPLATES: [&str; 3] = ["CVC", "VCVC", "CVCVC"];
/// Always vowels, whatever the language.
const VOWELS: &str = "aeiouy";

/// One lesson's text and what the generator had to compromise on to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub text: String,
    /// How many words the text holds.
    pub words: usize,
    /// `None` when the lesson is exactly what was asked for.
    pub note: Option<Note>,
}

/// A compromise the lesson had to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// Too few real words fit the unlocked letters: the text is pseudo words.
    FallbackToPseudo,
    /// Too few sentences fit the unlocked letters: every letter was allowed.
    FullAlphabetSentences,
    /// Sentences were skipped because the unlocked letters cannot type them.
    SentencesDropped { dropped: usize, total: usize },
}

/// Build one lesson of `words` words for `mode`.
///
/// `unlocked` is what the profile allows, `reachable` what the host keyboard can
/// actually produce; their intersection is the alphabet. `keys` supplies the
/// per-letter confidence that decides how often each letter is drilled.
#[allow(clippy::too_many_arguments)] // the plan's interface; all arguments are state
pub fn generate(
    layout: &Layout,
    corpus: &Corpus,
    lang: &str,
    keys: &BTreeMap<char, KeyStat>,
    unlocked: &BTreeSet<char>,
    reachable: &BTreeSet<char>,
    mode: Mode,
    words: usize,
    rng: &mut impl Rng,
) -> Lesson {
    let alphabet: Vec<char> = unlocked.intersection(reachable).copied().collect();
    if alphabet.is_empty() || words == 0 {
        return Lesson {
            text: String::new(),
            words: 0,
            note: None,
        };
    }
    match mode {
        Mode::Pseudo => pseudo_lesson(corpus, lang, keys, &alphabet, words, None, rng),
        Mode::Words => words_lesson(corpus, keys, &alphabet, words, rng).unwrap_or_else(|| {
            pseudo_lesson(
                corpus,
                lang,
                keys,
                &alphabet,
                words,
                Some(Note::FallbackToPseudo),
                rng,
            )
        }),
        Mode::Sentences => sentences_lesson(layout, corpus, &alphabet, rng).unwrap_or_else(|| {
            pseudo_lesson(
                corpus,
                lang,
                keys,
                &alphabet,
                words,
                Some(Note::FallbackToPseudo),
                rng,
            )
        }),
    }
}

/// Pseudo words: the fallback for every mode and the default lesson shape.
fn pseudo_lesson(
    corpus: &Corpus,
    lang: &str,
    keys: &BTreeMap<char, KeyStat>,
    alphabet: &[char],
    count: usize,
    note: Option<Note>,
    rng: &mut impl Rng,
) -> Lesson {
    let vowels = vowels(lang, alphabet);
    let weights: Vec<(char, f32)> = alphabet
        .iter()
        .map(|&ch| (ch, letter_weight(ch, keys)))
        .collect();
    let model = BigramModel::new(corpus, alphabet);
    let tier = Tier::of(alphabet.len());
    let text = (0..count)
        .map(|_| next_word(&weights, &vowels, &model, tier, rng))
        .collect::<Vec<_>>()
        .join(" ");
    Lesson {
        words: count,
        text,
        note,
    }
}

/// Real corpus words, or `None` when too few of them fit the alphabet.
///
/// A word's chance of being drawn is its score: `sqrt(freq)` times the mean
/// weakness of its letters (see [`word_score`]), so common words dominate the
/// lesson and a word carrying a shaky key is drilled while the letters catch up.
fn words_lesson(
    corpus: &Corpus,
    keys: &BTreeMap<char, KeyStat>,
    alphabet: &[char],
    count: usize,
    rng: &mut impl Rng,
) -> Option<Lesson> {
    let allowed: BTreeSet<char> = alphabet.iter().copied().collect();
    let scored: Vec<(&str, f32)> = corpus
        .words_covered_by(&allowed)
        .into_iter()
        .map(|word| (word.text.as_str(), word_score(&word.text, word.freq, keys)))
        .collect();
    if scored.len() < MIN_WORDS_CANDIDATES {
        return None;
    }
    let mut chosen: Vec<&str> = Vec::with_capacity(count);
    for _ in 0..count {
        let mut word = pick_word(rng, &scored);
        // One retry is enough to avoid "the the" without biasing the draw.
        if chosen.last() == Some(&word) && scored.len() > 1 {
            word = pick_word(rng, &scored);
        }
        chosen.push(word);
    }
    Some(Lesson {
        text: chosen.join(" "),
        words: chosen.len(),
        note: None,
    })
}

/// A real word's draw weight: `sqrt(freq)` (so a ten-times commoner word is only
/// about three times likelier, and rare words still appear) times the mean
/// weakness of its letters (so a word made of shaky keys is drilled).
fn word_score(text: &str, freq: u64, keys: &BTreeMap<char, KeyStat>) -> f32 {
    let len = text.chars().count();
    let weakness = if len == 0 {
        WEIGHT_FLOOR
    } else {
        text.chars().map(|ch| letter_weight(ch, keys)).sum::<f32>() / len as f32
    };
    (freq as f32).sqrt().max(1.0) * weakness
}

/// Draw one word from a scored pool.
fn pick_word<'a>(rng: &mut impl Rng, pool: &[(&'a str, f32)]) -> &'a str {
    let total: f32 = pool.iter().map(|(_, score)| score).sum();
    if total <= 0.0 {
        return pool[index(rng, pool.len())].0;
    }
    let mut point = unit(rng) * total;
    for (word, score) in pool {
        if point < *score {
            return word;
        }
        point -= *score;
    }
    pool[pool.len() - 1].0
}

/// Three real sentences, or `None` when the corpus cannot supply them at all.
fn sentences_lesson(
    layout: &Layout,
    corpus: &Corpus,
    alphabet: &[char],
    rng: &mut impl Rng,
) -> Option<Lesson> {
    let typable = typable_chars(layout, alphabet);
    let usable = corpus.sentences_covered_by(&typable);
    let total = corpus.sentences.len();
    if usable.len() >= MIN_SENTENCES {
        let note = (usable.len() < total).then_some(Note::SentencesDropped {
            dropped: total - usable.len(),
            total,
        });
        return Some(join_sentences(&usable, note, rng));
    }
    // Too few sentences fit the unlocked letters. The full board is the escape
    // hatch: the typist gets real prose, and the note says the letters went wide.
    let full = typable_chars(layout, &all_letters(layout));
    let usable = corpus.sentences_covered_by(&full);
    if usable.len() >= SENTENCES_PER_LESSON {
        return Some(join_sentences(
            &usable,
            Some(Note::FullAlphabetSentences),
            rng,
        ));
    }
    None
}

/// `SENTENCES_PER_LESSON` consecutive sentences from a rotating start, so the
/// same sentence does not appear twice inside one lesson.
fn join_sentences(usable: &[&str], note: Option<Note>, rng: &mut impl Rng) -> Lesson {
    let start = index(rng, usable.len());
    let chosen: Vec<&str> = (0..SENTENCES_PER_LESSON.min(usable.len()))
        .map(|offset| usable[(start + offset) % usable.len()])
        .collect();
    let text = chosen.join(" ");
    Lesson {
        words: text.split_whitespace().count(),
        text,
        note,
    }
}

/// Which pseudo-word generator the alphabet size allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Templates,
    Mixed,
    Bigrams,
}

impl Tier {
    fn of(alphabet_len: usize) -> Tier {
        if alphabet_len < TEMPLATE_TIER_BELOW {
            Tier::Templates
        } else if alphabet_len <= MIXED_TIER_UP_TO {
            Tier::Mixed
        } else {
            Tier::Bigrams
        }
    }
}

/// Corpus bigrams restricted to the alphabet, keyed by the first letter.
struct BigramModel {
    rows: BTreeMap<char, Vec<(char, u32)>>,
}

impl BigramModel {
    fn new(corpus: &Corpus, alphabet: &[char]) -> BigramModel {
        let allowed: BTreeSet<char> = alphabet.iter().copied().collect();
        let mut rows: BTreeMap<char, BTreeMap<char, u32>> = BTreeMap::new();
        for ((first, second), count) in corpus.bigrams() {
            if allowed.contains(&first) && allowed.contains(&second) {
                *rows.entry(first).or_default().entry(second).or_insert(0) += count;
            }
        }
        BigramModel {
            rows: rows
                .into_iter()
                .map(|(first, row)| (first, row.into_iter().collect()))
                .collect(),
        }
    }

    fn row(&self, first: char) -> Option<&[(char, u32)]> {
        self.rows
            .get(&first)
            .map(Vec::as_slice)
            .filter(|row| !row.is_empty())
    }
}

/// Build one pseudo word in the generator the tier allows.
fn next_word(
    weights: &[(char, f32)],
    vowels: &BTreeSet<char>,
    model: &BigramModel,
    tier: Tier,
    rng: &mut impl Rng,
) -> String {
    match tier {
        Tier::Templates => template_word(weights, vowels, rng),
        Tier::Mixed | Tier::Bigrams => chain_word(weights, model, tier, rng),
    }
}

/// A word from a syllable template: the only engine that works when the alphabet
/// is too small to have any useful bigram mass.
fn template_word(weights: &[(char, f32)], vowels: &BTreeSet<char>, rng: &mut impl Rng) -> String {
    let template = TEMPLATES[index(rng, TEMPLATES.len())];
    let consonants: Vec<(char, f32)> = weights
        .iter()
        .copied()
        .filter(|(ch, _)| !vowels.contains(ch))
        .collect();
    let vowel_pool: Vec<(char, f32)> = weights
        .iter()
        .copied()
        .filter(|(ch, _)| vowels.contains(ch))
        .collect();
    let mut word: Vec<char> = Vec::with_capacity(template.len());
    for slot in template.chars() {
        let pool = match slot {
            // A tiny alphabet may have no vowels (or no consonants) at all; the
            // whole set is then the only honest fallback.
            'V' if !vowel_pool.is_empty() => &vowel_pool,
            'C' if !consonants.is_empty() => &consonants,
            _ => weights,
        };
        word.push(avoid_triple(pool, &word, rng));
    }
    word.into_iter().collect()
}

/// A word chained from the bigram model, with the mixed tier's uniform floor.
fn chain_word(
    weights: &[(char, f32)],
    model: &BigramModel,
    tier: Tier,
    rng: &mut impl Rng,
) -> String {
    let len = MIN_WORD_LEN + index(rng, MAX_WORD_LEN - MIN_WORD_LEN + 1);
    let mut word: Vec<char> = Vec::with_capacity(len);
    word.push(pick(rng, weights));
    while word.len() < len {
        let previous = *word.last().expect("the word starts non-empty");
        let from_bigram = tier == Tier::Bigrams || unit(rng) >= FLOOR_MIX;
        let pool = match (from_bigram, model.row(previous)) {
            (true, Some(row)) => bigram_pool(row, weights),
            // The floor, and any letter the corpus never started a word with.
            _ => uniform_pool(weights),
        };
        word.push(avoid_triple(&pool, &word, rng));
    }
    word.into_iter().collect()
}

/// Bigram counts multiplied by each letter's inclusion weight, so a shaky letter
/// is drilled even where the corpus says otherwise.
fn bigram_pool(row: &[(char, u32)], weights: &[(char, f32)]) -> Vec<(char, f32)> {
    row.iter()
        .map(|(ch, count)| (*ch, *count as f32 * weight_of(weights, *ch)))
        .collect()
}

/// Every letter at equal probability: the "uniform floor".
fn uniform_pool(weights: &[(char, f32)]) -> Vec<(char, f32)> {
    weights.iter().map(|(ch, _)| (*ch, 1.0)).collect()
}

/// Pick from `pool`, never producing the same letter three times in a row.
fn avoid_triple(pool: &[(char, f32)], word: &[char], rng: &mut impl Rng) -> char {
    let len = word.len();
    let banned = (len >= 2 && word[len - 1] == word[len - 2]).then(|| word[len - 1]);
    if let Some(banned) = banned {
        let filtered: Vec<(char, f32)> = pool
            .iter()
            .copied()
            .filter(|(ch, _)| *ch != banned)
            .collect();
        if !filtered.is_empty() {
            return pick(rng, &filtered);
        }
    }
    pick(rng, pool)
}

/// Draw one letter from a weighted pool.
fn pick(rng: &mut impl Rng, pool: &[(char, f32)]) -> char {
    let total: f32 = pool.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return pool[index(rng, pool.len())].0;
    }
    let mut point = unit(rng) * total;
    for (ch, weight) in pool {
        if point < *weight {
            return *ch;
        }
        point -= *weight;
    }
    pool[pool.len() - 1].0
}

/// `0..n`, rejecting the biased tail rather than folding it in.
fn index(rng: &mut impl Rng, n: usize) -> usize {
    debug_assert!(n > 0, "an empty pool has no index");
    let n = n as u64;
    let span = u64::MAX - u64::MAX % n;
    loop {
        let draw = rng.next_u64();
        if draw < span {
            return (draw % n) as usize;
        }
    }
}

/// A uniform `0.0..1.0` draw with 24 bits of resolution — enough for a lesson and
/// independent of the RNG crate's sampling API.
fn unit(rng: &mut impl Rng) -> f32 {
    f32::from((rng.next_u32() >> 8) as u16) / f32::from(1u16 << 8) / f32::from(1u16 << 8)
}

/// How much a letter deserves to be drilled: `w = 0.15 + (1 - confidence)`, so a
/// perfect key still appears occasionally and an untouched one appears most.
fn letter_weight(ch: char, keys: &BTreeMap<char, KeyStat>) -> f32 {
    let confidence = keys
        .get(&ch)
        .map_or(0.0, |stat| stat.confidence(CONFIDENCE_TARGET_MS));
    WEIGHT_FLOOR + (1.0 - confidence)
}

/// A letter's weight, by alphabet order (the pool is built from a sorted set).
fn weight_of(weights: &[(char, f32)], ch: char) -> f32 {
    weights
        .binary_search_by_key(&ch, |(letter, _)| *letter)
        .map_or(1.0, |at| weights[at].1)
}

/// The vowel set for `lang` restricted to the alphabet: `aeiouy` plus the
/// language's accented vowels, since a French lesson on `é` needs it treated as a
/// vowel rather than a consonant.
fn vowels(lang: &str, alphabet: &[char]) -> BTreeSet<char> {
    let accented = match lang {
        "fr" => "àâæéèêëîïôœùûüÿ",
        _ => "áàâäãåéèêëíìîïóòôöõúùûüýÿ",
    };
    VOWELS
        .chars()
        .chain(accented.chars())
        .filter(|ch| alphabet.contains(ch))
        .collect()
}

/// Every character a sentence may contain: the alphabet, the space, and whatever
/// punctuation the layout can produce.
fn typable_chars(layout: &Layout, letters: &[char]) -> BTreeSet<char> {
    let mut set: BTreeSet<char> = letters.iter().copied().collect();
    set.insert(' ');
    // Prose starts sentences with a capital, so a letter is typable in both of
    // its cases as long as the layout has the shifted key.
    for ch in letters {
        for upper in ch.to_uppercase().chain(ch.to_lowercase()) {
            if upper.is_alphabetic() && layout.contains_char(upper) {
                set.insert(upper);
            }
        }
    }
    for key in layout.keys.values() {
        for ch in [Some(key.base), key.shift, key.altgr].into_iter().flatten() {
            if !ch.is_alphanumeric() && !ch.is_control() {
                set.insert(ch);
            }
        }
    }
    set
}

/// Every letter the layout can produce, used when the unlocked set is too small
/// for real sentences.
fn all_letters(layout: &Layout) -> Vec<char> {
    let mut set: BTreeSet<char> = BTreeSet::new();
    for key in layout.keys.values() {
        for ch in [Some(key.base), key.shift, key.altgr].into_iter().flatten() {
            if ch.is_alphabetic() {
                set.insert(ch);
            }
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use crate::corpus::{Corpus, Word};
    use crate::layout::Layout;
    use rand::rngs::SmallRng;
    use rand::SeedableRng;
    use std::collections::{BTreeMap, BTreeSet};

    fn colemak() -> Layout {
        let src = crate::layouts::bundled_source("colemak-dh").expect("bundled layout");
        Layout::from_toml(src).expect("valid bundled layout")
    }

    fn en() -> Corpus {
        crate::corpora::load("en", None).expect("bundled corpus")
    }

    fn chars(s: &str) -> BTreeSet<char> {
        s.chars().collect()
    }

    fn stats(letters: &str) -> BTreeMap<char, KeyStat> {
        letters
            .chars()
            .map(|c| {
                (
                    c,
                    KeyStat {
                        hits: 50,
                        misses: 0,
                        shift_misses: 0,
                        mean_interval_ms: 150.0,
                    },
                )
            })
            .collect()
    }

    fn rng(seed: u64) -> SmallRng {
        SmallRng::seed_from_u64(seed)
    }

    fn words_of(lesson: &Lesson) -> Vec<String> {
        lesson.text.split(' ').map(str::to_string).collect()
    }

    /// Every letter of every word is in `allowed`; the separating spaces are not
    /// letters and are checked separately by the caller that cares.
    fn assert_inside(text: &str, allowed: &BTreeSet<char>) {
        assert!(!text.contains("  "), "double space in {text:?}");
        for word in text.split(' ') {
            for ch in word.chars() {
                assert!(
                    allowed.contains(&ch),
                    "{ch:?} outside {allowed:?} in {word:?}"
                );
            }
        }
    }

    fn assert_no_triple(text: &str) {
        for word in text.split(' ') {
            let letters: Vec<char> = word.chars().collect();
            for triple in letters.windows(3) {
                assert!(
                    !(triple[0] == triple[1] && triple[1] == triple[2]),
                    "three {:?} in a row in {word:?}",
                    triple[0]
                );
            }
        }
    }

    #[test]
    fn four_letters_produce_template_words_of_a_sane_length() {
        let allowed = chars("tnse");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnse"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            40,
            &mut rng(7),
        );
        assert_eq!(lesson.words, 40);
        assert_eq!(lesson.note, None);
        assert_inside(&lesson.text, &allowed);
        assert_no_triple(&lesson.text);
        for word in words_of(&lesson) {
            assert!(
                (3..=8).contains(&word.chars().count()),
                "word {word:?} of {} letters",
                word.chars().count()
            );
        }
        let distinct: BTreeSet<char> = lesson.text.chars().collect();
        assert!(
            distinct.len() >= 3,
            "only {distinct:?} appeared over 40 words"
        );
    }

    #[test]
    fn eight_letters_use_the_mixed_bigram_model() {
        let allowed = chars("tnseriao");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnseriao"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            60,
            &mut rng(11),
        );
        assert_eq!(lesson.words, 60);
        assert_inside(&lesson.text, &allowed);
        assert_no_triple(&lesson.text);
    }

    #[test]
    fn a_wide_alphabet_stays_inside_the_set() {
        let allowed = chars("abcdefghijklmn");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("abcdefghijklmn"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            60,
            &mut rng(3),
        );
        assert_inside(&lesson.text, &allowed);
        assert_no_triple(&lesson.text);
    }

    #[test]
    fn unreachable_letters_never_appear() {
        let unlocked: BTreeSet<char> = ('a'..='z').collect();
        let reachable: BTreeSet<char> = unlocked.difference(&chars("qz")).copied().collect();
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("abcdefghijklmnopqrstuvwxy"),
            &unlocked,
            &reachable,
            Mode::Pseudo,
            60,
            &mut rng(5),
        );
        assert_inside(&lesson.text, &reachable);
    }

    #[test]
    fn words_mode_returns_real_corpus_words_fully_covered() {
        let allowed: BTreeSet<char> = ('a'..='z').collect();
        let corpus = en();
        let lesson = generate(
            &colemak(),
            &corpus,
            "en",
            &stats("abcdefghijklmnopqrstuvwxyz"),
            &allowed,
            &allowed,
            Mode::Words,
            20,
            &mut rng(13),
        );
        assert_eq!(lesson.note, None);
        assert_eq!(lesson.words, 20);
        for word in words_of(&lesson) {
            assert_inside(&word, &allowed);
            assert!(
                corpus.words.iter().any(|known| known.text == word),
                "{word:?} is not a corpus word"
            );
        }
    }

    #[test]
    fn words_mode_uses_the_real_dictionary_from_the_first_four_letters() {
        // t, n, s and e are the first four Colemak-DH letters; the bundled
        // dictionary still has real words for them (ten, set, nest, …), so an
        // early learner is never shown generated text by default.
        let allowed = chars("tnse");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnse"),
            &allowed,
            &allowed,
            Mode::Words,
            20,
            &mut rng(17),
        );
        assert_eq!(lesson.note, None, "real words, no fallback");
        assert_eq!(lesson.words, 20);
        assert_inside(&lesson.text, &allowed);
        assert!(
            words_of(&lesson)
                .iter()
                .all(|word| word.chars().count() > 1),
            "no single-letter filler: {:?}",
            words_of(&lesson)
        );
    }

    #[test]
    fn words_mode_falls_back_to_pseudo_on_a_tiny_alphabet() {
        // q, x and z: the dictionary has almost nothing to offer, so the drill
        // says so and generates text instead of repeating two words.
        let allowed = chars("qxz");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("qxz"),
            &allowed,
            &allowed,
            Mode::Words,
            20,
            &mut rng(17),
        );
        assert_eq!(lesson.note, Some(Note::FallbackToPseudo));
        assert_eq!(lesson.words, 20);
        assert_inside(&lesson.text, &allowed);
    }

    /// A corpus of `targets` plus filler words, so `words` mode is not skipped for
    /// want of candidates. The filler is rare, so the targets dominate.
    fn scored_corpus(targets: &[(&str, u64)]) -> Corpus {
        let mut corpus = Corpus::default();
        for i in 0..MIN_WORDS_CANDIDATES {
            let text = format!("xy{}", "z".repeat(i % 4 + 1));
            corpus.words.push(Word::new(text, 1));
        }
        for (text, freq) in targets {
            corpus.words.push(Word::new(*text, *freq));
        }
        corpus
    }

    fn occurrences(lesson: &Lesson, word: &str) -> usize {
        words_of(lesson)
            .iter()
            .filter(|drawn| drawn.as_str() == word)
            .count()
    }

    #[test]
    fn a_word_with_a_shaky_key_is_drilled_more_often() {
        // `bat` and `cat` are equally frequent; `c` has never been practised, so
        // the weakness factor should make `cat` the likelier word.
        let allowed: BTreeSet<char> = ('a'..='z').collect();
        let keys = stats("bta");
        let lesson = generate(
            &colemak(),
            &scored_corpus(&[("bat", 1_000), ("cat", 1_000)]),
            "en",
            &keys,
            &allowed,
            &allowed,
            Mode::Words,
            400,
            &mut rng(37),
        );
        assert_eq!(lesson.note, None);
        let (bat, cat) = (occurrences(&lesson, "bat"), occurrences(&lesson, "cat"));
        assert!(cat > bat, "shaky `cat` {cat} vs strong `bat` {bat}");
    }

    #[test]
    fn a_much_commoner_word_still_wins_when_confidences_match() {
        let allowed: BTreeSet<char> = ('a'..='z').collect();
        let keys = stats("abcdefghijklmnopqrstuvwxyz");
        let lesson = generate(
            &colemak(),
            &scored_corpus(&[("bat", 10_000), ("cat", 100)]),
            "en",
            &keys,
            &allowed,
            &allowed,
            Mode::Words,
            400,
            &mut rng(41),
        );
        let (bat, cat) = (occurrences(&lesson, "bat"), occurrences(&lesson, "cat"));
        assert!(bat > cat, "frequent `bat` {bat} vs rare `cat` {cat}");
    }

    #[test]
    fn sentences_mode_joins_three_sentences() {
        let allowed: BTreeSet<char> = ('a'..='z').collect();
        let corpus = en();
        let lesson = generate(
            &colemak(),
            &corpus,
            "en",
            &stats("abcdefghijklmnopqrstuvwxyz"),
            &allowed,
            &allowed,
            Mode::Sentences,
            20,
            &mut rng(23),
        );
        assert!(lesson.text.split(' ').count() > 6, "{}", lesson.text);
        assert!(
            !matches!(lesson.note, Some(Note::FullAlphabetSentences)),
            "{:?}",
            lesson.note
        );
    }

    #[test]
    fn sentences_mode_bails_out_to_the_full_alphabet() {
        let allowed = chars("tnse");
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnse"),
            &allowed,
            &allowed,
            Mode::Sentences,
            20,
            &mut rng(29),
        );
        assert_eq!(lesson.note, Some(Note::FullAlphabetSentences));
        assert!(lesson.text.split(' ').count() > 6, "{}", lesson.text);
    }

    #[test]
    fn the_same_seed_gives_the_same_lesson() {
        let allowed = chars("tnseriao");
        let first = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnseriao"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            20,
            &mut rng(99),
        );
        let second = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnseriao"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            20,
            &mut rng(99),
        );
        assert_eq!(first, second);
        let other = generate(
            &colemak(),
            &en(),
            "en",
            &stats("tnseriao"),
            &allowed,
            &allowed,
            Mode::Pseudo,
            20,
            &mut rng(100),
        );
        assert_ne!(first, other, "a different seed should not repeat");
    }

    #[test]
    fn shakier_letters_are_drilled_more_often() {
        // `t` is strong, `n` is weak: over many words `n` should appear more.
        let allowed = chars("tns");
        let mut keys = stats("ts");
        keys.insert(
            'n',
            KeyStat {
                hits: 4,
                misses: 6,
                shift_misses: 0,
                mean_interval_ms: 600.0,
            },
        );
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &keys,
            &allowed,
            &allowed,
            Mode::Pseudo,
            300,
            &mut rng(31),
        );
        let count = |c: char| lesson.text.chars().filter(|x| *x == c).count();
        assert!(
            count('n') > count('t'),
            "n {} vs t {}",
            count('n'),
            count('t')
        );
    }

    #[test]
    fn an_empty_alphabet_yields_an_empty_lesson() {
        let lesson = generate(
            &colemak(),
            &en(),
            "en",
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            Mode::Pseudo,
            20,
            &mut rng(1),
        );
        assert_eq!(lesson.text, "");
        assert_eq!(lesson.words, 0);
    }
}
