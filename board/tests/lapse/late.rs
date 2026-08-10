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

/// One residency drift and the refusal the drifted head must produce.
type ResidencyCase = (u8, fn(&mut ResidencyHead), Refusal);

#[test]
fn residency_refusals_name_the_seat_they_read_and_write_nothing() {
    let cases: [ResidencyCase; 5] = [
        (
            59,
            |head| head.state = ResidentState::Draining,
            Refusal::ResidentState(ResidentState::Draining),
        ),
        (
            60,
            |head| head.epoch = uuid(28),
            Refusal::StaleResidentEpoch {
                seated: uuid(28),
                named: uuid(22),
            },
        ),
        (
            61,
            |head| head.lanes.clear(),
            Refusal::MissingLane(uuid(20)),
        ),
        (
            62,
            |head| head.lanes[0].disposition = "released".to_owned(),
            Refusal::LaneDisposition {
                lane: uuid(20),
                disposition: "released".to_owned(),
            },
        ),
        (
            63,
            |head| head.lanes[0].occupant = Some(uuid(29)),
            Refusal::Displaced {
                lane: uuid(20),
                seated: Some(uuid(29)),
                named: uuid(21),
            },
        ),
    ];
    for (command, drift, expected) in cases {
        let mut journal = claimed();
        drift_residency(&mut journal, drift);
        refuses(&mut journal, lapse_request(command, 20), &expected);
    }
}

/// A residency that fails two guards reports the first, not the worst.
///
/// `admit_residency` is four early returns, so a head both off its seat and
/// missing the claimed lane reports only one. The state is the honest answer:
/// the lane is gone because the resident is draining, not because of the seat.
#[test]
fn the_residency_chain_reports_the_first_guard_that_fires() {
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
                warnings: Vec::new(),
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
