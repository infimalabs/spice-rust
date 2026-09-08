use std::fs;
use std::path::{Path, PathBuf};

use board::Snapshot;
use board::allocate::{Mint, mint};
use board::lapse::{LapseError, Refusal, Request, lapse};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Instant,
    LaneResidency, LeaseHead, Origin, Phase, Producer, ResidencyHead, ResidentState, TaskHandle,
    TaskHead, TranscriptSourceHead, WireVersion, Witness, WorkflowHead, body, intent,
};

fn born() -> wire::TaskStamp {
    "1kKRRsDP".parse().expect("fixture task stamp")
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
        .parse()
        .expect("fixture uuid")
}

fn instant(minute: u8) -> Instant {
    format!("2026-08-01T07:{minute:02}:00.000Z")
        .parse()
        .expect("fixture instant")
}

fn task() -> TaskHandle {
    TaskHandle::for_project("board.possession", &born())
}

fn projects() -> board::ProjectPolicy {
    board::ProjectPolicy::new(
        ["board".to_owned()],
        Vec::new(),
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy")
}

/// One non-empty proven boundary, so an opening fact carries something to lose.
///
/// An empty `heads` list compares equal to the boundary a claim would carry if
/// the field were dropped on the way through, which would let the assertion
/// below pass on exactly the shape it exists to catch.
fn opening_boundary() -> body::SourceBoundary {
    body::SourceBoundary::Proven {
        heads: vec![TranscriptSourceHead {
            lane: uuid(20),
            vendor: "claude".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid(23),
            next: body::TranscriptPosition {
                member: 1,
                offset: 4096,
            },
        }],
    }
}

/// Seed one active claim the way the claim command writes it.
///
/// The shared minter writes the fact and both heads, and the transaction
/// carries the command receipt beside them, so `command` is spent here exactly
/// as a real claim would spend it.
fn claim(journal: &mut Journal, command: u8) {
    claim_opening(
        journal,
        command,
        body::SourceBoundary::Proven { heads: Vec::new() },
    );
}

/// Seed that same claim with a caller-chosen opening boundary.
fn claim_opening(journal: &mut Journal, command: u8, source: body::SourceBoundary) {
    let heads = journal
        .read_with(|read| read.task_heads())
        .expect("heads before the claim");
    let selected = heads
        .iter()
        .find(|heads| heads.statement().body().born == born())
        .expect("the fixture task is on the board");
    let workflow = selected.workflow().body().clone();
    let expected = vec![
        Witness::Task {
            task: born(),
            head: selected.statement().position(),
        },
        Witness::Workflow {
            task: born(),
            head: selected.workflow().position(),
            phase: workflow.phase.clone(),
        },
        Witness::Claim {
            task: born(),
            head: selected.claim().map(journal::JournalHead::position),
        },
        Witness::Eligibility {
            rule: "next".parse().expect("rule"),
        },
        Witness::Residency {
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
        },
    ];
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        instant(5),
        Producer::CommandDoor,
    );
    mint(
        &mut write,
        Mint {
            task: &born(),
            actor: &actor,
            workflow: &workflow,
            project: "board.possession",
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(5),
            expiry: instant(20),
            maximum: instant(35),
            git: None,
            recovery: None,
            source,
        },
    )
    .expect("the claim mints");
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(command),
                epoch: uuid(22),
                kind: CommandKind::ClaimWin,
                expected,
                body: intent::ClaimWin {
                    rule: "next".parse().expect("rule"),
                    recovery: None,
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder: actor,
                expiry: None,
                warnings: Box::default(),
                creation_context: None,
                next: None,
            },
        )
        .expect("the claim receipt builds");
    journal.apply(write).expect("the claim commits");
}

