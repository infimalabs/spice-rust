use super::*;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
const LINEAGE_A: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const LINEAGE_B: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const OCCUPANT_A: &str = "018f82d4-6f59-7d06-92e3-bce913116304";
const OCCUPANT_B: &str = "018f82d4-6f59-7d06-92e3-bce913116305";
const SOURCE: &str = "018f82d4-6f59-7d06-92e3-bce913116306";
const OTHER_SOURCE: &str = "018f82d4-6f59-7d06-92e3-bce913116307";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116308";
const REVIEW_CLOSE_OFFSET: u64 = 200;

#[test]
fn ordinary_completion_review_and_renewal_share_one_half_open_fold() {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.claim(occupant_a(), proven(None));
    facts.batch(
        source(),
        ORIGIN,
        pos(1, 100),
        vec![sample(1, 10, 10, 11)],
        0,
    );
    facts.push(body::ClaimRenewed {
        task: task(),
        epoch: uuid(EPOCH),
        until: instant(30),
        maximum: instant(40),
    });
    facts.done(proven(Some(pos(1, 100))));
    facts.push(body::TaskPhased {
        task: task(),
        phase: phase("review"),
    });
    facts.push(body::TaskAssigned {
        task: task(),
        reviewer: wire::Actor::Lane(lane()),
        phase: phase("review"),
    });
    facts.claim(occupant_a(), proven(Some(pos(1, 100))));
    facts.batch(
        source(),
        pos(1, 100),
        pos(1, REVIEW_CLOSE_OFFSET),
        vec![sample(1, 100, 10, 7)],
        0,
    );
    // This batch starts exactly at the closing locus and must not enter review.
    facts.batch(
        source(),
        pos(1, REVIEW_CLOSE_OFFSET),
        pos(1, 250),
        vec![sample(1, REVIEW_CLOSE_OFFSET, 10, 99)],
        0,
    );
    facts.reviewed(proven(Some(pos(1, REVIEW_CLOSE_OFFSET))));

    let answer = derive(&facts.snapshot()).expect("effort derives");
    assert_eq!(answer.phases.len(), 2);
    assert_eq!(answer.phases[0].renewals, 1, "renewal does not split");
    assert_available(&answer.phases[0], 11);
    assert_available(&answer.phases[1], 7);
    assert_eq!(answer.phases[1].phase, phase("review"));
    assert_eq!(answer.aggregates.len(), 2, "phase partitions stay separate");
    assert!(answer.phases.iter().all(|phase| {
        phase
            .segments
            .iter()
            .all(|segment| segment.from.offset < REVIEW_CLOSE_OFFSET)
    }));
}

#[test]
fn carry_and_lineage_movement_split_at_their_recorded_source_loci() {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.claim(occupant_a(), proven(None));
    facts.batch(source(), ORIGIN, pos(1, 50), vec![sample(1, 10, 10, 3)], 0);
    facts.push(body::LaneArranged {
        lane: lane(),
        from_lineage_id: lineage_a(),
        lineage_id: lineage_b(),
        before: lane(),
        sources: vec![body::LaneSourceBoundary {
            lane: lane(),
            source: proven(Some(pos(1, 50))),
        }],
    });
    facts.batch(
        source(),
        pos(1, 50),
        pos(1, 100),
        vec![sample(1, 60, 10, 5)],
        0,
    );
    facts.push(body::ClaimCarried {
        task: task(),
        lane: lane(),
        occupant: occupant_b(),
        ancestor: occupant_a(),
        epoch: uuid(EPOCH),
        source: proven(Some(pos(1, 100))),
    });
    facts.session(occupant_b(), "sonnet", "high");
    facts.batch(
        source(),
        pos(1, 100),
        pos(1, 150),
        vec![sample(1, 120, 10, 7)],
        0,
    );
    facts.done(proven(Some(pos(1, 150))));

    let answer = derive(&facts.snapshot()).expect("effort derives");
    let phase = &answer.phases[0];
    assert_available(phase, 15);
    assert_eq!(
        phase
            .segments
            .iter()
            .map(|segment| (segment.occupant, segment.lineage))
            .collect::<Vec<_>>(),
        vec![
            (occupant_a(), lineage_a()),
            (occupant_a(), lineage_b()),
            (occupant_b(), lineage_b()),
        ]
    );
    assert_eq!(
        phase
            .segments
            .iter()
            .map(|segment| segment.session.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("session-018f82d4-6f59-7d06-92e3-bce913116304"),
            Some("session-018f82d4-6f59-7d06-92e3-bce913116304"),
            Some("session-018f82d4-6f59-7d06-92e3-bce913116305"),
        ]
    );
}

