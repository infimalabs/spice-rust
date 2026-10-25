# Tastes

A taste is a stable, near-universal operator preference about how work should
be shaped: blunter than a lint and broader than a task instruction. It applies
across files, tasks, and lanes, names an action the author can take directly,
and stays worth hearing when a harmless mention fires it. Guidance that depends
on one task, one subsystem, or a long exception list belongs in that task or
its review.

The tracked `[taste]` table in `spice.toml` is the only configured set; the
binary ships no named tastes. Matching, adjudication, reminder, and selector
rules live in the
[maxim register](docs/product/invariants/session.md#session-a-maxim-bag-is-one-opinion-with-several-triggers).

## Authoring rules

- State one portable preference as a blunt imperative. Name the desired action,
  not the subsystem where one example happened.
- Use one ordinary word for the name. The name is identity; renaming removes
  one taste and adds another.
- Keep the word bag small: the words authors naturally use when proposing the
  disfavored shape. Avoid project nouns and overloaded words whose ordinary
  uses would dominate the fires.
- Enumerate every intended variation. Matching is over case-insensitive whole
  alphabetic words, and an entry containing whitespace is a phrase; globs,
  digits, punctuation, stemming, and substrings are refused.
- Make the message imperative, context-independent, and actionable on its own;
  the trigger words only select candidates.
- Prefer the general failure over one narrow symptom. If one message needs
  several unrelated bags, merge them; if one bag needs several unrelated
  messages, split the taste.
- Treat false positives as a design cost. Direct mode pays in interruptions,
  adjudicated mode in judge calls; a harmless candidate must stay cheap to
  clear or decline, or the taste is too contextual.
- Prove selection on both a use and a mention
  (`taste/tests/fixtures/behavior.txt` and `mention.txt`). The bag cannot tell
  them apart, so the authoring rules are what keep the mention worth reading.

## Curation

The operator owns the set. Adding, editing, renaming, disabling
(`enabled = false`), re-enabling, or removing a taste is an operator commit;
deleting a row leaves its earlier facts as history that never reactivate it.

`spice taste add TEXT` and `spice taste drop NAME` file curation proposals from
the evidence that motivated them; a drop proposal carries the recent declined
loci and recorded clearances. A proposal inherits the caller's sole active claim
or needs an explicit `--origin`. No proposal changes the tracked set.

The curation view derives from the journal at one snapshot: recent reminders
per taste and vendor, confirmed and cleared provenance, how live reminders were
answered, and recurrence across compaction epochs. It shows fires and their
ages rather than an all-time count, and a resent lineage counts once.
Recurrence with acknowledgment supports keeping a taste; repeated declines mean
the bag is too broad or the preference does not earn its interruption, and
the attached loci tell which.
