# Third-party data in keytle

The bundled corpora under `keytle-core/src/data/corpora/` are derived from the
sources below. keytle itself is MIT OR Apache-2.0; the data files keep the
licences of their sources.

## FrequencyWords - `en/words.txt` frequency order

- Source: <https://github.com/hermitdave/FrequencyWords>, release `2018`
  - `https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/en/en_50k.txt`
- Derived from the OpenSubtitles 2018 corpus.
- Licence: Creative Commons Attribution-ShareAlike 4.0 International
  (CC-BY-SA 4.0) - <https://creativecommons.org/licenses/by-sa/4.0/>
- Changes: the top 10 000 entries were kept, filtered to entries that are purely
  alphabetic and present in `dwyl/english-words`, lowercased, de-duplicated, and
  written as `word frequency` per line with the published counts. Words shorter
  than three letters were dropped unless they are everyday function words
  (`a`, `i`, `is`, `it`, `to`, `in`, `on`, `of`, `so`, `we`, …), because the
  subtitle list's one- and two-letter entries are mostly names, abbreviations
  and cut-off words.

## dwyl/english-words - `en/words.txt` word filter

- Source: <https://github.com/dwyl/english-words>, `words_alpha.txt`
- Licence: The Unlicense (public domain dedication) -
  <https://unlicense.org>
- Changes: used as the dictionary the FrequencyWords entries above are checked
  against, so subtitle noise (`hmm`, `gotta`, character names) never reaches a
  lesson. No words are copied from this list into the bundled file; it only
  decides which of the frequency entries are real words.

## Lexique 3.83 - `fr/words.txt`

- Source: <http://www.lexique.org/databases/Lexique383/Lexique383.tsv>
- A lexicon of written and spoken French (New et al.), the reference list of
  French word forms with frequencies.
- Licence: Creative Commons Attribution-ShareAlike 4.0 International
  (CC-BY-SA 4.0) - <https://creativecommons.org/licenses/by-sa/4.0/>
- Changes: the `ortho` column was lowercased and filtered to purely alphabetic
  forms (French letters including accents); frequencies are
  `freqfilms2 + freqlivres` summed per form; the 10 000 most frequent forms were
  kept and written as `word frequency` per line.

## Tatoeba - `en/sentences.txt`, `fr/sentences.txt`

- Source: <https://tatoeba.org>, per-language sentence exports
  - `https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2`
  - `https://downloads.tatoeba.org/exports/per_language/fra/fra_sentences.tsv.bz2`
- Licence: Creative Commons Attribution 2.0 France (CC-BY 2.0 FR) -
  <https://creativecommons.org/licenses/by/2.0/fr/>
- Changes: sentences of 20–80 characters were kept, one per line; lines that
  contained tabs or quotation marks, or characters outside ordinary letters and
  punctuation, were dropped; typographic apostrophes were normalised to `'`;
  duplicates were removed and the first 800 kept per language.