#[test]
fn interrupted_and_unproven_attempts_stay_visible_but_nonnumeric() {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.claim(
        occupant_a(),
        SourceBoundary::Incomplete {
            reason: "fixture boundary unavailable".to_owned(),
        },
    );
    facts.push(body::ClaimFreed {
        task: task(),
        actor: wire::Actor::Lane(lane()),
    });
    facts.claim(occupant_a(), proven(None));
    facts.push(body::ClaimLapsed {
        task: task(),
        occupant: occupant_a(),
        expiry: instant(40),
    });
    facts.claim(occupant_a(), proven(None));
    facts.push(body::TaskDeleted {
        task: task(),
        reason: "fixture deletion".to_owned(),
        claimed: Some(wire::Actor::Lane(lane())),
    });

    let answer = derive(&facts.snapshot()).expect("effort derives");
    assert_eq!(answer.phases.len(), 3);
    assert!(answer.aggregates.is_empty());
    assert_reason(&answer.phases[0], EffortUnavailableCode::ClaimFreed);
    assert_reason(&answer.phases[0], EffortUnavailableCode::IncompleteBoundary);
    assert!(
        reasons(&answer.phases[0])
            .iter()
            .any(|reason| reason.detail.contains("fixture boundary unavailable"))
    );
    assert_reason(&answer.phases[1], EffortUnavailableCode::ClaimLapsed);
    assert_reason(&answer.phases[2], EffortUnavailableCode::TaskDeleted);
}