/// Current claim and lease heads with the witnesses a possession command spends.
///
/// `Law::POSSESSION` requires exactly the claim, lease, and residency
/// witnesses, so both writers below re-read the heads instead of restating
/// positions an earlier write happened to leave behind.
fn possession(journal: &Journal) -> (ClaimHead, LeaseHead, Vec<Witness>) {
    let heads = journal
        .read_with(|read| read.task_heads())
        .expect("heads before the possession change");
    let selected = heads
        .iter()
        .find(|heads| heads.statement().body().born == born())
        .expect("the fixture task is on the board");
    let claim = selected.claim().expect("the fixture task is claimed");
    let lease = selected.lease().expect("an active claim carries its lease");
    let witnesses = vec![
        Witness::Claim {
            task: born(),
            head: Some(claim.position()),
        },
        Witness::Lease {
            task: born(),
            head: lease.position(),
        },
        Witness::Residency {
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
        },
    ];
    (claim.body().clone(), lease.body().clone(), witnesses)
}

/// Release the claim the way the resident's own free command writes it.
///
/// `spice::board_command::progress` amends the claim head it already read and
/// moves only the disposition. Building a head from scratch here would seed a
/// shape no production writer produces, so the refusal it sets up would be
/// answering the fixture rather than the board.
fn free(journal: &mut Journal, command: u8) {
    let (claim, _, expected) = possession(journal);
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("release origin"),
        instant(10),
        Producer::CommandDoor,
    );
    let freed = write
        .fact(
            body::ClaimFreed {
                task: born(),
                actor: actor.clone(),
            },
            FactCause::Root,
        )
        .expect("release fact");
    let mut next = claim;
    next.disposition = ClaimDisposition::Freed;
    write.claim_head(born(), freed, move |fact| ClaimHead { fact, ..next });
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(command),
                epoch: uuid(22),
                kind: CommandKind::ClaimFree,
                expected,
                body: intent::TaskRef { task: task() },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder: actor,
                expiry: None,
                warnings: Box::default(),
                creation_context: None,
                next: None,
            },
        )
        .expect("the release receipt builds");
    journal.apply(write).expect("the release commits");
}

/// Extend the lease the way the resident's own reclaim command writes it.
///
/// The resident amends the lease head in place, so only the renewal and expiry
/// move and the durable maximum stays where the claim set it. A lapse that
/// meets this expiry is meeting one that travelled the production path.
fn renew(journal: &mut Journal, command: u8, until: u8) {
    let (_, lease, expected) = possession(journal);
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("renewal origin"),
        instant(10),
        Producer::BusTimer,
    );
    let renewed = write
        .fact(
            body::ClaimRenewed {
                task: born(),
                epoch: uuid(22),
                until: instant(until),
                maximum: lease.maximum,
            },
            FactCause::Root,
        )
        .expect("renewal fact");
    let mut next = lease;
    next.renewal = instant(10);
    next.expiry = instant(until);
    write.lease_head(born(), renewed, move |_| next);
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(command),
                epoch: uuid(22),
                kind: CommandKind::ClaimRenew,
                expected,
                body: intent::ClaimRenew {
                    task: task(),
                    lease_seconds: 15,
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder: actor,
                expiry: None,
                warnings: Box::default(),
                creation_context: None,
                next: None,
            },
        )
        .expect("the renewal receipt builds");
    journal.apply(write).expect("the renewal commits");
}

fn lapse_request(command: u8, expiry: u8) -> Request {
    Request {
        projects: projects(),
        command: uuid(command),
        occupant: uuid(21),
        epoch: uuid(22),
        expiry: instant(expiry),
        at: instant(expiry),
        intent: intent::TaskRef { task: task() },
    }
}

fn fixture(path: Option<&Path>) -> Journal {
    seeded(path, true)
}

