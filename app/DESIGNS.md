# Design rooms

A Design's room carries its read view, operator conclusion and proposal
decisions, coordination, and lane contributions. Its
[standing](#design-standing) and its place in working-group consoles belong to
the [team console](CONSOLE.md).

## Design standing

Every read derives one standing from the Design's workflow and the
working-group partition alone. Board rows, `spice task show`, `spice group
list`, `spice room status`, and the browser agree:

Standing terms run from durable to volatile: lifecycle first, then
working-group activity, then pass cycle or claim state. The most volatile term
therefore comes last, limiting label jitter as the Design moves.

- **open, passive**: open and in no working group. No lane can claim it or be
  coordinated onto it until the operator moves it into a group.
- **open, active**: open and in a working group, so allocation, capture, and
  room coordination may reach it.
- **concluded done**: the operator concluded it done.
- **concluded collapsed**: the operator collapsed it, with the durable reason
  that collapse recorded.
- **deleted**.

A concluded or deleted Design keeps its standing while a saved group still
holds it: group membership decides activity only for an open Design. The
command bar's Design rail counts open active and passive Designs without
dimming or changing the tab edge for passive standing. The work overview and
dependency cards label a passive Design **Passive**, waiting on the operator.

## Reading a Design

Any Design, open, completed, or closed, opens for reading in its task
overview: its design tile in the Spice menu's Working groups catalog, a
Design-intent task's detail, and a Design permalink all open the same view.
Below the task's state, its **Design room** section is the
task room every task detail shows, with its composer and its conversation
newest first, extended with the room's [standing](#design-standing), its
positive [conclusion control](#concluding-a-design), and every immutable
[proposal version and decision](#deciding-a-design-proposal), resolved through
the Design's birth stamp under its current handle, with the permalink beside
the heading. The conversation and proposal versions stay readable in every
standing. Opening or reading the view moves no group, joins, acknowledges,
authorizes, and wakes nothing, and commits no journal fact. Operator decision
controls appear only on an open Design.
Lanes refine a Design's statement through ordinary task edit authority, `spice
task modify --pitch` for its problem sentence (the why), `task modify
--description` or a `spice://task/HANDLE/description` write for the how, never
from the browser, which hosts only principal sessions.

## Concluding a Design

Only the operator concludes a Design. While it is open and the resident offers
conclusion, the read view's **Conclusion** section previews **done**. Done keeps
outputs already selected by proposal decisions released and is refused while
an output still owes the operator's selection. **Submit conclusion** witnesses
the Design's statement, workflow, and claim heads, the same contract as
`spice task conclude`. A refusal, including a live claim or moved head, keeps
the preview beside it; standing moves only when the board shows the conclusion.
Collapse is not a second conclusion shortcut in this panel: it is a decision
on the exact proposal version whose premise failed.

## Deciding a Design proposal

The read view's **Design pass proposals** section lists every proposal recorded
under the Design, newest first, without replacing earlier versions. Each card
names its immutable proposal identity and journal fact, then leads with the
recommendation and the Design's own pitch. Below that head come the lines the
operator decides on, never collapsed: one per work output with its handle, its
pitch, its kind and status, its verdict control, and a Plan's acceptance; then
the governed inputs the same way without a verdict. Source references follow
as a count whose exact, unresolved links disclose on demand, and the authored
justification is collapsed beneath them; cited question results, critiques,
review coverage, and every earlier decision and execution authorization
follow. The room's proposal card presents the same head and lines with the
reference count alone. A referenced task or question result absent from the
current projection is shown as unavailable; an exact source identity is shown
as an unresolved link rather than being called available. Concluded Designs
keep the same cards read-only. Output lines identify declared Plans and
Designs by intent, and undeclared work as a task with its authored flow. A
version its author withdrew leaves the open list and open count for a separate
**Withdrawn versions** history naming its author, age, reason, and withdrawal
fact, with no decision control; the view never picks among open versions by
recency.

On an open Design, an operator chooses one action for an exact undecided
version:

- **Resolve outputs** assigns **Accept** or **Reject** to every output with
  native radios. No output starts decided: **Accept all** and **Reject all**
  above the list mark every output at once, and Preview refuses, naming the
  undecided outputs, until each carries a verdict, so no silent default ever
  grants or withholds authorization. Each
  output may carry its own comment, and the version may carry an optional
  overall comment. Only accepted outputs are released; rejected outputs stay
  gated, and the Design remains open. Rejecting every output releases nothing,
  like a historical Return decision.
- **Collapse** requires a reason naming why this version's premise did not
  hold. It atomically records the decision and concludes the Design collapsed.

**Preview decision** mutates nothing. **Submit decision** sends one
`preparation decide` command witnessing the Design's statement, workflow, and
proposal heads, the exact Design pass proposal fact, governed input statements,
accepted output transitions, and absence of the new decision and output
authorizations. Resolution atomically records every verdict and comment plus
authorization for exactly the accepted outputs; there is no coverage-record
prerequisite, later Accept stage, or separate Authorize action. Coverage
remains readable evidence. Rejected-output comments and the overall comment
become input to the Design's current or next holder.

The first click marks that exact version pending before the socket write, so a
double activation cannot submit twice. While it is pending, every other
proposal version's decision controls stay disabled; its exact reply settles
before those controls unlock. Each proposal version keeps its own local
verdicts, comments, action, preview, and resident refusal while the operator
edits or previews another version. Editing that version again clears its
notice. A successful reply reports only that the write landed: standing,
accepted outputs, and conclusion continue to come from committed room and
board authority. A decided version shows its immutable decision and offers no
second submission. A stale proposal, moved head, live claim, concluded Design,
foreign output, incomplete verdict set, or mismatched version refusal
stays beside the unchanged draft and requires a fresh preview.
Passive versus active working-group standing changes no operator decision
authority. Native radio, text, and button controls preserve keyboard order in
normal and narrow layouts.

## Preparation authority

A room for a controlling task also exposes its Design pass proposals, critiques,
review coverage, decisions, and execution-authorization records in immutable
journal order. Each record preserves exact outputs, governed inputs, source
link, every output verdict and comment, the overall comment, and accepted
outputs; review evidence and execution authorization remain distinct.
Historical Select, Return, and Collapse records remain readable. The same
authority is available through bounded `spice preparation show HANDLE` and
`spice://task/HANDLE/preparation` pages.
Reading or opening these records joins, acknowledges, and changes nothing.

## Room coordination

Start and Resume are explicit coordination, not browser-open events. The
operator names a seated occupant's current base-52 address plus the unresolved
criterion, bounded contribution, and stop condition. Spice records one ordinary
durable direction with the room marker; the recipient then acquires the exact
anchor or related preparation through ordinary native task possession. Filing
visibility keeps hidden controller anchors out of automatic allocation; the
room command adds no other eligibility or possession rule.

Put aside cancels only unanswered coordination for that room. If the anchor has
a native holder, it sends that holder a checkpoint/free direction and shows a
stop request until that exact claim is completed, freed, or lapsed; it never
force-frees work or releases child claims and unrelated direction. The answer
asks its initiating consumer to hide the room locally; the team console keeps
the design's composer, since the group partition decides what it shows. Room
history, drafts, groups, selected outputs, other browsers, and repository
authority remain unchanged.

The design menu's **Coordinate** form, beneath the room's standing, offers
these commands while the Design is open and active. A passive Design shows
instead that no lane can claim it or be coordinated onto it until it moves
into a working group, and a concluded or deleted one shows no form. A lane
address field suggests each occupied seat's address with its lane name;
Criterion, Contribution, and Stop when complete Start and Resume, and Put
aside needs only the room. Each
button stays disabled while the resident does not offer its command. Before
submitting, the browser resolves the address against the board's residency
and addresses with the resident's own rule: an unknown, vacant, or simulated
seat refuses in place with the resident's reason and explanation, and nothing
is written. The submission
witnesses the anchor's statement, workflow, and claim heads and the target
seat's residency, so a moved anchor or seat refuses with the resident's
reason. The composer's report names the request key and target, or after Put
aside the stop request's key, target, and claim, which stays held until its
holder settles it.

The native command surface is `spice room status HANDLE`,
`spice room start|resume HANDLE ADDRESS --criterion TEXT --contribution TEXT
--stop TEXT`, and `spice room put-aside HANDLE`. Nothing repeats a Start or
Resume on the operator's behalf: each asks one lane to hold and propose, and
only another operator command, or Design outreach's own once-per-item-per-session
bound, asks again.

## Room standing

Each design's menu and read view open with the room's standing, derived from
the room snapshot, its live facts, the board, and lane topology on every render:

- **Next action**: the one next useful action that authority supports, as one
  sentence naming who acts next and on what, never a command line.
- **Latest coordination**: its request and that request's disposition, and
  whether a put-aside followed it.
- **Lanes**: each involved lane with whether it is seated, vacant, or unknown,
  plus one chip per reason, each naming its source:
  - authored message and contribution history;
  - a recorded Design request the lane has not answered or declined;
  - native work;
  - a live coordination request;
  - a claim-bound stop request;
  - a leave with the lane's clause, the decline of a Design request it owed.
  Nothing ages. The lane's next room publication, a contribution or a leave,
  answers a Design request it postdates and ends that request's listening; a
  lane owing no request has nothing to leave. Authored messages and
  contributions never create listening; only a later Design request restores
  it. History and native duties stay.
- **Relevance**: the resident's shadow relevance observations, each with its
  classification, progress standing, explanation, exceptions, uncertainty,
  source coverage, and operator verdict.

Agreement, silence, message count, elapsed time, ACK, and ANS never read as
progress or approval; the section states only what moves the Design. It only
renders inside an explicitly opened menu or read view: a status or relevance
update changes its text in place and never mounts a room, moves a console, or
wakes a lane.

## Design contributions

A lane contributes to an open Design's problem space with one typed record:
a charge from -2 to +2 against one of the Design's question items, or a
question, with its text and the transcript, journal, or artifact sources it
cites. A ±1 charge is an opinion; 0 and ±2 cite at least one source. Retained
posts from before charges name no item and read as Design-level. The app reads
each charge to the operator as a vote: strongly for, for, evidence, against,
or strongly against. A
contribution is
lane-authored evidence for the operator's judgment, never authority: it
moves no task, claim, working group, candidate, decision, or authorization,
and it wakes nobody. Lanes contribute with `spice room contribute` from their
own sessions; the operator reads contributions and decides through the
ordinary Design controls.

The Design read view shows each contribution once, in the room conversation
at its journal position, with its lane, time, vote, text, and every cited
source; the Conversation header's count is the sheet's only contribution
count. The question item a contribution weighs reads as a `Q N` link that
focuses item N in the sheet's Contract list, so the item's words appear in
that list and nowhere else on the card. The resident resolves each cited
source for the shown Design, and the card says whether it is available or why
not (missing, hidden, changed, unreadable, or retained in an unsupported
shape), the same states any actor's `spice room history` prints. It also
gives the item's text as the link's title while the statement the
contribution witnessed is still current, and otherwise says the item belongs
to an earlier statement, because the journal keeps no superseded statement
text.
Both come from the `room::contribution-read` read, which any authenticated
session may issue and the app issues once per contribution of a shown Design
that cites a source or names an item, at most eight in flight, and again
after a reconnect; it resolves through the same resident path as
`spice room history`. This read-only surface
never creates or answers the session-profile Design requests delivered through
a lane's own session.

The read view's **Design maturity** panel sits beside the Design's Standing,
in one row directly under the room heading, and stacks beneath it below 720px.
It derives from the bounded per-Design measure the room projection folds
incrementally from streamed requests and contributions, never by folding
rendered room history. It shows the state word and summary, recent
statement-head request yield, and folded older-head and unattributed
historical totals; each current question item's state (quiet, unopposed,
contested, or concurred) with its votes and lanes reads beside that question
in the sheet's Contract list. Fresh has no contributions; steeped means
some current item concurs and none is contested, and the summary otherwise
names the item and the vote it needs next. Steeped maturity makes the Design's
first pass ready for a lane, which the standing's pass cycle shows; it is not a
danger or workflow state, and maturity never blocks or authorizes a Design
decision.

An open Design's view offers the reader no contribution control. A lane learns
of an open Design matching its claimed work from the resident's private notice
and `spice room awareness`. Each names the Design's title and stored pitch,
the claimed task and its matched terms, each distinct term once and at most
six, along with the statement revision and the typed `room contribute`
commands, and states that the invitation creates no assignment and requires no
response.
