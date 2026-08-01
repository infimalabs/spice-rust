# CLI

## Preamble

Every leaf returns the same typed result to text and `--json` renderers:

| Result | Meaning | Exit |
| --- | --- | --- |
| `Reply` | The requested read or committed transition completed. | `0` |
| `Refusal` | The request was understood but a named rule, witness, posture, or missing capability declined it. | `2` |
| `Fault` | The protocol, journal, toolchain, or environment failed. | `1` |

A text refusal is one sentence on stderr in the form
`spice: <reason>; <remedy>`. It names a required resident, failed
witness, missing tool, withdrawn release, or apply requirement precisely
enough to act. A stale epoch is a `Fault` that names both epochs. The
requested transition makes no authority change; the bus may retain
bounded disposable refusal telemetry for the chronology view.

Direct intent executes immediately after capability and witness checks.
Authored input does not: its bare command emits a normalized ordered
preview, and only the matching apply form may commit it. A changed plan
refuses as stale instead of applying different bytes. Document apply is
stricter still: `--apply=<digest>` must name the exact digest printed by
the preview. Text and JSON are renderings of the same reply DTO; flags do
not create a second behavior.

Three independent labels state hermetic posture:

| Label | Contract |
| --- | --- |
| `daemonless` | Runs without `spice up`; it cannot append facts, change heads, or acquire or renew a claim. |
| `offline` | Opens no network connection. |
| `toolful` | May launch declared external tools; the command names their owner, bounds, cancellation, and network posture. |

Labels compose. `daemonless` does not imply `offline`, and neither means
toolchain-free.

## Daemonless

Every leaf in this band is `daemonless`; additional labels are explicit.

| Verb | Labels | Contract |
| --- | --- | --- |
| `spice watch [ROOT]` | `offline` | Detect and render live vendor sessions in the terminal without a socket, journal append, or hidden drift alarm; absence and decode residue are accounted on screen, never smoothed over. |
| `spice retro [LANE\|LOCUS\|PATH]` | `offline` | Replay preserved tape spans, loci, absence, and decode residue from disk; a lane resolves to its current recorded tape. |
| `spice doctor` | `offline` | Report environment, configuration, optional RTK, and per-verb tool posture; print and append nothing. |
| `spice version` | `offline` | Print the binary hash, wire version, journal schema version, and embedded app digest; the schema read is `JournalRead`-only and refuses during an unfinalized migration. |
| `spice install [--apply=<digest>\|--unapply]` | `offline` | Bare invocation previews hooks, generated skill, tracked configuration, and unit templates and prints the plan digest; `--apply=<digest>` writes only that exact plan, while `--unapply` reverses only the durable install record. |
| `spice lock` | `offline` | List configured resource locks and pool shards with holder pid, cwd, and start instant, ranked and packed. |
| `spice lock run NAME [--shard N] -- CMD` | `toolful` | Hold the named lock, the exact shard, or the first free pool shard for the child command's life; contention never blocks — it exits with the configured busy, taken, or full code, and the child is the declared tool. |
| `spice restore verify BUNDLE` | `offline` | Verify bundle identity, checksums, enumerated filenames, journal schema, and tape manifest without mutation. |
| `spice gate [--at REV]` | `toolful` | Run the current gate engine with current policy over the staged tree or the tree selected by `--at`; old verdicts remain receipts, never executable arms. |
| `spice gate camp` | `toolful` | Audit every ratchet distribution for pile-ups and report camping without moving a floor. |
| `spice gate kill` | `toolful` | Run the deflaked mutation battery and compare the confirmed kill rate with its shrink-only floor. |
| `spice gate receipt` | `toolful` | Render the receipt that pins tree, gate-set digest, and policy digest. |
| `spice proof` | `offline`, `toolful` | Prove the target manifest in the sealed container with vendored crates, an empty cache, and network disabled. |
| `spice proof replay` | `offline`, `toolful` | Replay the sealed battery and re-derive its evidence against the manifest without authority mutation. |
| `spice release notes` | `offline` | Render curated notes from the Git range and the sealed release manifest with its detached receipts; no journal open, no hidden fact read. |

## Resident

Except for `spice up`, each leaf in this band opens the current socket
with `{wire, capability, epoch}`. A stale client faults by naming both
epochs; a missing resident refuses by naming `spice up`.