/// Seed the board, optionally without the residency head a lapse demands.
///
/// `observe` reads it only after the claim, the lease and the expiry chain
/// pass, so reaching that read empty needs a board that never seated one.
fn seeded(path: Option<&Path>, residency: bool) -> Journal {
    let mut journal = path.map_or_else(
        || Journal::in_memory().expect("memory journal"),
        |path| Journal::open(path).expect("file journal"),
    );
    let phase = "todo".parse::<Phase>().expect("phase");
    let origin = "task:BOARD-root".parse::<Origin>().expect("origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(0),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::TaskAdded {
                task: born(),
                title: "Lapse possession".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "board.possession".to_owned(),
                tags: vec!["priority:high".to_owned()],
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
            },
            FactCause::Root,
        )
        .expect("task fact");
    let statement_origin = origin;
    write.task_head(fact, move |position| TaskHead {
        born: born(),
        queue_origin: position,
        title: "Lapse possession".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "board.possession".to_owned(),
        tags: vec!["priority:high".to_owned()],
        dependencies: Vec::new(),
        provenance: statement_origin,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision: position,
    });
    write.workflow_head(born(), fact, move |_| WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    if residency {
        seat_residency(&mut write, fact);
    }
    journal.apply(write).expect("seed authority");
    journal
}

