# TASTES

## 1. What a taste is

A taste is a stable, near-universal operator preference about how work should
be shaped. It is deliberately blunter than a lint and broader than a task
instruction. A useful taste applies across files, tasks, and lanes; names an
action the author can take immediately; and remains worth hearing when a
harmless mention triggers judgment. Guidance that depends on one task, one
subsystem, or a long exception list belongs in that task or its review instead.

The word bag selects candidate prose. It does not decide whether the prose
violates the taste. The judge receives the taste, the trigger span, and enough
surrounding context to distinguish use from mention. Prose that exhibits the
disfavored shape confirms; prose that quotes, cites, or analyzes the rule
clears. The journal records the outcome and locus as `taste::confirmed` or
`taste::cleared`. A trigger hit alone is neither a fact nor steering.

The active set is the tracked `[taste]` configuration. `TASTES.md` owns the
curation law and the rationale for the installed seeds; it does not form a
second executable copy. Named entries exist only through operator commits.

## 2. Authoring rules

A taste must satisfy all of these rules:

- State one portable preference as a blunt imperative. Name the desired action,
  not the subsystem where one example happened.
- Use one ordinary word for the taste name. The name is identity; changing it
  removes one taste and adds another.
- Keep the word bag small. Include the words authors naturally use when
  proposing the disfavored shape. Avoid project nouns and overloaded words
  whose ordinary uses would dominate judgment.
- Use exact words, phrases, or word globs. Matching is over
  whitespace-normalized, case-insensitive tokens: a bare word matches one
  whole token, a trailing `*` covers suffixes of that token, and an entry
  containing a space is a phrase matched across normalized whitespace and
  line breaks. No other glob position and no regex is admitted. Prefer
  explicit forms when a stem would capture unrelated words.
- Make the message imperative, context-independent, and immediately
  actionable. The message must carry the whole preference because the trigger
  words are only candidate selectors.
- Prefer the general failure over one narrow symptom. If one message needs
  several unrelated bags, merge the bags; if one bag needs several unrelated
  messages, split the taste.
- Treat false positives as a design cost. A harmless candidate must still be
  cheap to judge and useful to reconsider; otherwise the taste is too
  contextual.
- Prove use and mention separately. The behavior fixture must confirm, the
  taste's own text must clear, and the documents of record must clear every
  active bag in full.

The gauntlet the wave-zero TASTES leaf archives under `docs/record/` is the
standing non-hit corpus. A change to a word bag or message is incomplete until
that corpus clears and the paired behavior fixture still confirms; the
`fallbacks` fixture exercises a phrase trigger specifically, so the phrase
clause of the matching law cannot rot silently.

## 3. Curation lifecycle

`spice install` writes the starter set into tracked `spice.toml`; the binary
ships no named tastes. From that point the operator owns the set outright.
Adding, editing, renaming, or removing a taste requires an operator commit.
Deletion is the only disablement.

`spice taste add` prepares an admission proposal with the recurring evidence
that motivated it. It does not activate the taste. Admission requires the
authoring checks, the paired confirm-and-clear fixtures, and an operator commit
of the exact entry.

Every judgment preserves its locus. The curation view shows confirmations,
clearances, and their ages rather than presenting an all-time count as current
truth. Recent repeated confirmations support retention. Repeated clearances
mean either that the bag is too broad or that the preference no longer earns
its interruption; the attached loci let the operator distinguish those cases.

`spice taste drop NAME` prepares a retirement proposal carrying the cleared
loci that provoked it. The operator may narrow the bag, revise the message, or
delete the tracked entry. Only the commit changes membership. Earlier facts
remain history after deletion; they never reactivate the taste.

## 4. The starter set install seeds

Install seeds these entries:

```toml
[taste.polling]
words = ["delay*", "poll*", "sleep*", "slept"]
message = "Respond to the real event, signal, or completion notification through a blocking call, watcher, or callback, or restructure the flow so nothing remains to await."

[taste.fallbacks]
words = ["fall back", "fall backs", "fallback*", "falls back"]
message = "Keep one deterministic path and let violated assumptions refuse immediately. Add a default or resolver order only when the contract names it."

[taste.compatibility]
words = ["compatib*"]
message = "Keep one tested, visible current path. Migrate retained data forward once or refuse naming its owning release; never accumulate release-specific execution arms."

[taste.shims]
words = ["shim", "shimmed", "shimming", "shims"]
message = "Replace the old shape outright, delete it, and route every caller to the current interface."

[taste.aliases]
words = ["alias*"]
message = "Rename in place, update every reference in one change, and leave exactly one name."

[taste.legacy]
words = ["legac*"]
message = "Treat old implementations as evidence, never as executable arms. Migrate retained data forward once or refuse naming its owning release."
```
