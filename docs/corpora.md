# Corpora

A lesson is built from a word list rather than from random letters, so what you
type is English or French instead of noise.

## Bundled

| language | words | sentences |
| --- | --- | --- |
| `en` | FrequencyWords, top 10 000, checked against a real dictionary | Tatoeba |
| `fr` | Lexique 3.83, top 10 000 forms | Tatoeba |

Both keep their published frequencies, and both are compiled in, so keytle works
offline and out of the box. Provenance, the changes made to each list and their
licences are in
[`keytle-core/src/data/corpora/NOTICE.md`](../keytle-core/src/data/corpora/NOTICE.md).

## Your own

Drop files next to the bundled ones and they are merged on top:

```
<config>/corpora/<lang>/words.txt       word frequency   - one per line
<config>/corpora/<lang>/sentences.txt   one sentence per line
```

`<config>` is `~/.config/keytle` unless `KEYTLE_CONFIG_DIR` or
`$XDG_CONFIG_HOME` move it. A missing file is normal; a bad line is an error that
names the file and the line number, so a broken list fails at load time instead
of producing an untypable lesson. `words.txt` lines are `the 22761659` - letters
only - while `sentences.txt` may use punctuation.

A file dropped there is used by the next lesson; there is no import step.

## Modes

| mode | what a lesson is |
| --- | --- |
| `words` | 20 real dictionary words, the default |
| `pseudo` | words generated from the corpus's letter pairs, for day one when the alphabet is still tiny |
| `sentences` | three consecutive real sentences |

Every mode only uses letters you have unlocked: a lesson never asks for a key you
do not own yet. When the alphabet is too narrow to spell enough real words (or to
cover enough sentences), keytle says so in the status line and falls back to
pseudo-words for that lesson rather than showing you untypable text.
