# CONFIG

One tree, one law: every top-level table is a stable domain, every key
is a single word, and nesting follows paths. Domains coincide with
crates today, but a crate refactor never moves operator surface. The
compiler binds the law: the tree is the serde shape of each domain's
config types, so an unknown table or key refuses at load and names its
full path.

## The Law

1. **Tables are domains.** `[taste]`, `[consent]`, `[gate]`, `[board]`,
   `[lane]`, `[steer]`, `[vendor]`, `[listen]`, `[lock]`, `[canvas]`,
   and `[proof]` are the complete top level. Nesting follows domain
   paths: `[board.next]` configures the allocator. A later crate
   boundary may move beneath that path; the operator surface does not.
2. **Keys are single words.** Foreign values keep their skin: a binary
   path, a model name, or a container digest may follow its owner's
   convention; our keys never compound.
3. **Defaults are compiled.** Every default is a `Default` impl in the
   binary, sealed by the proof hash. Files carry only intent; an empty
   spice.toml is a complete, working configuration.
4. **Builtins are values, never entries.** The binary ships numbers,
   strings, and shapes; it ships zero tastes, zero consent rules, and
   zero seats. Every named entry is operator-authored in the tracked
   file. `spice install` seeds a starter set the operator owns
   outright, and **deletion is the only disable**. No `enabled` flags,
   `false` sentinels, or shadowing machinery exist.
5. **One pattern form: the glob.** Gitignore semantics apply to paths
   and star semantics to words: `"poll*"` covers polled, polling, and
   polls; `"src/**"` covers the tree. Exact strings are exact.
   The matching unit is stated once: prose is compared as
   whitespace-normalized, case-insensitive tokens; a bare word matches
   one whole token, a trailing `*` covers suffixes of that token, and
   an entry containing a space is a phrase matched across normalized
   whitespace and line breaks. **Regex never appears in
   configuration**: a rule that needs one is a crate change with
   tests, not a config value. Patterns remain auditable by eye.
6. **Rules are named, ordered, first-match.** Every decision table is
   an array of tables in document order. The first matching rule
   answers, and every rule carries `name` because facts cite it:
   `consent::granted` names the granting rule, and budgets meter per
   name. A rule's name is its identity; renaming creates a new rule
   with a fresh budget.
7. **Durations are words, not numbers.** `"90s"`, `"15m"`, and
   `"24h"` share one closed grammar and one parser. A bare number
   refuses by naming the key. A config file reads aloud correctly or
   it does not load.
8. **The truth has no knobs.** `journal`, `bus`, `fold`, `ack`,
   `dialect`, `tape`, `wire`, and `app` take no configuration. What
   has no keys cannot be misconfigured, and authority is exactly the
   part that must not be. `listen` does take one operational knob:
   `window` is the canary's absence horizon, expressed as a duration
   word.
9. **The executable surface executes only approved.** The parts of
   tracked config that command execution or judgment—consent rules,
   vendor binaries, hooks, the skill, gate baselines, the taste
   table, and the proof container pin—carry an approval digest. Git
   cannot distinguish an operator commit from an agent commit, so the
   resident can: changed bytes append `trust::pended`, and the
   changed surface refuses loudly until the operator grants that
   digest. No earlier arm runs meanwhile; refusal, never fallback.
10. **One emission exposes everything.** Bare `spice config` renders
    the complete effective tree as valid TOML: every key of every
    domain, nothing hidden, with per-key provenance (`builtin`,
    `tracked`, `local`, or `flag`) and the builtin value shown when an
    override shadows it. `--json` emits the same structs. Load
    validates everything at once; the resident refuses to start on
    bad config and names every bad key in one report.
11. **Every key names its admitted layers.** A generated per-key
    registry carries domain path, type, builtin, admitted layers,
    policy classification, and flag mapping; merge validates every
    source against it, and help and provenance render from it. The
    local layer is default-deny: release one admits exactly `[lock]`,
    `[canvas.pack]`, and `[listen].window` in `config.toml` — machine
    capacity and rendering posture — and every other key is tracked
    intent an agent-band write refuses by name.

## The three files

