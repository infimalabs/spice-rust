# TASTES

## 1. What a taste is

A taste is a stable, near-universal operator preference about how work should
be shaped. It is deliberately blunter than a lint and broader than a task
instruction. A useful taste applies across files, tasks, and lanes; names an
action the author can take immediately; and remains worth hearing when a
harmless mention fires it. Guidance that depends on one task, one subsystem, or
a long exception list belongs in that task or its review instead.

One runtime law has two explicitly configured modes. With `judge.enabled`
absent or false, selection is the whole decision: a configured bag hit in
authored prose emits one `[TASTE]` ordinary directive to the lane that wrote the
prose, carrying the taste's message unchanged. With `judge.enabled = true`, the
resident deterministically shuffles several equivalent framings, then invokes a
fresh bounded judge child for each framing in sequence. Exactly one normalized YES confirms the
candidate and opens the same ordinary directive; exactly one normalized NO
clears it without interruption. Malformed replies retry only to the declared
bound, while absence, timeout, process error, or exhausted ambiguity abandons
the candidate and emits nothing.

Both judgments are durable `taste::judged` evidence carrying the exact source,
bag identity, trigger, vendor, transcript locus, compaction epoch, and ordered
attempt provenance. That evidence makes replay and restart idempotent; only its
confirmed variant is a steering publication. The same taste and normalized
prose can be decided at most once for one bag and compaction epoch, and prose
already carrying the reserved `[TASTE]` marker is classified as the runtime's
own echo before matching, so a quoted reminder never becomes a candidate.
Confirmed and direct reminders are ordinary steering: acknowledged, declined,
resent, and expiring like any other directive.

`taste::judged` is the sole resident decision fact. Other decodable
taste-decision facts are read-only curation evidence and never drive a resident
decision.

The configured set is the tracked `[taste]` configuration. `enabled` defaults
to true and `scopes` defaults to the universal selector. Each named row may set
`enabled = false` while remaining visible and enumerable, and may carry a
`scopes` selector using the same `paths`, `extensions`, `vendors`, `models`,
and `phases` grammar as the rest of repository policy. Alternatives within one
axis are ORed and constrained axes are ANDed; malformed selectors and missing
runtime context refuse rather than widening applicability. `TASTES.md` owns
the curation law and the rationale for the installed seeds; it does not form a
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
- Use exact alphabetic words or phrases and enumerate every intended variation
  in the bag. Matching is over case-insensitive whole words, and an entry
  containing whitespace is a phrase matched across consecutive words. Globs,
  digits, punctuation, stemming, and substring matching are not admitted.
- Make the message imperative, context-independent, and immediately
  actionable. The message must carry the whole preference because the trigger
  words are only candidate selectors.
- Prefer the general failure over one narrow symptom. If one message needs
  several unrelated bags, merge the bags; if one bag needs several unrelated
  messages, split the taste.
- Treat false positives as a design cost. The direct mode pays in
  interruptions; adjudicated mode pays in bounded judge calls and may still
  confirm a poor candidate. A harmless candidate must therefore stay cheap to
  clear or decline and useful to reconsider; otherwise the taste is too
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

Tracked `spice-rust.toml` carries the starter set; installation never authors
tracked policy, and the binary ships no named tastes. The operator owns the set
outright.
Adding, editing, renaming, disabling, re-enabling, or removing a taste requires
an operator commit. `enabled = false` is the named reversible disablement;
deletion removes the row and its history from the current configured record.

When tracked policy declares `policy.taste_mining_threshold >= 2`, `spice taste
mine` reads one authenticated journal snapshot, clusters recurring operator
corrections answered by acknowledgement or decline at that threshold, and files
or reuses hidden, operator-reviewed proposals whose origins name the oldest
supporting acknowledgement key as `ack:<key>`. Without that declaration mining
refuses, and in either case it never edits tracked policy. `spice taste add TEXT`
and `spice taste drop NAME` prepare explicit curation proposals from the evidence
that motivated them. A proposal inherits the acting caller's sole active claim,
or carries an explicit causal `--origin`—ordinarily `ack:` or `task:`; an
ungrounded filing is refused. No proposal changes the tracked set. Admission or
retirement requires the authoring checks, the paired confirm-and-clear fixtures,
and an operator commit of the exact entry.

Every direct fire and adjudicated decision preserves its locus. The curation
view is derived from the journal at one snapshot: recent reminders per taste
and per vendor, confirmed and cleared provenance, how live reminders were
answered, and how often one unchanged identity returned across compaction
epochs. It shows those fires and their ages rather than presenting an all-time
count as current truth, and it persists no counter of its own. A lineage that
was resent counts once, because the resend climbs the same ladder rather than
starting a second reminder.

Recurrence with acknowledgment supports retention: the preference keeps being
worth saying. Repeated declines mean either that the bag is too broad or that
the preference no longer earns its interruption; the attached loci let the
operator distinguish those cases. Recorded clearances read the same way and
stay visible for as long as the journal holds them.

`spice taste drop NAME` prepares a retirement proposal carrying the recent
declined loci and the recorded clearances that provoked it. The operator may
narrow the bag, revise the message, or delete the tracked entry. Only the
commit changes membership. Earlier facts remain history after deletion; they
never reactivate the taste.

## 4. The tracked starter set

Repository configuration carries these entries:

```toml
[taste.polling]
words = ["delay", "delays", "delayed", "delaying", "poll", "polls", "polled", "polling", "sleep", "sleeps", "sleeping", "slept"]
message = "Respond to the real event, signal, or completion notification through a blocking call, watcher, or callback, or restructure the flow so nothing remains to await."

[taste.fallbacks]
words = ["fall back", "fall backs", "fallback", "fallbacks", "falls back"]
message = "Keep one deterministic path and let violated assumptions refuse immediately. Add a default or resolver order only when the contract names it."

[taste.compatibility]
words = ["compatible", "compatibility", "compatibilities", "compatibly"]
message = "Keep one tested, visible current path. Migrate retained data forward once or refuse naming its owning release; never accumulate release-specific execution arms."

[taste.shims]
words = ["shim", "shimmed", "shimming", "shims"]
message = "Replace the old shape outright, delete it, and route every caller to the current interface."

[taste.aliases]
words = ["alias", "aliases", "aliased", "aliasing"]
message = "Rename in place, update every reference in one change, and leave exactly one name."

[taste.legacy]
words = ["legacy", "legacies"]
message = "Treat old implementations as evidence, never as executable arms. Migrate retained data forward once or refuse naming its owning release."
```
