# Third-party data in keytle

The bundled corpora under `keytle-core/src/data/corpora/` are derived from the
sources below. keytle itself is MIT OR Apache-2.0; the data files keep the
licences of their sources.

## FrequencyWords — `en/words.txt`, `fr/words.txt`

- Source: <https://github.com/hermitdave/FrequencyWords>, release `2018`
  - `https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/en/en_50k.txt`
  - `https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/fr/fr_50k.txt`
- Derived from the OpenSubtitles 2018 corpus.
- Licence: Creative Commons Attribution-ShareAlike 4.0 International
  (CC-BY-SA 4.0) — <https://creativecommons.org/licenses/by-sa/4.0/>
- Changes: the top 10 000 entries were kept, each list filtered to entries that
  are purely alphabetic in that language (`en` ASCII letters only, `fr` Latin
  letters including accents), lowercased, de-duplicated, and written as
  `word frequency` per line with the published counts.

## Tatoeba — `en/sentences.txt`, `fr/sentences.txt`

- Source: <https://tatoeba.org>, per-language sentence exports
  - `https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2`
  - `https://downloads.tatoeba.org/exports/per_language/fra/fra_sentences.tsv.bz2`
- Licence: Creative Commons Attribution 2.0 France (CC-BY 2.0 FR) —
  <https://creativecommons.org/licenses/by/2.0/fr/>
- Changes: sentences of 20–80 characters were kept, one per line; lines that
  contained tabs or quotation marks, or characters outside ordinary letters and
  punctuation, were dropped; typographic apostrophes were normalised to `'`;
  duplicates were removed and the first 800 kept per language.