/// Seat the lane the fixture claims from, occupied by the fixture occupant.
fn seat_residency(write: &mut JournalWrite, fact: journal::PendingFact) {
    write.residency_head(fact, |_| ResidencyHead {
        epoch: uuid(22),
        state: ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![LaneResidency {
            lane: uuid(20),
            lineage_id: uuid(20),
            name: None,
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid(21)),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: uuid(20),
            policy: wire::intent::LineageLife::Drain,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
        renewals: Vec::new(),
    });
}

fn claimed() -> Journal {
    let mut journal = fixture(None);
    claim(&mut journal, 31);
    journal
}

#[test]
fn lapse_preserves_lease_and_makes_the_task_allocatable_again() {
    let mut journal = claimed();
    let before = Snapshot::read(&journal, &projects()).expect("claimed snapshot");
    let statement = before.entries()[0].statement().clone();
    let workflow = before.entries()[0].workflow().clone();
    let lease = before.entries()[0].lease().expect("lease").clone();

    let expired = lapse(&mut journal, lapse_request(32, 20)).expect("lapse claim");
    assert_eq!(expired.task(), &task());
    assert_eq!(expired.first(), expired.last());
    let after = Snapshot::read(&journal, &projects()).expect("lapsed snapshot");
    assert_eq!(
        after.entries()[0].claim().expect("claim").disposition,
        ClaimDisposition::Lapsed
    );
    assert_eq!(after.entries()[0].statement(), &statement);
    assert_eq!(after.entries()[0].workflow(), &workflow);
    assert_eq!(after.entries()[0].lease(), Some(&lease));
    let facts = journal.fact_snapshot().expect("lapsed facts");
    assert_eq!(facts.facts().len(), 3);
    assert_eq!(facts.facts()[2].fact.kind(), wire::FactKind::ClaimLapsed);
    assert_eq!(journal.verify().expect("authority verifies").receipts, 2);

    assert!(
        board::eligible_entry(after.entries(), &after.entries()[0], &Actor::Lane(uuid(20))),
        "a lapsed claim leaves the task allocatable again"
    );
}

#[test]
fn timer_and_authority_refusals_leave_the_journal_unchanged() {
    let cases = [
        {
            let mut request = lapse_request(40, 20);
            request.intent.task = TaskHandle::for_project(
                "board.possession",
                &"1kKRRsDQ".parse().expect("missing task stamp"),
            );
            let expected = Refusal::MissingTask(request.intent.task.clone());
            (request, expected)
        },
        {
            let mut request = lapse_request(41, 20);
            request.at = instant(19);
            let expected = Refusal::Early {
                at: request.at,
                expiry: request.expiry,
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(42, 20);
            request.expiry = instant(19);
            let expected = Refusal::StaleExpiry {
                stored: instant(20),
                fired: request.expiry,
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(43, 20);
            request.occupant = uuid(24);
            let expected = Refusal::ClaimOccupant {
                holding: uuid(21),
                named: uuid(24),
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(44, 20);
            request.epoch = uuid(23);
            let expected = Refusal::ClaimEpoch {
                holding: uuid(22),
                named: uuid(23),
            };
            (request, expected)
        },
    ];
    for (request, expected) in cases {
        refuses(&mut claimed(), request, &expected);
    }

    let mut journal = claimed();
    free(&mut journal, 45);
    refuses(
        &mut journal,
        lapse_request(46, 20),
        &Refusal::Inactive(ClaimDisposition::Freed),
    );
}

#[test]
fn a_claim_without_its_lease_is_a_typed_zero_write_refusal() {
    let mut journal = fixture(None);
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        instant(5),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::ClaimWon {
                task: born(),
                actor: actor.clone(),
                lane: uuid(20),
                occupant: uuid(21),
                epoch: uuid(22),
                cell: Some(wire::ClaimCell {
                    project: "board.possession".to_owned(),
                    phase: "todo".parse().expect("phase"),
                }),
                until: instant(20),
                git: None,
                recovery: None,
                source: body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("claim fact");
    write.claim_head(born(), fact, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell: wire::ClaimCell {
            project: "board.possession".to_owned(),
            phase: "todo".parse().expect("phase"),
        },
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    journal.apply(write).expect("seed malformed claim");

    let before = journal.fact_snapshot().expect("before malformed refusal");
    let heads = Snapshot::read(&journal, &projects()).expect("heads before malformed refusal");
    assert!(matches!(
        lapse(&mut journal, lapse_request(48, 20)),
        Err(LapseError::Refused(Refusal::MissingLease))
    ));
    assert_eq!(
        journal.fact_snapshot().expect("after malformed refusal"),
        before
    );
    assert_eq!(
        Snapshot::read(&journal, &projects()).expect("heads after malformed refusal"),
        heads
    );
}

/// Assert one refusal by variant and payload, and that it wrote nothing.
///
/// A refusal that moved a fact or a head is a defect the variant assertion
/// cannot see, and whole-`Refusal` equality stops a pass on an adjacent guard.
fn refuses(journal: &mut Journal, request: Request, expected: &Refusal) {
    let before = journal.fact_snapshot().expect("facts before refusal");
    let heads = Snapshot::read(journal, &projects()).expect("heads before refusal");
    match lapse(journal, request) {
        Err(LapseError::Refused(observed)) => assert_eq!(&observed, expected),
        other => panic!("expected {expected:?}, found {other:?}"),
    }
    let facts = journal.fact_snapshot().expect("facts after refusal");
    assert_eq!(facts, before);
    let settled = Snapshot::read(journal, &projects()).expect("heads after refusal");
    assert_eq!(settled, heads);
}

/// Amend the lease head the way a renewal does, with a caller-chosen drift.
///
/// `renew` above moves only the fields a renewal moves; these fixtures need the
/// ones it never moves, so the drift lands on the head that was read.
fn drift_lease(journal: &mut Journal, epoch: CanonicalUuid, drift: impl FnOnce(&mut LeaseHead)) {
    let (_, lease, _) = possession(journal);
    let mut write = JournalWrite::new(
        Actor::Lane(uuid(20)),
        "task:BOARD-a1".parse().expect("drift origin"),
        instant(10),
        Producer::BusTimer,
    );
    let fact = write
        .fact(
            body::ClaimRenewed {
                task: born(),
                epoch,
                until: lease.expiry,
                maximum: lease.maximum,
            },
            FactCause::Root,
        )
        .expect("drifted renewal fact");
    let mut next = lease;
    drift(&mut next);
    write.lease_head(born(), fact, move |_| next);
    journal.apply(write).expect("the drifted lease commits");
}

/// Amend the residency head the way the resident amends its own.
///
/// `spice::resident::shutdown` clones the head it read, moves one field and
/// writes it back beside a transition fact; one built here would answer itself.
fn drift_residency(journal: &mut Journal, drift: impl FnOnce(&mut ResidencyHead)) {
    let observed = journal
        .read_with(|read| read.residency_head())
        .expect("residency read")
        .expect("the fixture seats a residency head");
    let mut next = observed.body().clone();
    drift(&mut next);
    let mut write = JournalWrite::new(
        Actor::Operator,
        "resident:up".parse().expect("residency origin"),
        instant(10),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::UpDraining {
                epoch: next.epoch,
                occupants: 1,
            },
            FactCause::Root,
        )
        .expect("residency transition fact");
    write.residency_head(fact, move |_| next);
    journal.apply(write).expect("the drifted residency commits");
}

#[test]
fn a_task_that_was_never_claimed_refuses_and_claiming_it_makes_it_lapse() {
    let mut journal = fixture(None);
    refuses(&mut journal, lapse_request(50, 20), &Refusal::MissingClaim);
    let mut restored = claimed();
    lapse(&mut restored, lapse_request(52, 20)).expect("the restored claim lapses");
}

/// A lease left behind by an epoch change is refused before the named epoch is.
///
/// The request names an epoch matching neither head, so both epoch guards in
/// `admit_expiry` are live at once. It runs claim-versus-lease first; reversing
/// that calls the command stale when the board itself is what drifted.
#[test]
fn a_lease_epoch_that_left_the_claim_behind_is_named_before_the_request_epoch() {
    let mut journal = claimed();
    drift_lease(&mut journal, uuid(26), |lease| lease.epoch = uuid(26));
    let mut request = lapse_request(53, 20);
    request.epoch = uuid(27);
    refuses(
        &mut journal,
        request,
        &Refusal::ClaimLeaseEpoch {
            claim: uuid(22),
            lease: uuid(26),
        },
    );
}

#[test]
fn an_expiry_past_the_durable_maximum_refuses_and_one_inside_it_lapses() {
    let mut journal = claimed();
    drift_lease(&mut journal, uuid(22), |lease| lease.expiry = instant(40));
    refuses(
        &mut journal,
        lapse_request(54, 40),
        &Refusal::IncoherentLease,
    );
    let mut coherent = claimed();
    drift_lease(&mut coherent, uuid(22), |lease| lease.expiry = instant(30));
    lapse(&mut coherent, lapse_request(55, 30)).expect("a coherent lease lapses");
}

#[test]
fn a_board_that_seated_no_residency_refuses_and_a_seated_one_lapses() {
    let mut journal = seeded(None, false);
    claim(&mut journal, 56);
    refuses(
        &mut journal,
        lapse_request(57, 20),
        &Refusal::MissingResidency,
    );
    let mut seated = claimed();
    lapse(&mut seated, lapse_request(58, 20)).expect("a seated board lapses");
}

#[test]
fn moved_residency_identity_does_not_strand_an_expired_claim() {
    let cases: [fn(&mut ResidencyHead); 4] = [
        |head| head.epoch = uuid(28),
        |head| head.lanes.clear(),
        |head| head.lanes[0].disposition = "released".to_owned(),
        |head| head.lanes[0].occupant = Some(uuid(29)),
    ];
    for (index, drift) in cases.into_iter().enumerate() {
        let mut journal = claimed();
        drift_residency(&mut journal, drift);
        lapse(
            &mut journal,
            lapse_request(60 + u8::try_from(index).expect("fixture index fits"), 20),
        )
        .expect("stale claim authority lapses after residency identity moves");
        let snapshot = Snapshot::read(&journal, &projects()).expect("lapsed snapshot");
        assert_eq!(
            snapshot.entries()[0]
                .claim()
                .expect("claim remains")
                .disposition,
            ClaimDisposition::Lapsed
        );
    }
}

#[test]
fn nonseated_residency_refuses_before_identity_is_considered() {
    let mut journal = claimed();
    drift_residency(&mut journal, |head| {
        head.state = ResidentState::Draining;
        head.lanes.clear();
    });
    refuses(
        &mut journal,
        lapse_request(64, 20),
        &Refusal::ResidentState(ResidentState::Draining),
    );
}

#[test]
fn duplicate_receipt_rolls_back_and_independent_connections_share_lapse() {
    let store = Store::new();
    let mut seeded = fixture(Some(store.path()));
    claim(&mut seeded, 31);
    drop(seeded);
    let mut first = Journal::open(store.path()).expect("first connection");
    let mut second = Journal::open(store.path()).expect("second connection");

    let before = first.fact_snapshot().expect("before duplicate");
    let heads = Snapshot::read(&first, &projects()).expect("heads before duplicate");
    assert!(matches!(
        lapse(&mut first, lapse_request(31, 20)),
        Err(LapseError::Commit(_))
    ));
    assert_eq!(first.fact_snapshot().expect("duplicate rolls back"), before);
    assert_eq!(
        Snapshot::read(&first, &projects()).expect("duplicate preserves heads"),
        heads
    );

    lapse(&mut second, lapse_request(52, 20)).expect("second connection lapses");
    let shared = Snapshot::read(&first, &projects()).expect("first sees the lapse");
    assert!(
        board::eligible_entry(
            shared.entries(),
            &shared.entries()[0],
            &Actor::Lane(uuid(20))
        ),
        "the connection that did not lapse still sees the task allocatable"
    );
    assert_eq!(
        first.verify().expect("shared authority verifies").receipts,
        2
    );
}

/// A timer armed at a superseded expiry writes nothing; the current one still ends the claim.
///
/// This coverage lived in the renew suite, because a renewal was the only
/// thing that could move an expiry out from under an armed timer. What it
/// proves is a property of lapse, not of renew: the timer carries the expiry
/// it was armed with, and authority refuses it once the lease has moved past
/// that instant. Losing it would leave a timer left over from before a
/// renewal free to end a claim its holder had already extended.
///
/// The refusal is read for both instants rather than matched loosely, because
/// a stale timer and a current one differ only in which expiry they name.
#[test]
fn a_superseded_expiry_is_refused_and_the_renewed_one_still_lapses() {
    let mut journal = claimed();
    renew(&mut journal, 52, 25);

    let before = journal.fact_snapshot().expect("before the stale timer");
    let heads = Snapshot::read(&journal, &projects()).expect("heads before the stale timer");
    match lapse(&mut journal, lapse_request(53, 20)) {
        Err(LapseError::Refused(observed)) => assert_eq!(
            observed,
            Refusal::StaleExpiry {
                stored: instant(25),
                fired: instant(20),
            }
        ),
        other => panic!("expected a stale expiry refusal, found {other:?}"),
    }
    assert_eq!(
        journal
            .fact_snapshot()
            .expect("the stale timer writes nothing"),
        before
    );
    assert_eq!(
        Snapshot::read(&journal, &projects()).expect("the stale timer preserves heads"),
        heads
    );

    let renewed = Snapshot::read(&journal, &projects()).expect("renewed heads");
    board::Active::admit(&renewed.entries()[0], &Actor::Lane(uuid(20)), instant(24))
        .expect("the renewed lease is still active past the expiry the timer named");

    lapse(&mut journal, lapse_request(54, 25)).expect("the renewed expiry lapses");
    let reallocatable = Snapshot::read(&journal, &projects()).expect("heads after the lapse");
    assert!(
        board::eligible_entry(
            reallocatable.entries(),
            &reallocatable.entries()[0],
            &Actor::Lane(uuid(20))
        ),
        "a lease lapsed at its renewed expiry leaves the task allocatable again"
    );
}

/// The opening fact keeps its boundary and no renewal records one of its own.
///
/// This coverage arrived in the renew suite, but what it constrains is the fact
/// schema and the writer that emits it, not the module the suite named. A claim
/// opens against a proven transcript position; a renewal only moves the lease,
/// so a boundary on `ClaimRenewed` would assert a provenance the renewal never
/// established. The writer exercised here is the one the resident actually uses,
/// which is the whole reason the property still has a subject after the
/// unreached module is gone.
#[test]
fn an_opening_boundary_survives_renewals_that_record_none_of_their_own() {
    let mut journal = fixture(None);
    claim_opening(&mut journal, 31, opening_boundary());
    renew(&mut journal, 32, 25);
    renew(&mut journal, 33, 30);

    let facts = journal.fact_snapshot().expect("renewed facts");
    let kinds = facts
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            wire::FactKind::TaskAdded,
            wire::FactKind::ClaimWon,
            wire::FactKind::ClaimRenewed,
            wire::FactKind::ClaimRenewed,
        ],
    );
    let won: body::ClaimWon = facts.facts()[1]
        .fact
        .read()
        .expect("the opening fact reads back through the registry");
    assert_eq!(won.source, opening_boundary());

    // Read as text rather than as a field, because the absence being proven is
    // the absence of a field: an added one would compile everywhere and only
    // show up as a key nothing here would otherwise look at.
    for position in [2, 3] {
        let renewal: body::ClaimRenewed = facts.facts()[position]
            .fact
            .read()
            .expect("the renewal reads back through the registry");
        let encoded = String::from_utf8(wire::encode(&renewal).expect("encode the renewal"))
            .expect("a renewal encodes as text");
        assert!(
            !encoded.contains("source"),
            "renewal {position} recorded a boundary of its own: {encoded}",
        );
    }
}

/// A task command whose transaction appends no fact is refused, not receipted.
///
/// A journal write can carry a command receipt without carrying a fact, because
/// the receipt is assembled without one. That shape is unrepresentable: a
/// receipt records the span of facts it caused, and a write with no fact has no
/// span to name, so the commit path reaches for a first and last position that
/// are not there. Removing this refusal does not make such a write succeed
/// quietly -- it aborts the process inside position assignment. What the
/// refusal buys is a typed refusal a producer can report at the door instead of
/// a panic underneath it, and it has to come from the transaction itself,
/// because every producer assembles its receipt before it can know whether its
/// facts survived the guards.
///
/// The witness positions are fixture values. Command law admits a witness list
/// by kind rather than by position, and this transaction is refused before any
/// guard reads, so nothing here depends on which position they name.
#[test]
fn a_task_command_that_appends_no_fact_is_refused_instead_of_receipted() {
    let mut journal = claimed();
    let before = journal.fact_snapshot().expect("facts before").facts().len();
    let holder = Actor::Lane(uuid(20));
    let head = wire::Seq::new(2).expect("fixture head position");

    let mut write = JournalWrite::new(
        holder.clone(),
        "task:BOARD-a1".parse::<Origin>().expect("origin"),
        instant(10),
        Producer::BusTimer,
    );
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(34),
                epoch: uuid(22),
                kind: CommandKind::ClaimRenew,
                expected: vec![
                    Witness::Claim {
                        task: born(),
                        head: Some(head),
                    },
                    Witness::Lease { task: born(), head },
                    Witness::Residency {
                        lane: uuid(20),
                        occupant: uuid(21),
                        epoch: uuid(22),
                    },
                ],
                body: intent::ClaimRenew {
                    task: task(),
                    lease_seconds: 15,
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder,
                expiry: None,
                warnings: Box::default(),
                creation_context: None,
                next: None,
            },
        )
        .expect("the receipt itself is well formed");

    let error = journal
        .apply(write)
        .expect_err("a transaction appending no fact cannot apply");
    assert!(
        error.to_string().contains("must append at least one fact"),
        "a factless transaction was refused for another reason: {error}",
    );
    assert_eq!(
        journal.fact_snapshot().expect("facts after").facts().len(),
        before,
        "a refused transaction appended a fact",
    );
}

struct Store {
    path: PathBuf,
}

impl Store {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "spice-board-lapse-{}-{}.sqlite",
                std::process::id(),
                uuid(60)
            )),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            self.path.with_extension("sqlite-wal"),
            self.path.with_extension("sqlite-shm"),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove lapse fixture: {error}"),
            }
        }
    }
}
