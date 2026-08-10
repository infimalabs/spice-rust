# TASTES

## 1. What a taste is

A taste is a stable, near-universal operator preference about how work should
be shaped. It is deliberately blunter than a lint and broader than a task
instruction. A useful taste applies across files, tasks, and lanes; names an
action the author can take immediately; and remains worth hearing when a
harmless mention fires it. Guidance that depends on one task, one subsystem, or
a long exception list belongs in that task or its review instead.

One runtime law is executable, and selection is the whole decision. A
configured bag hit in authored prose emits one `[TASTE]` ordinary directive to
the lane that wrote the prose, carrying the taste's message unchanged. Nothing
separates use from mention first. The same taste and normalized prose fire at
most once per authored content and compaction epoch, and prose already carrying
the reserved `[TASTE]` marker is classified as the runtime's own echo before
matching, so a quoted reminder never fires again. From there it is ordinary
steering: acknowledged, declined, resent, and expiring like any other
directive. A decline is the operator's judgment recorded after the
interruption, not a gate placed before it.

Historical `taste::confirmed` and `taste::cleared` facts predate that law. They
remain decodable and visible as curation evidence; no current command mints
another one.

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
  whose ordinary uses would dominate the fires.
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
- Treat false positives as a design cost paid in interruptions. A mention fires
  the same reminder a violation does, so a harmless candidate must still be
  cheap to decline and useful to reconsider; otherwise the taste is too
  contextual.
- Prove selection on both a use and a mention. Each fixture must select the
  candidate, because the bag cannot tell them apart and the authoring rules are
  what keep the mention worth reading.

The paired fixtures are `taste/tests/fixtures/behavior.txt` and
`taste/tests/fixtures/mention.txt`. A change to a word bag or message is
incomplete until both still select and every published seed still selects prose
built from its own tracked words; the `fallbacks` seed carries a phrase
trigger specifically, so the phrase clause of the matching law cannot rot
silently.

## 3. Curation lifecycle

`spice install` writes the starter set into tracked `spice-rust.toml`; the binary
ships no named tastes. From that point the operator owns the set outright.
Adding, editing, renaming, or removing a taste requires an operator commit.
Deletion is the only disablement.

`spice taste add` and `spice taste drop` prepare curation proposals from the
evidence that motivated them. A proposal inherits the operator's active task,
or names its steering or parent cause explicitly with `--origin`; an ungrounded
filing is refused. A proposal does not change the tracked set. Admission or
retirement requires the authoring checks, the paired confirm-and-clear
fixtures, and an operator commit of the exact entry.

Every fire preserves its locus. The curation view is derived from the journal
at one snapshot: recent reminders per taste and per driver, how each was
answered, and how often one unchanged identity returned across compaction
epochs. It shows those fires and their ages rather than presenting an all-time
count as current truth, and it persists no counter of its own. A lineage that
was resent counts once, because the resend climbs the same ladder rather than
starting a second reminder.

Recurrence with acknowledgment supports retention: the preference keeps being
worth saying. Repeated declines mean either that the bag is too broad or that
the preference no longer earns its interruption; the attached loci let the
operator distinguish those cases. Historical clearances read the same way and
stay visible for as long as the journal holds them.

`spice taste drop NAME` prepares a retirement proposal carrying the recent
declined loci and the historical clearances that provoked it. The operator may
narrow the bag, revise the message, or delete the tracked entry. Only the
commit changes membership. Earlier facts remain history after deletion; they
never reactivate the taste.

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