| File | Carries | Written by |
| --- | --- | --- |
| spice.toml (tracked) | policy intent: consent, tastes, gate, lanes, vendors | operator commits only |
| config.toml (repo-local) | the named non-policy set: local paths, local posture | `spice config KEY VALUE` preview plus exact `--apply=<digest>` |
| baseline.toml (tracked) | versioned ratchet state: inherited measures, kill rate | `spice install --apply`; moved by operator commits |

Flags win over all three. Policy keys refuse `spice config` and name
spice.toml. baseline.toml is state, not intent: machine-written,
operator-moved, and outside this tree.

## The Tree

This is the complete top level with representative keys. Bare
`spice config` generates the exhaustive effective tree.

```toml
[taste.polling]                  # one table per taste; the name is identity
words = ["delay*", "poll*", "sleep*", "slept"]
message = "Respond to the real event, signal, or completion notification through a blocking call, watcher, or callback, or restructure the flow so nothing remains to await."
# this entry is the installed seed, byte-identical to TASTES.md

[consent]
posture = "queue"                # unmatched requests take this disposition

[[consent.rule]]                 # ordered; first match answers
name = "reads"
tool = "read"                    # exact or a glob such as "git *"
answer = "allow"                 # allow | deny | queue
budget = 25                      # grants before one queued renewal

[[consent.rule]]
name = "pushes"
tool = "git push*"
answer = "queue"

[gate]
[gate.walk]
skip = ["fixtures/**"]           # path-glob jurisdiction carve-outs

[board]
[board.next]
review = -100.0                  # anti-self-review coefficient
band = 2.0                       # urgency tie-break band

[lane]
life = "Drive"                   # Steer | Drive | Drain
lease = "15m"                    # claim deadline
vendor = "claude"                # armed vendor a new seat launches
deaths = 3                       # young deaths before lane::held
young = "30s"                    # an occupant dying sooner counts
hold = "30m"                     # automatic hold; lane wake clears early

[steer]
expiry = "24h"                   # directive lapse horizon
resend = ["5m", "15m", "1h"]     # escalation ladder

[vendor.claude]
binary = "claude"                # absent keys mean the vendor's own default

[vendor.codex]
binary = "codex"

[listen]
window = "90s"                   # canary absence horizon

[lock]
busy = 75                        # exit when the named lock is held
taken = 76                       # exit when the chosen shard is held
full = 77                        # exit when every shard is held

[lock.unity]
shards = 1                       # a lock is a pool of one

[lock.suite]
shards = 4                       # capacity for concurrent heavy suites

[canvas]
[canvas.pack]
rows = 40                        # packing budget per surface

[proof]
image = "sha256:2f1f7a4c9e6d0b835a1c47e9d20b6f83c15d9a047e6b3a92c8d05e714f6a9b3c"
# the compiled pin rendered; an override selects another approved digest
```

## Superseded

The prior config machinery that does not cross, each with its
successor:

| Was | Now |
| --- | --- |
| hand-enumerated inflections (`"poll, polled, polling, polls"`) | word globs: `"poll*"` |
| raw `rc.*` override passthrough | nothing; the escape hatch died with the process seam |
| generated taskrc, report, and analytics tables | nothing; the current store speaks the wire |
| urgency coefficient tables | code with an oracle table; `board.next` keeps two knobs |
| disableable named-entry registries with `false` sentinels and approval digests | builtins ship no entries; deletion is the only disable |
| env-literal and magic baselines as config | baseline.toml: state beside intent, never inside it |
| mounted repository commands (`[commands]`) | LATER.md, behind a second-real-consumer trigger |
| bare-number durations | the duration grammar: words that read aloud |
| `[locks]` compound keys (`lock_contention_exit_code`, `pool_exhaustion_exit_code`) | `[lock]` single words: busy, taken, full |
| lock `path`/`directory` overrides and `state_root` | the fixed `locks/<name>/<shard>` topology under `.spice/` |

What is absent is absent by law: no `[journal]`, `[bus]`, `[fold]`,
`[ack]`, `[dialect]`, `[tape]`, `[wire]`, or `[app]`. The tree grows
only when a domain gains a `Default` it can justify, and every growth
appears in `spice config` output in the release that ships it.