| Verb | Contract |
| --- | --- |
| `spice up` | Run the resident in the foreground. Acquire the kernel writer lock before opening the journal, finish the one forward migration, mint the epoch, publish discovery, and own the bus until `down`; a second resident refuses by naming the seated holder. |
| `spice down` | Drive `Seated → Quiescing → Draining → Terminating → Killing → Verifying → Dark`. Each edge has a deadline and admitted-command set; claims finish or remain honestly preserved, ACKs are never forged, and durable residue is named before authority is released. |
| `spice board [FAMILY]` | With no argument, return the whole authoritative board ranked and packed. A family or handle returns full depth, also packed, preserving the mid-zoom. |
| `spice board graph VIEW [--at BORN]` | Render one named board diagram from authoritative heads; `--at` bounds the rendering at a creation stamp. |
| `spice board handout OUTPUT [--at BORN]` | Render every named board diagram and compose the one shareable document at `OUTPUT`. |
| `spice task next` | Run the one deterministic, explainable eligibility query and atomically win its claim. This is the sole allocation path. |
| `spice task add TITLE [--project PROJECT]` | Add one task as direct intent. Structured input or multiple rows belong to `doc`. |
| `spice task done [--validation TEXT]` | Complete the phase held by the caller's active claim after re-proving task, workflow, claim, lease, lane, occupant, and resident epoch witnesses; carry validation in the transition. |
| `spice task review HANDLE` | Record the assigned review verdict after re-proving the active claim and anti-self-review assignment witnesses. |
| `spice task note HANDLE TEXT` | Append an observation without replacing the task statement. |
| `spice task defer HANDLE UNTIL` | Set the task's wakeable deferral under its current workflow head. |
| `spice task wake HANDLE` | Clear the current deferral and return the task to ordinary eligibility. |
| `spice task free HANDLE` | Release the winning claim so the allocator may consider the task again. |
| `spice task oops TEXT` | File friction as wakeable work and carry the observation as its note; oops has no second meaning. |
| `spice task depends HANDLE AFTER...` | Wire dependency edges between existing tasks as direct intent under exact head witnesses; one edge needs no document round-trip. |
| `spice doc [PATH\|-]` | Classify and normalize a task document, then preview its complete plan, board barrier, and digest without mutation. |
| `spice doc [PATH\|-] --apply=<digest>` | Commit only the exact normalized preview at its witnessed board barrier as one journal transaction. |
| `spice doc ledger HANDLE` | Export the task family in the dialect's one normal form. |
| `spice lane` | Return seats, occupants, lives, leases, lineage, and current tape loci as a packed list. |
| `spice lane add [WORKTREE] [--life Steer\|Drive\|Drain] [--vendor NAME]` | Seat a lane in an operator-created worktree, mint its capability, bind its occupant epoch, and select its one policy life and armed vendor; `lane::started` records desired and actual launch identity. Spice creates no worktree. |
| `spice lane drain ID` | Stop admission, finish or preserve claims, retire only directives with evidence, end the occupant, and verify the lane is drained. |
| `spice lane wake ID` | Clear a `lane::held` pause early and return the lane to seating eligibility as `lane::resumed`. |
| `spice steer LANE TEXT` | Append one durable directive to a lane; a typed disposition — semantic ACK, authenticated lane command, operator cancel, or lapse — is the only retirement. |
| `spice steer cancel ID` | Withdraw a live directive as direct operator intent; the lineage retires as `steer::canceled`. |
| `spice inbox [LANE]` | Return the pending directives the named or current lane must hear next, ranked and packed. |
| `spice recall [LANE]` | Return one ranked, bounded, typed account of operator intent, active and next work, pending ACKs, lineage, recent failures, and repository state at a composite read barrier; it is also injected at seat, renewal, and compaction. |
| `spice consent` | Return pending ACP requests, named rules, budget state, and disposition rates as a packed queue. |
| `spice consent allow ID` | Grant one pending request as direct operator intent under its current disposition and budget witnesses. |
| `spice consent deny ID` | Deny one pending request as direct operator intent under its current disposition witness. |
| `spice consent policy` | Render the closed ordered decision table, each first match, and remaining named budgets. |
| `spice taste` | Return active tastes and evidence ages as a packed list. |
| `spice taste add TEXT` | Create the proposal work for a tracked taste addition; membership changes only by operator commit. |
| `spice taste drop NAME` | Create the proposal work for a tracked taste removal, carrying evidence and reason. |
| `spice trust` | Render executable digests and their `pended`, `granted`, or `revoked` state. |
| `spice trust grant DIGEST` | Grant the exact executable digest under operator capability. |
| `spice trust revoke DIGEST` | Revoke the exact executable digest under operator capability. |
| `spice backup OUTPUT` | Quiesce authority long enough to produce one journal snapshot plus an enumerated tape manifest at `OUTPUT`; the bundle carries verbatim tape and is operator-held secret material. |
| `spice restore BUNDLE [--apply=<digest>]` | Bare invocation verifies and previews the ordered restore and prints its plan digest; `--apply=<digest>` runs the lock-held recovery protocol for exactly that plan — stop admission, close, exchange, reopen, new epoch, `journal::restored`. |
| `spice journal` | Return the immutable fact stream ranked and packed. |
| `spice journal replay [FOLD]` | Re-derive every fold or the named fold into a new generation, then publish it transactionally after horizon checks. |
| `spice journal reindex` | Rebuild journal-maintained indexes under the writer guard and verify them before publication; it never manufactures authoritative heads. |
| `spice journal verify` | Check envelope decode, head agreement, command receipts, wake cursor, and thread integrity without mutation. |
| `spice fold` | Return every fold's six answers, generation, cursor, horizon, and exact recovery command as a packed list. |
| `spice thread REF` | Walk one `origin` and `cause` chain end to end through the journal-maintained thread index. |
| `spice canvas` | Return the loopback URL and the caller's read, steer, and grant capabilities; no browser DTO differs from wire. |
| `spice release publish [--apply=<digest>]` | Bare invocation previews publication and verifies HEAD, sealed release manifest, detached proof receipts, and trust witnesses, printing the plan digest; `--apply=<digest>` publishes only that exact plan. |

