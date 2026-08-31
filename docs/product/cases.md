# Case battery

Each case names the expected behavior and the source rule it exercises. They are
written to become tests; together they cover every entry in the invariant
register.

---

## Where to start

Ordered by how quietly the property fails. Passing only these does not prove the
product contract; failing any of them disproves it.

| Property | Proof |
| --- | --- |
| Evidence isolation | prove a staged native target outside the candidate checkout; candidate source and ambient build output cannot satisfy it |
| Rendering never launches | open every read surface with all lanes vacant; assert zero occupant launches |
| Layout purity | replay recorded event logs; assert identical positions |
| Zero-write on no-op | count style writes on an unchanged render; must be 0 |
| Single wake per decision | several derived-ready tasks, all lanes vacant; exactly one occupant starts |
| Starvation is an age | one ready task; starts only after the age |
| Semantic retirement | read without acking; count unchanged |
| Landing atomicity | interrupt a landing; tree is pre-merge or recoverable |
| Install reversibility | install twice, then unapply; the operator's original state returns |
| Footprint containment | land a diff touching a foreign path; refused |
| Review softness | single-agent fleet completes a review phase |
| Claim exclusivity | concurrent `next`; distinct rows |
| Claim lapse and steal | accept a lapse transaction; then explicitly steal an exact live claim and observe ownership refusal at the displaced claimant |
| Priority inheritance | critical blocked by trivial; trivial is handed out first |
| Lane persistence | renew mid-draft; draft and history survive, and the unchanged lineage retains its task policy |
| Discovery fails closed | fail enumeration; zero lanes close |
| Stream resumption | interrupt the authenticated stream; resume the exact cursor once, or replace state with one authoritative snapshot after an epoch change |
| Overlay discipline | open every menu; no layout box changes |
| Projection loudness | corrupt a projection; the answer differs from a healthy empty |
| Repair-first refusals | every refusal with a way out leads with the exact repair action; commands are runnable as printed |
| Provenance | every effective config value names its source |
| Observer inertness | run observation; assert no repo, team, claim, or hook writes |

---

## The battery

Grouped by behavior for navigation. Each situation is its own durable, descriptive identity.

### Families

- [Core work](cases/core.md) — Steering; Claims and allocation; Capacity and restart; Integration; Lifecycle; Topology; Reading surface; Audio
- [Supporting planes](cases/planes.md) — Checks; Session and counsel; Metrics; Task documents; Extension and wire; Distribution and observation; Identity and authority; Work, continued; Lifecycle, continued; Topology and surface, continued; Checks, session, extension, continued; Conduct
- [Surface and checks](cases/surface.md) — The rewind guard; The composer strip; Check mechanics
- [Release](cases/release.md) — Release as a plan; Release gates and evidence; Release notes; Publication
- [Installation and session](cases/session.md) — Installation; Receipt, resumption, and reversal; Session — sources, window, budget; Session — windows and the record; Session — meter and effort
- [Orchestration and audit](cases/conduct.md) — Affinity and anti-affinity; The prepended plan phase; Steering escalation and the shell boundary; Maxim bags; Audited by review, not settled by a case