#[test]
fn replacement_mixed_identity_and_overflow_are_each_withheld() {
    let mut replacement = Facts::new();
    replacement.task();
    replacement.lane();
    replacement.session(occupant_a(), "sonnet", "high");
    replacement.claim(occupant_a(), proven(Some(pos(1, 50))));
    replacement.done(proven_head(other_source(), pos(1, 50)));
    let answer = derive(&replacement.snapshot()).expect("replacement derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::SourceReplaced);

    let mut mixed = Facts::new();
    mixed.task();
    mixed.lane();
    mixed.session(occupant_a(), "sonnet", "high");
    mixed.claim(occupant_a(), proven(None));
    mixed.batch(source(), ORIGIN, pos(1, 50), vec![sample(1, 10, 10, 1)], 0);
    mixed.push(body::ClaimCarried {
        task: task(),
        lane: lane(),
        occupant: occupant_b(),
        ancestor: occupant_a(),
        epoch: uuid(EPOCH),
        source: proven(Some(pos(1, 50))),
    });
    mixed.session(occupant_b(), "opus", "high");
    mixed.batch(
        source(),
        pos(1, 50),
        pos(1, 100),
        vec![sample(1, 60, 10, 1)],
        0,
    );
    mixed.done(proven(Some(pos(1, 100))));
    let answer = derive(&mixed.snapshot()).expect("mixed derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::MixedLaunch);

    let mut overflow = Facts::new();
    overflow.task();
    overflow.lane();
    overflow.session(occupant_a(), "sonnet", "high");
    overflow.claim(occupant_a(), proven(None));
    overflow.batch(
        source(),
        ORIGIN,
        pos(1, 100),
        vec![sample(1, 10, 10, u64::MAX), sample(1, 30, 10, 1)],
        0,
    );
    overflow.done(proven(Some(pos(1, 100))));
    let answer = derive(&overflow.snapshot()).expect("overflow derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::ArithmeticOverflow);
}

#[test]
fn complete_no_usage_is_numeric_zero_but_an_untagged_launch_is_not() {
    let mut zero = Facts::new();
    zero.task();
    zero.lane();
    zero.session(occupant_a(), "sonnet", "high");
    zero.claim(occupant_a(), proven(None));
    zero.batch(source(), ORIGIN, pos(1, 50), Vec::new(), 1);
    zero.done(proven(Some(pos(1, 50))));
    let answer = derive(&zero.snapshot()).expect("zero derives");
    assert_available(&answer.phases[0], 0);
    assert_eq!(answer.aggregates[0].usage_sum.input_tokens, 0);

    let mut untagged = Facts::new();
    untagged.task();
    untagged.lane();
    untagged.claim(occupant_a(), proven(None));
    untagged.batch(source(), ORIGIN, pos(1, 50), vec![sample(1, 10, 10, 1)], 0);
    untagged.done(proven(Some(pos(1, 50))));
    let answer = derive(&untagged.snapshot()).expect("untagged derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::UntaggedLaunch);
    assert_reason(
        &answer.phases[0],
        EffortUnavailableCode::SessionDisagreement,
    );
    assert!(answer.aggregates.is_empty());

    let mut carried = Facts::new();
    carried.task();
    carried.lane();
    carried.session(occupant_a(), "sonnet", "high");
    carried.claim(occupant_a(), proven(None));
    carried.push(body::ClaimCarried {
        task: task(),
        lane: lane(),
        occupant: occupant_b(),
        ancestor: occupant_a(),
        epoch: uuid(EPOCH),
        source: proven(None),
    });
    carried.session(occupant_b(), "opus", "high");
    carried.done(proven(None));
    let answer = derive(&carried.snapshot()).expect("zero-usage carry derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::MixedLaunch);
    assert!(answer.aggregates.is_empty());
}

#[test]
fn a_replacement_claim_closes_the_displaced_attempt_and_accounts_the_winner() {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.session(occupant_b(), "sonnet", "high");
    facts.claim(occupant_a(), proven(None));
    facts.claim(occupant_b(), proven(None));
    facts.batch(source(), ORIGIN, pos(1, 50), vec![sample(1, 10, 10, 8)], 0);
    facts.done(proven(Some(pos(1, 50))));

    let answer = derive(&facts.snapshot()).expect("replacement claim derives");
    assert_eq!(answer.phases.len(), 2);
    assert_reason(&answer.phases[0], EffortUnavailableCode::ClaimStolen);
    assert_reason(&answer.phases[0], EffortUnavailableCode::MissingEnd);
    assert_available(&answer.phases[1], 8);
    assert_eq!(answer.phases[1].segments[0].occupant, occupant_b());
    assert_eq!(answer.aggregates.len(), 1);
    assert_eq!(answer.aggregates[0].count, 1);
}

#[test]
fn converged_absent_boundaries_are_visible_and_refused_attempts_invent_no_phase() {
    let mut refused = Facts::new();
    refused.task();
    refused.lane();
    refused.session(occupant_a(), "sonnet", "high");
    let answer = derive(&refused.snapshot()).expect("no-fact refusal derives");
    assert!(
        answer.phases.is_empty(),
        "a refusal authored no lifecycle fact"
    );
    assert!(answer.aggregates.is_empty());

    let mut converged = Facts::new();
    converged.task();
    converged.lane();
    converged.session(occupant_a(), "sonnet", "high");
    converged.claim(occupant_a(), SourceBoundary::unrecorded());
    converged.done(SourceBoundary::unrecorded());
    let answer = derive(&converged.snapshot()).expect("converged boundary derives");
    assert_eq!(answer.phases.len(), 1);
    assert_reason(&answer.phases[0], EffortUnavailableCode::IncompleteBoundary);
    assert!(
        reasons(&answer.phases[0])
            .iter()
            .all(|reason| reason.code != EffortUnavailableCode::MissingStart),
        "a converged absence keeps the lifecycle while withholding only its number"
    );
    assert!(answer.aggregates.is_empty());
}

/// Usage is attributed only when both bounding facts carry a proven boundary.
///
/// The control arm is load-bearing rather than decoration. It holds the same
/// fixture with both bounds proven and reports a number, so neither withheld
/// arm below can be passing because the fixture ran short of transcript, a
/// session declaration, or lineage. Each withheld arm leaves the other bound
/// proven, so severing the check on one bound reds exactly one of them and the
/// other bound cannot mask it. The pre-existing converged-absence test cannot
/// carry this clause because it makes both bounds incomplete at once.
///
/// These arms also settle that withholding is read from the recorded boundary
/// and never from live state. The lane is seated, claimed, and fully accounted
/// at the moment the fold runs, which the control proves by reporting from it,
/// so a fold that filled a missing boundary in from current lane membership
/// would report a number here where both withheld arms require withheld.
#[test]
fn either_bound_alone_withholds_a_fully_accounted_interval() {
    let control = accounted(proven(None), proven(Some(pos(1, 100))));
    assert_available(&control, 11);

    let unproven_opening = accounted(
        incomplete("opening streams unnamed"),
        proven(Some(pos(1, 100))),
    );
    assert_reason(&unproven_opening, EffortUnavailableCode::IncompleteBoundary);

    let unproven_closing = accounted(proven(None), incomplete("closing streams unnamed"));
    assert_reason(&unproven_closing, EffortUnavailableCode::IncompleteBoundary);
}

/// A withheld interval reports the reason recorded on the boundary itself.
///
/// The two arms differ only in the reason they record, and each is reported
/// verbatim as the tail of the detail rather than replaced by a reason the
/// fold composed. The arm carrying a reason other than
/// [`SourceBoundary::UNRECORDED`] is withheld through the same code and the
/// same path as the migrated one, which is what proves no arm is keyed to the
/// constant the decode migration itself writes.
///
/// A fact stored without the field needs no arm of its own here. Decode
/// converges that absence onto exactly one value before the fold sees it, so
/// the fold matches two variants and holds no third; the convergence is pinned
/// for all four carriers by `wire/tests/source_boundary.rs` and is cited rather
/// than re-tested.
#[test]
fn a_withheld_boundary_reports_whichever_reason_it_recorded() {
    let authored = "fixture could not name the lane's streams";
    assert_ne!(
        authored,
        SourceBoundary::UNRECORDED,
        "the authored arm must not be the migration constant"
    );

    let recorded = accounted(incomplete(authored), proven(Some(pos(1, 100))));
    assert!(
        boundary_detail(&recorded).ends_with(authored),
        "recorded reason is carried verbatim, found {:?}",
        boundary_detail(&recorded)
    );
    assert!(
        !boundary_detail(&recorded).contains(SourceBoundary::UNRECORDED),
        "an authored reason is never replaced by the migration constant"
    );

    let migrated = accounted(SourceBoundary::unrecorded(), proven(Some(pos(1, 100))));
    assert!(
        boundary_detail(&migrated).ends_with(SourceBoundary::UNRECORDED),
        "the migrated reason travels the same path, found {:?}",
        boundary_detail(&migrated)
    );
}

#[test]
fn malformed_usage_source_gaps_and_broken_carries_refuse_by_name() {
    let mut malformed = Facts::new();
    malformed.task();
    malformed.lane();
    malformed.session(occupant_a(), "sonnet", "high");
    malformed.claim(occupant_a(), proven(None));
    let mut empty_span = sample(1, 10, 10, 2);
    empty_span.spans.clear();
    malformed.batch(source(), ORIGIN, pos(1, 50), vec![empty_span], 0);
    malformed.done(proven(Some(pos(1, 50))));
    let answer = derive(&malformed.snapshot()).expect("malformed usage derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::MalformedUsage);

    let mut gap = Facts::new();
    gap.task();
    gap.lane();
    gap.session(occupant_a(), "sonnet", "high");
    gap.claim(occupant_a(), proven(None));
    gap.batch(
        source(),
        pos(1, 10),
        pos(1, 50),
        vec![sample(1, 20, 10, 2)],
        0,
    );
    gap.done(proven(Some(pos(1, 50))));
    let answer = derive(&gap.snapshot()).expect("source gap derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::SourceGap);

    let mut carry = Facts::new();
    carry.task();
    carry.lane();
    carry.session(occupant_a(), "sonnet", "high");
    carry.claim(occupant_a(), proven(None));
    carry.push(body::ClaimCarried {
        task: task(),
        lane: lane(),
        occupant: occupant_b(),
        ancestor: occupant_b(),
        epoch: uuid(EPOCH),
        source: proven(None),
    });
    carry.done(proven(None));
    let answer = derive(&carry.snapshot()).expect("broken carry derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::BrokenCarry);
    assert_reason(
        &answer.phases[0],
        EffortUnavailableCode::OccupantDisagreement,
    );
}

#[test]
fn a_pre_claim_batch_cannot_be_reversed_into_the_phase_interval() {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.batch(source(), ORIGIN, pos(1, 50), vec![sample(1, 10, 10, 99)], 0);
    facts.claim(occupant_a(), proven(None));
    facts.done(proven(Some(pos(1, 50))));

    let answer = derive(&facts.snapshot()).expect("journal-order reversal derives");
    assert_reason(&answer.phases[0], EffortUnavailableCode::SourceGap);
    assert!(answer.aggregates.is_empty());
}

#[test]
fn task_selection_recomputes_aggregates_from_only_the_retained_rows() {
    let identity = EffortIdentity {
        vendor: vendor(),
        model: "sonnet".to_owned(),
        effort: "high".to_owned(),
    };
    let available = |task: TaskHandle, opening: u64, input_tokens: u64| EffortPhaseAnswer {
        task,
        phase: phase("todo"),
        phase_index: 0,
        opening: Seq::new(opening).expect("fixture sequence is nonzero"),
        closing: Seq::new(opening + 1),
        renewals: 0,
        opening_boundary: proven(None),
        closing_boundary: Some(proven(None)),
        segments: Vec::new(),
        availability: EffortAvailability::Available {
            identity: identity.clone(),
            usage: EffortUsage {
                input_tokens,
                ..zero_usage()
            },
            wall_ms: 1_000,
        },
    };
    let selected_task = task();
    let other_task: TaskHandle = "EFFORT-b2".parse().expect("fixture task");
    let phases = vec![
        available(selected_task.clone(), 1, 3),
        available(other_task, 3, 97),
    ];
    let answer = EffortAnswer {
        aggregates: aggregate(&phases).expect("unfiltered aggregate"),
        phases,
    };

    let answer = select_task_effort(
        answer,
        &wire::intent::EffortRead {
            task: Some(selected_task.clone()),
        },
    )
    .expect("selection recomputes");
    assert_eq!(answer.phases.len(), 1);
    assert_eq!(answer.phases[0].task, selected_task);
    assert_eq!(answer.aggregates.len(), 1);
    assert_eq!(answer.aggregates[0].count, 1);
    assert_eq!(answer.aggregates[0].usage_sum.input_tokens, 3);
    assert_eq!(answer.aggregates[0].usage_average.input_tokens, 3);
}

struct Facts {
    rows: Vec<wire::Envelope<wire::AnyFact>>,
}

impl Facts {
    fn new() -> Self {
        Self { rows: Vec::new() }
    }

    fn push<F: wire::Body>(&mut self, body: F) -> Seq {
        let seq = Seq::new(u64::try_from(self.rows.len()).expect("fixture length fits u64") + 1)
            .expect("fixture sequence is nonzero");
        let typed = wire::Fact::new(body);
        let bytes = wire::encode(&typed).expect("typed fixture fact encodes");
        let fact = wire::decode(&bytes).expect("fixture reads as opaque fact");
        self.rows.push(wire::Envelope {
            v: wire::WireVersion::CURRENT,
            seq,
            at: instant(seq.get()),
            actor: wire::Actor::Spice,
            origin: "task:EFFORT-a1".parse().expect("fixture origin"),
            cause: (seq.get() > 1)
                .then(|| Seq::new(seq.get() - 1).expect("prior fixture sequence")),
            fact,
        });
        seq
    }

    fn task(&mut self) {
        self.push(body::TaskAdded {
            task: task(),
            uuid: uuid("018f82d4-6f59-7d06-92e3-bce913116399"),
            title: "fixture effort".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "fold.effort".to_owned(),
            tags: Vec::new(),
            depends: Vec::new(),
            flow: vec![phase("todo"), phase("review")],
            phase: phase("todo"),
            revision: 1,
        });
    }

    fn lane(&mut self) {
        self.push(body::LaneSeated {
            lane: lane(),
            lineage_id: lineage_a(),
            capability: uuid("018f82d4-6f59-7d06-92e3-bce913116398"),
            name: None,
            worktree: "/worktree".to_owned(),
            life: "drive".to_owned(),
            vendor: vendor(),
        });
    }

    fn session(&mut self, occupant: CanonicalUuid, model: &str, effort: &str) {
        self.push(body::LaneSessionDeclared {
            lane: lane(),
            occupant,
            worktree: "/worktree".to_owned(),
            vendor: vendor(),
            launch: Some(body::LaneLaunchIdentity::new(model, effort)),
            epoch: uuid(EPOCH),
            stream: "stdout".to_owned(),
            identity: format!("session-{occupant}"),
            member: 1,
            offset: 0,
            length: 1,
        });
    }

    fn claim(&mut self, occupant: CanonicalUuid, source: SourceBoundary) {
        self.push(body::ClaimWon {
            task: task(),
            actor: wire::Actor::Lane(lane()),
            lane: lane(),
            occupant,
            epoch: uuid(EPOCH),
            until: instant(50),
            git: None,
            source,
        });
    }

    fn batch(
        &mut self,
        source: CanonicalUuid,
        from: TranscriptPosition,
        to: TranscriptPosition,
        usage: Vec<body::TranscriptUsage>,
        non_usage_records: u64,
    ) {
        self.push(body::JournalIngested {
            lane: lane(),
            vendor: vendor(),
            stream: "session".to_owned(),
            source,
            supersedes: None,
            from,
            to,
            usage,
            non_usage_records,
            residue_records: 0,
        });
    }

    fn done(&mut self, source: SourceBoundary) {
        self.push(body::TaskDone {
            task: task(),
            validation: "fixture validated".to_owned(),
            git: None,
            source,
        });
    }

    fn reviewed(&mut self, source: SourceBoundary) {
        self.push(body::TaskReviewed {
            task: task(),
            reviewer: wire::Actor::Lane(lane()),
            verdict: body::Verdict::Clean,
            finding: None,
            git: None,
            source,
        });
    }

    fn snapshot(self) -> journal::FactSnapshot {
        journal::FactSnapshot::from_facts(self.rows).expect("fixture snapshot is contiguous")
    }
}

fn sample(member: u64, offset: u64, length: u64, input_tokens: u64) -> body::TranscriptUsage {
    body::TranscriptUsage {
        turns: 1,
        input_tokens,
        cached_input_tokens: 2,
        cache_write_input_tokens: 3,
        output_tokens: 4,
        reasoning_output_tokens: Some(5),
        spans: vec![body::TranscriptSpan {
            member,
            offset,
            length,
        }],
        ordinal: 0,
    }
}

fn proven(next: Option<TranscriptPosition>) -> SourceBoundary {
    next.map_or_else(
        || SourceBoundary::Proven { heads: Vec::new() },
        |next| proven_head(source(), next),
    )
}

fn proven_head(source: CanonicalUuid, next: TranscriptPosition) -> SourceBoundary {
    SourceBoundary::Proven {
        heads: vec![wire::TranscriptSourceHead {
            lane: lane(),
            vendor: vendor(),
            stream: "session".to_owned(),
            source,
            next,
        }],
    }
}

/// One fully accounted interval whose only variable is its two boundaries.
///
/// The lane is seated, its session declared, and a single batch covers the
/// whole interval, so with both bounds proven the fold reports a number. Every
/// caller varies only the boundaries, which is what makes a withheld answer
/// evidence about the recorded boundary instead of about absent transcript.
fn accounted(opening: SourceBoundary, closing: SourceBoundary) -> EffortPhaseAnswer {
    let mut facts = Facts::new();
    facts.task();
    facts.lane();
    facts.session(occupant_a(), "sonnet", "high");
    facts.claim(occupant_a(), opening);
    facts.batch(
        source(),
        ORIGIN,
        pos(1, 100),
        vec![sample(1, 10, 10, 11)],
        0,
    );
    facts.done(closing);
    let mut answer = derive(&facts.snapshot()).expect("accounted fixture derives");
    assert_eq!(answer.phases.len(), 1, "the fixture holds one interval");
    answer.phases.remove(0)
}

fn incomplete(reason: &str) -> SourceBoundary {
    SourceBoundary::Incomplete {
        reason: reason.to_owned(),
    }
}

/// The single incomplete-boundary detail a withheld interval reports.
///
/// Asserting exactly one pins what the reader receives for one withheld bound,
/// and nothing more. It is deliberately not a guard against the bound being
/// recorded twice: `finish` sorts and dedups reasons by code and detail, and
/// both recorders spell the detail identically, so a doubled recording collapses
/// before any assertion can see it. That duplicate is observable only under
/// mutation, by severing one recorder and finding the interval still withheld,
/// which is why removing it was a precondition for pinning the bounds separately
/// rather than a change any assertion here could have caught.
fn boundary_detail(phase: &EffortPhaseAnswer) -> &str {
    let found = reasons(phase)
        .iter()
        .filter(|reason| reason.code == EffortUnavailableCode::IncompleteBoundary)
        .collect::<Vec<_>>();
    assert_eq!(
        found.len(),
        1,
        "one incomplete-boundary reason, found {found:?} in {:?}",
        phase.availability
    );
    &found[0].detail
}

fn assert_available(phase: &EffortPhaseAnswer, input_tokens: u64) {
    let EffortAvailability::Available { usage, .. } = &phase.availability else {
        panic!("expected available, found {:?}", phase.availability);
    };
    assert_eq!(usage.input_tokens, input_tokens);
}

fn assert_reason(phase: &EffortPhaseAnswer, code: EffortUnavailableCode) {
    assert!(
        reasons(phase).iter().any(|reason| reason.code == code),
        "missing {code:?} in {:?}",
        phase.availability
    );
}

fn reasons(phase: &EffortPhaseAnswer) -> &[EffortUnavailable] {
    match &phase.availability {
        EffortAvailability::Unavailable { reasons } => reasons,
        EffortAvailability::Available { .. } => &[],
    }
}

fn task() -> TaskHandle {
    "EFFORT-a1".parse().expect("fixture task")
}

fn phase(value: &str) -> Phase {
    value.parse().expect("fixture phase")
}

fn vendor() -> Name {
    "claude".parse().expect("fixture vendor")
}

fn lane() -> CanonicalUuid {
    uuid(LANE)
}

fn lineage_a() -> CanonicalUuid {
    uuid(LINEAGE_A)
}

fn lineage_b() -> CanonicalUuid {
    uuid(LINEAGE_B)
}

fn occupant_a() -> CanonicalUuid {
    uuid(OCCUPANT_A)
}

fn occupant_b() -> CanonicalUuid {
    uuid(OCCUPANT_B)
}

fn source() -> CanonicalUuid {
    uuid(SOURCE)
}

fn other_source() -> CanonicalUuid {
    uuid(OTHER_SOURCE)
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture UUID")
}

fn pos(member: u64, offset: u64) -> TranscriptPosition {
    TranscriptPosition { member, offset }
}

fn instant(second: u64) -> Instant {
    format!("2030-04-12T18:44:{:02}.000Z", second % 60)
        .parse()
        .expect("fixture instant")
}