## Agent

These leaves exist for hook and generated-skill machinery; agents use the
resident band for shared work.

| Verb | Labels | Contract |
| --- | --- | --- |
| `spice run -- CMD` | `daemonless`, `toolful` | Deliver pending steering on stderr, then execute the command natively; the operator's own command is the declared tool. Gate measurement owns the budget; the wrapper never refuses runtime on its own, and its hot path performs no Git discovery, SQLite open, or extra spawn — geometry and steering arrive from the hook environment and rendered inbox. Without the resident it leaves one durable per-lane degradation marker. |
| `spice ack KEY [--decline]` | — | Retire a pending directive by authenticated lane command when the vendor absorbs the semantic ACK prose; the disposition fact records command evidence instead of a transcript locus. |
| `spice config [KEY [VALUE]]` | `daemonless`, `offline` | Read or set only the keys the generated registry marks local. A bare read returns the effective tree with per-key provenance; any key not marked local refuses and names `spice.toml`. |

## Discipline

- **Origin minting.** The resident derives actor, lane, and origin rights
  from the accepted connection capability. Direct operator intent roots
  its own origin; work under a claim inherits the task origin; a response
  to steering carries the directive lineage. Caller-authored attribution
  remains data and cannot mint the envelope.

- **Configuration is not policy.** `spice config` may write only the
  keys the generated per-key registry marks local — release one marks
  exactly `[lock]`, `[canvas.pack]`, and `[listen].window`. Consent,
  taste, gate, lane, steer, board, vendor, and proof policy come from
  tracked `spice.toml`; a config attempt refuses the policy key and
  names the tracked file. Trust is journal state under operator
  capability, never a configuration table.

- **Defaults are compiled.** No packaged defaults file exists. Each
  builtin is a typed default sealed by the binary and proof hash; effective
  values layer builtin, tracked, local, then flag, and every read names
  provenance.

- **Every listing packs.** Every list crosses as `Packed<T>` with ranked
  rows, explicit overflow, and horizon. Text and app renderers may shorten
  only by showing the marker; a gate may refuse overflow where completeness
  is required.

- **Degrade once.** If no resident exists, `spice run` executes the command
  natively and publishes one durable marker per lane. The next resident
  recognizes each once, appends one fact, and escalates one directive
  rather than emitting a diagnostic for every command.

- **Renewal is automatic.** The resident renews a live claim from observed
  liveness and cadence within its maximum lease. Compaction triggers a
  renewal check and recall injection; a successor may carry an ancestor's
  claim but cannot renew the ancestor's token.

- **Document apply is the sole editor.** Only
  `spice doc PATH --apply=<digest>` mints `task::edited`. Task add creates,
  task note appends, and phase commands advance workflow without becoming
  competing authored editors.

- **Doctor appends nothing.** `spice doctor` reports local truth only.
  Resident recognition owns `vendor::armed`, canary facts, and any durable
  reaction to a changed executable surface.

- **Meters live on magnitude.** Effort, pressure, residue, consent rates,
  and related meters render on canvas magnitude. `spice fold` reports
  derivation health and never substitutes plumbing for product data.

- **Missing verbs stay missing.** Multi-user authority and agent-to-agent
  messaging are never surfaces. A northbound ACP server, query DSL, plugin
  API, multi-machine authority, adversarial isolation, Windows, tape
  pruning, a migration bridge, lane worktree provisioning, mounted
  repository commands, settled-statement correction, and steering
  attachments wait in `LATER.md` for their named evidence triggers.

- **The contract table is generated.** Every clap leaf has one row naming
  principal, authority touched, result class, and facts minted, and every
  row names its battery owner. The standing gate fails both an uncovered
  leaf and a contract row with no command.

## Session

Two shells share this session; `up` remains a foreground process in the
resident shell.

```console
operator$ spice version
operator$ spice install
operator$ spice install --apply=4b6c1de92a70f3b8a51c96de07c14488a6f2f1e5cf9d3b25c6b9be9a5f27c4e1
resident$ spice up
operator$ spice lane add ../spice-lane1 --life Drive
operator$ spice board
operator$ spice doc plan.md
operator$ spice doc plan.md --apply=7a19c4b7df55c869739af3f2a7bf9d06cf88cbce3fd1562448db73c271981d27
operator$ spice task next
operator$ spice steer lane1 "prefer the closed enum"
operator$ spice inbox lane1
operator$ spice recall lane1
operator$ spice thread ack:20260731T120000Z-q7f3
operator$ spice consent
operator$ spice gate --at HEAD
operator$ spice proof
operator$ spice release publish
operator$ spice release publish --apply=9c0f4a2e6d81b35720cd8ef14ab97c6531e08d4b2f76a19c58e3b0d7c2a64f18
operator$ spice down
```
