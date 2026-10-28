# L-ROOM — conversation at a task

A room lets the operator talk at a task while agents challenge the need,
gather evidence, and construct the smallest justified commitment. An evidenced
negative conclusion is a result; activity alone is not. Each room is the
conversation attached to one [anchor](model/failures.md#ontology) task: it
indexes admitted publications whose bytes stay with their owning source, and it
is neither a filing filter nor a roster. The
[register](invariants/identity.md#identity-a-room-reads-back-from-its-owning-sources)
owns each rule summarized here.

- **Publishing.** A lane's conversation publishes in authored assistant prose:
  `MSG #TASK` records in a room, `MSG ADDRESS#TASK` also directs a lane, and
  `MSG #KEY` tells the operator about a direction it sent
  ([grammar](invariants/identity.md#steering-an-msg-packet-owns-exactly-its-body-hunk)).
  ACK answers a delivery with its response clause and NAK declines it with a
  reason; a longer or source-backed answer goes by MSG to the sender, then a
  one-line ACK, and a Design question is a typed `spice room contribute`
  record. The operator or any lane records an observation, text the room's
  fact owns, with `spice room post` or `spice task note` in any task's room
  that is not deleted; writing one makes its author no listener, and it
  changes no task generation. The operator replies to a lane's MSG item with
  `spice room reply`, sending operator direction back to that lane. When the
  source packet speaks in exactly one room,
  the reply records there too. A tool call's intent may retire a key, but
  publishes only as the narrow fallback for a message with no text block.
- **Delivery.** A directed route is a native steering directive bound to the
  logical lane: vacancy, resume, and renewal keep its key, body, and expiry for
  whichever occupant sits there next, while a replacement lane inherits
  nothing. An operator route opens an operator item in the inbox of the
  principal it answers
  ([routing](invariants/identity.md#identity-an-operator-item-reaches-the-principal-it-answers)).
  Room-only traffic reaches current listeners as one bounded catch-up aside at
  a safe boundary. Recording is not receipt.
- **Receipt, reply, and authorization stay apart.** ACK/NAK settles a delivery,
  and an operator ACK/NAK clause reaches no lane; only a reply does. Recovery
  returns a lapsed item to pending with the same key and a fresh expiry.
  Execution authorization remains preparation authority, never conversation
  traffic.
- **No push.** A room records no coordination and names no recipient. The
  operator puts a lane on a Design by telling it to `spice task capture
  HANDLE`. Deferring a held Design asks its holder to checkpoint and free
  through one stop direction.
- **Attention.** An authored room message or contribution makes its lane a
  participant; neither subscribes that lane, grants permission to post, nor
  restores listening. Routing a packet into a room and an owed receipt
  subscribe no one: steering delivers each directly, and room history stays
  readable on demand. Listening is derived from native
  work and a live stop request. Each native reason
  starts at the journal authority that created the current duty: a claim keeps
  its original win across renewal and occupant carry, ends with that claim,
  and a later claim starts fresh without replaying publications from the gap.
  A recorded Design request also keeps the asked lane listening from that
  request's position, without making it a participant, until the lane's next
  room publication answers or declines it. Nothing ages. Each room catch-up
  names the lane's reasons, and the leave command only for a room whose
  Design request it owes. `spice room leave HANDLE --clause TEXT` is an
  attributed lane act recorded once in room history and standing: the decline
  of the owed request and nothing else. A lane owing none there is refused with
  `room::leave-nothing-owed`, naming each native duty that keeps it listening,
  and nothing is written. A leave never frees a claim or settles a delivery,
  and its reply names each native duty that still holds the lane. Relevance is
  [shadow evidence](invariants/identity.md#identity-room-relevance-is-shadow-evidence-before-automation)
  the operator assesses with `spice room assess`.
- **Contribution.** A lane records its reasoning on an open Design with
  `spice room contribute HANDLE --charge=C --question N [--cite KEY...] TEXT`
  or `spice room contribute HANDLE --ask [--question N] TEXT`: a
  [typed](invariants/identity.md#identity-a-lane-contributes-typed-problem-space-records-to-an-open-design)
  charge from -2 to +2 weighing one question item of the Design statement it
  witnessed, or a question. A ±1 charge is an opinion; 0 and ±2 cite at least
  one source. Contributors start from no: the Design and each question must
  earn its place from evidence. A negative charge is the expected answer when
  a question has not; charging every question negative says the Design itself
  should not exist. This is the stance every Design request renders.
  Each KEY resolves by form: a task handle or stamp cites that
  task's statement head fact, a publication stamp a contribution in this
  room, a base-52 code that journal fact, and a hex digest a retained
  artifact. Its answer carries `source`, the exact JSON a candidate reference
  cites it by. Room history groups contributions by kind beside the full
  chronology, and `--contributions` reads only them. Any lane may contribute
  without an invitation or subscription; the record makes it a participant,
  not a listener. A contribution is problem-space record, never a decision: it
  changes no task, claim, group, candidate, or authorization, and a concluded
  Design accepts none.
- **Design statements.** A Design's pitch is the plain sentence that states
  its problem, not a one-word heading. Each unresolved question is its
  own item in the statement list; Design surfaces label that typed list
  Questions while ordinary tasks continue to present it as Acceptance.
- **Session evidence.** Design matching retains bounded evidence under the
  exact `(lane, vendor, vendor session identity)` declared by the transcript.
  A resumed occupant inherits that profile; a fresh identity starts empty, and
  an undeclared occupant contributes and inherits nothing. Each profile keeps
  the newest 32 distinct successful task completions and 64 distinct settled
  assistant messages, suppressing exact content repeats. Warming walks
  completion history and the lane tape newest first: startup takes one bounded
  window of each, and every later journal commit takes at most one more from
  the recorded cursors, until both histories reach their beginnings or a class
  is already full of newer cards. It distinguishes ready, resumably incomplete,
  and failed acquisition from an empty ready profile.
  Declaration, newly stored evidence, readiness, and Design-statement changes
  expose bounded cursor-addressed selection opportunities.
- **Design awareness.** Before allocation, a completion, settled assistant
  message, filing or revision of an open Design, completed profile warm, or
  session resume compares one continuing session's bounded evidence with every
  filed open Design. Retained task statement/completion and message/passage
  views use the same 256-statement corpus, logarithmic term frequency, smooth
  inverse document frequency, L2 normalization, and durable Design-identity
  tie break. Each candidate's cosine floor follows its maturity alone, never
  its working group: an unsteeped Design invites at the wide 0.10 floor, and a
  steeped one at the narrow 0.20 floor, since every question has been heard
  and it is close, so it asks only closely matched lanes. The reach widens
  again as soon as a later charge contests an item or a statement edit
  un-steeps it. The floor decides qualification only; finite cosine score still
  decides ranking. One opportunity selects one Design at most; one session has
  one unanswered request at most. Whether a lane is asked is decided on the
  whole statement; what it is asked is every question item still owed to it.
  While any item is contested, a lane already in a contested item's series is
  asked to re-charge it at strength 2; otherwise a lane is asked about quiet
  and unopposed items whose charge series does not hold it. A concurred item is
  never asked. The request
  lists the owed items in statement order, except that the one whose text,
  read with the problem sentence, scores best against any retained card view
  at or above the floor leads.
  A `room::noticed` record names the exact Design statement, session,
  compaction race witness, opportunity, evidence and immutable identity, the
  reach whose floor admitted the match, at most six strongest weighted
  overlaps, and the asked items. A wide invitation says so: its match was cast
  slightly wider than usual under the lower floor, so the invited lane weighs
  the listed terms accordingly. Each item is asked once per
  continuing session, and a Design with no item the lane may be asked is asked
  as a whole once, unless every current item concurs: then nothing is left to
  weigh and the Design asks nobody. An item request settles on the lane's own
  contribution against any listed item or its leave, and is not reissued in
  that session for the items it left unweighed; a later session is owed those.
  A whole-Design ask settles on any room record by the lane. Answers stay
  settled across occupants, restarts, and compaction.
  Prior contribution retains bounded room context without creating attention
  or recruitment.
  Compaction neither resets history nor creates an opportunity. A fresh vendor
  identity may receive a later ordinary offer without deleting the lane.
  The request prints the Design's stored pitch (its problem sentence). An item
  request then lists each item with its position and standing, says the lane
  may weigh any one or more, shows the contribute form once, and offers the
  supporting task as a `--cite` key; conversation evidence is not citable. A
  whole-Design request lists a bounded prefix of whole open questions. One
  contribution or `spice room leave --clause` answers it and ends its
  listening; the text says so.
  Selection, `task next`, room catch-up, and `room leave` share one owed-request
  check. A changed or closed Design, replaced session, lost or replaced
  transcript source, or lost task statement, completion, or claim evidence
  releases it on all of them. Deferring the Design ends it for as long as the
  Design stays deferred; group movement does not release an otherwise-current
  open Design request. Compaction does not either, but
  one landing between selection and recording refuses the recording;
  unrelated writes do not release it. The request grants no claim, work,
  workflow state, or wake. `spice room awareness HANDLE` shows its exact
  opportunity, evidence, statement, session, compaction, matched terms, and
  asked items.
- **Design maturity.** One bounded projection per Design folds admitted
  requests and typed contributions in journal order. It reports contribution
  and contributing-lane totals, requested reach, same-statement
  requested/contributing lane intersections, and one charge series per
  question item. A charged contribution records its item's text identity
  (trimmed, whitespace collapsed, case-folded) at admission, so a reorder
  keeps a series and a reword starts a new one. Over nonzero charges in
  journal order, an item is quiet with no charge. While only one sign has been
  seen, it concurs once three distinct lanes, or two with a strength-2 charge,
  have charged it, and is otherwise unopposed and needs more lanes of that
  sign. Once both signs have been seen, it concurs when the current one-signed
  run holds two distinct lanes one of which earlier charged the opposite sign,
  or three distinct lanes, and is otherwise contested. A run cannot bury the
  opposite side's last strength-2 charge without a strength-2 charge of its
  own. Zero charges and questions never enter a series. Steeping is
  judged against the current statement's items: fresh means no
  contribution; steeped means every current item has been charged, some
  item concurs, and none is contested, so a quiet item blocks and an
  unopposed one does not; anything else is steeping and names the lowest
  contested item, else the lowest quiet item, else the lowest unopposed item,
  with the input it needs, and counts the items still quiet. `spice room
  awareness HANDLE` and the browser room show each item's state. Steeped
  maturity makes a Design's first pass ready and narrows its invitation reach
  to the 0.20 floor. Beyond those two effects the state is
  advisory: it never changes standing, possession, grouping, selection, or
  authorization.

- **Preparation.** `task plan` declares a Plan with a `plan,review` flow.
  `task design` defaults to a Design with one open `design` phase when it
  names an open question, and to `plan,design` when it names none: its
  questions are its acceptance, so an unframed Design is sharpened first just
  as ordinary work without acceptance gets a plan bookend. It also admits
  `plan,design` or `review,design` when a lane must first sharpen or judge
  the statement under a normal claim.
  Completing that intake phase releases its claim and enters the open
  Design; before then the row is ordinary claimed work, not a Design room or
  request target. Active Plan and Design claims refuse implementation commits.
  A Design plans Plans; a Plan plans executable tasks.

  Each claimed Design pass ends with exactly one `design propose
  JUSTIFICATION` or with the existing proposal-less free/lapse abandonment.
  The client assembles the proposal from what the pass built and what the
  Design's room holds, leaving the holder only the justification and, for a
  pass that built nothing, a conclude or collapse recommendation. Its outputs
  are the pending tasks prepared under the Design that no decision resolved,
  its governed inputs their other pending dependencies, and its references
  every room contribution and earlier decision; `--cite RELATION=KEY` adds or
  relabels one. The client derives the Design from the caller's sole active
  Design claim (`--design HANDLE` when it holds several), mints the candidate
  stamp, and witnesses the Design statement and active claim heads, the
  outputs and inputs, and the room head it read; the resident records the
  completed pass horizon against those heads and releases that claim in the
  same guarded transaction. A work recommendation names one or more
  acceptance-bearing outputs of any intent still gated by the Design, such as
  ordinary tasks or Plans it constructed directly, with source-backed
  justification. A conclude or collapse recommendation names no output or
  governed input and carries the same source-backed justification. A mistaken
  undecided proposal can be retracted by its author with `design withdraw
  WITHDRAWAL_JSON`; the version stays in history but leaves the operator's
  choices.
  When a predecessor proposal's immutable receipt does not prove an active
  claim, migration preserves that missing horizon as historical absence instead
  of fabricating a claim. New proposal commands still require the exact active
  claim and cannot create that historical shape.

  A Design is offered for a pass only when it is
  [ready](invariants/identity.md#work-a-design-schedules-each-pass-from-input-after-its-last-pass):
  its first pass needs steeped maturity, except that an OOPS Design's first
  pass needs only one of its reports, and every later pass needs counted
  input recorded after the last proposal or abandonment, with no released
  work still pending. Contributions, operator room posts, returned or
  rejecting decisions, each report
  filed into an OOPS Design, and the final settlement of released work count;
  asks, notices, leaves, and group movement do not.
  Released work is the Design's prerequisite set: the accepted
  outputs it waits on, drawn as its dependencies on every graph and sheet
  without an edge the operator could edit. It settles by the fact that
  completes it: a completing `task done`, operator closure or deletion, a
  conclusion, or the clean final review of a flow that ends in review, which
  the review fact records as its completed status. Until then the Design
  gathers input, awaits the operator's decision, or waits on its released
  work, deferred work included, whatever input arrives meanwhile.

  Every surface names the same cycle state, who acts next, and the exact
  command: `task show` and `task status`, `board` rows, dry and empty drain,
  `group list`, every Design command reply, and the
  browser's Design room and working-group lights. A multi-pass Design reads,
  from the operator's chair:

  1. **Gathering input**: contributors act. Lanes contribute, the operator
     posts in the room, or tells a lane to capture it. Allocation waits, but
     a lane may claim it by name with `spice task capture DESIGN`, or take it
     from its holder with `spice task capture DESIGN --steal`, and end that
     pass with one proposal.
  2. **Ready for proposal**: a lane acts. Allocation offers the active Design;
     its holder ends the pass with one `design propose`, which releases
     the claim. Freeing or lapsing instead abandons the pass, and the cycle
     shows that the last pass ended without a proposal.
  3. **Awaiting decision**: the operator acts on the cited proposal with
     `design decide`. Accepting work releases its outputs and closes the
     rejected ones; accepting an outputless conclude or collapse
     recommendation concludes the Design; returning it, or rejecting every
     output, is counted input, so the Design is ready for its next pass at
     once.
  4. **Waiting on selected work**: the accepted work runs as the Design's
     prerequisites. Its final settlement makes the next pass ready; input
     recorded meanwhile counts then.

  A deferred Design keeps its cycle and history but no lane may possess it
  until it is woken or moved into a working group.

  A Design
  [stays open until the operator settles it](invariants/identity.md#work-a-design-stays-open-until-the-operator-concludes-it):
  ordinary lane `task done` refuses it. The operator accepts a conclude or
  collapse proposal through `design decide`; that decision stages the
  guarded `task conclude --done` or `--collapse` authority without fabricating
  an output. Direct conclude remains the normal terminal act; operator close
  and delete remain explicit emergency settlements and close gated outputs too.

  Each work output waits on every ordinary dependency to its Designs until an
  exact operator `design decide` decision assigns every output in one
  Design pass proposal an accepted or rejected verdict. The decision and
  execution-authorization record land together with removal of exactly that
  Design's edge from each accepted output. Another open Design or Plan edge
  continues to gate the output; its own decision or settlement remains
  independent. Rejected and unproposed outputs keep the deciding Design's
  edge, and the Design remains open. Rejecting every output and returning a
  proposal remove no edge. A Design edge otherwise has ordinary dependency
  authority: task dependency edits can add or remove it, and re-adding one
  admits a later exact decision without erasing prior authorization history.
  Full-history replay follows that same edge timeline: it restores the
  schema-77 crossing from retained preparation authority, keeps that restored
  edge through any dependency edit that omits it until an edit lists it or
  execution authorization removes it, and requires an ordinary dependency edit
  before a later proposal can name that output under the same Design.
  Resolution belongs to the operator; Design holders do not infer a decision,
  and working-group membership grants no
  authorization.

  Collapse, done conclusion, operator close, or delete of a Design closes every
  still-dependent pending output and gated descendant in the same write,
  except a branch still held by another open Design or Plan. Negative
  settlement of a Plan follows the same rule. A prior exact Design acceptance
  remains live because its edge is gone. Work behind a positively completed
  Plan becomes eligible through ordinary dependency settlement. Any live claim
  in the closure set refuses the whole operation and names its holder; forced
  close or delete frees those claims atomically. Migration performs the same
  closure for retained terminal, deleted, or absent parents, so no read-time
  stranded classification or repair path remains.

  A Plan is an ordinary reviewed task constructor. Every task it
  constructs, including a sub-Plan, waits on an ordinary dependency to that
  Plan until positive review, along with its other ordinary dependencies. No
  Design proposal, selection, or execution authorization is involved. A
  selected Plan therefore runs, and its own review releases its constructed
  tasks.
  An open Design with an undecided proposal
  [waits on the operator](invariants/identity.md#work-a-design-awaiting-decision-waits-on-the-operator-without-a-new-state):
  no lane is offered or woken for it, and status counts it under
  `designs awaiting-decision`.
  An output a pass constructed without proposing it awaits no decision; it
  stays gated, and its Design follows the pass cycle.

- **Organization and foreground.** The
  [working-group partition](invariants/identity.md#identity-working-groups-are-one-guarded-partition-of-designs)
  is shared repository organization read by `spice group list` and changed by
  `spice group move`, `spice group arrange`, and `spice group disband`.
  Disbanding leaves every Design ungrouped; open consoles, their order,
  drafts, speech, and focus are this browser's foreground and are never
  shared.
- **Design standing.** Grouping organizes Designs and
  [never gates possession](invariants/identity.md#work-deferral-stops-an-open-design):
  an undeferred open Design, grouped or not, is offered by the allocator once
  its pass cycle is ready, and capture and steal may reach it, so no
  Design needs a group to reach its proposal. Deferral is the one way to stop
  an open Design: `spice task defer DESIGN` keeps it from allocation, capture,
  steal, recovery, and invitations, and ends any invitation still
  owed for it. If a lane holds it, an operator deferral sends that holder one
  stop direction, queued for the lane's next seat when vacant; deferring it
  again asks again only when no stop stands for the current claim. The holder
  checkpoints and runs ordinary `spice task free DESIGN`, which parks
  unpublished commits and dirty work; its claim stays authoritative until
  then. The holder's own deferral releases its claim directly. `spice task
  wake DESIGN` (or `--claim`) makes it eligible again, and so does
  `spice group move DESIGN --to GROUP` or `--new`, which wakes it in the same
  write; its room, candidates, outputs, history, and authorization are
  unchanged throughout.
- **Opportunistic OOPS Design.** Every `task oops` report is a Plan prepared
  under it, so it is an ordinary Design with automatic intake. It has no
  candidate until a holder
  proposes: the holder triages the pending reports as a whole, gives each
  cause one acceptance-bearing output, and names the reports to reject, as its
  statement requires. It steeps and refills from counted input exactly as any
  Design does; a report is a prepared Plan, not input, so reports alone ready
  no pass. `spice task capture` names a gathering OOPS Design to open its pass
  at once. The operator decides that proposal like any other, Return
  included, and reports arriving meanwhile wait for the next pass. Its
  intent and project cannot be edited; its statement text can. The operator
  may collapse the Design; the next report then opens a new generation.

Every read — `spice room history|inbox|relevance|status`,
`spice design show`, `spice group list`, and their `spice://` resources —
joins, acknowledges, delivers, settles, and wakes nothing.

Design command bodies and the room and Design limits, with their
refusals and repairs, are
[interface contracts](interfaces.md#room-and-design-commands).

## Deliberate non-capabilities

- No room event wakes a lane. Only operator-directed steering wakes its named
  lane; a peer packet, room post, invitation, or relevance observation never
  does.
- No fallback respondent: a vacant respondent's delivery waits unheard for that
  lane's next start.
- Reads, invitations, ACK-only traffic, compaction, and renewal make no
  participant and grant no claim, read permission, team membership, or wake.
- No automated push: only the operator telling a lane to capture a Design
  asks it to hold and propose, and Design outreach asks each continuing session
  about each question item at most once; nothing else starts work, claims,
  decides, or authorizes.
- Agreement, silence, message count, elapsed time, and ACK never count as
  progress or approval.
- No timed deferral coordinates a room, and gating adds no task status.
- A working group has no name, is never empty, and is never removed as an
  object; it holds no lanes, claims, or read authority, and its only work
  authority is the active standing of the Designs it holds. A grouped design
  that is deleted or loses its Design intent stays in its group, rendered
  unavailable.
- Put aside and deactivation never force-free a claim or touch child claims
  and unrelated direction.
