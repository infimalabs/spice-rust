use super::*;

// Executable samples covering every custom wire-crossing identity.

fn at(text: &str) -> Instant {
    text.parse().expect("instant")
}

/// A committed position covering exactly one sequence.
fn committed(position: u64) -> ReplyPosition {
    ReplyPosition::Committed(SeqRange {
        first: seq(position),
        last: seq(position),
    })
}

/// The refusal the reply frames reuse, carrying every optional field.
fn stale_witness() -> Refusal {
    Refusal {
        reason: "witness::stale".to_owned(),
        witness: Some("task".to_owned()),
        expected: Some("48190".to_owned()),
        observed: Some("48213".to_owned()),
        remedy: "re-read the task head and retry".to_owned(),
    }
}

/// The fault the reply frames reuse.
fn moved_epoch() -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: "epoch::moved".to_owned(),
        detail: "resident epoch moved".to_owned(),
    }
}

#[test]
fn every_crossing_type_obeys_the_identity_law() {
    // Coverage accumulates across the families so one assertion can speak for
    // the whole crate; splitting it per family would let a missing family pass.
    let mut covered = BTreeSet::new();

    cover_spellings(&mut covered);
    cover_config_scalars(&mut covered);
    cover_doctor_vocabularies(&mut covered);
    cover_journal_frames(&mut covered);
    cover_task_heads(&mut covered);
    cover_claim_heads(&mut covered);
    cover_policy_heads(&mut covered);
    cover_residency_heads(&mut covered);
    cover_head_enums(&mut covered);
    cover_command_frames(&mut covered);
    cover_stream_frames(&mut covered);
    cover_packed_frames(&mut covered);
    cover_refusal_frames(&mut covered);
    cover_reply_frames(&mut covered);
    cover_witness_values(&mut covered);
    cover_vocabularies(&mut covered);

    assert_coverage_is_complete(&covered);
}

/// Closed Doctor report states and metadata spellings.
fn cover_doctor_vocabularies(covered: &mut BTreeSet<String>) {
    spelling_law(covered, "SpiceVariable", SpiceVariable::ALL);
    spelling_law(covered, "DoctorToolMode", DoctorToolMode::ALL);
    ranked_law(covered, "DoctorToolRequirement", DoctorToolRequirement::ALL);
    ranked_law(covered, "DoctorToolSource", DoctorToolSource::ALL);
    ranked_law(covered, "DoctorToolBounds", DoctorToolBounds::ALL);
    spelling_law(
        covered,
        "DoctorToolCancellation",
        DoctorToolCancellation::ALL,
    );
    ranked_law(covered, "DoctorToolNetwork", DoctorToolNetwork::ALL);
    law(
        covered,
        "DoctorObservation",
        &[
            DoctorObservation::Ready {},
            DoctorObservation::Unavailable {
                reason: "rtk is absent".to_owned(),
            },
            DoctorObservation::InputRequired {
                reason: "the caller supplies CMD".to_owned(),
            },
            DoctorObservation::NotApplicable {
                reason: "the command declares no tool".to_owned(),
            },
            DoctorObservation::Unsupported {
                reason: "inspection would execute the tool".to_owned(),
            },
        ],
    );
}

/// Shared configuration words and the typed provenance surrounding them.
fn cover_config_scalars(covered: &mut BTreeSet<String>) {
    spelling_law(
        covered,
        "DurationWord",
        &[
            "0s".parse::<DurationWord>().expect("duration word"),
            "00s".parse::<DurationWord>().expect("duration word"),
            "15m".parse::<DurationWord>().expect("duration word"),
            format!("{}s", u64::MAX)
                .parse::<DurationWord>()
                .expect("maximum duration word"),
        ],
    );

    law(
        covered,
        "Provenance",
        &[
            Provenance::<DurationWord>::Builtin {},
            Provenance::Tracked { builtin: None },
            Provenance::Tracked {
                builtin: Some("5m".parse().expect("duration word")),
            },
            Provenance::Local { builtin: None },
            Provenance::Local {
                builtin: Some("10m".parse().expect("duration word")),
            },
            Provenance::Flag { builtin: None },
            Provenance::Flag {
                builtin: Some("15m".parse().expect("duration word")),
            },
        ],
    );
}

/// The spelling newtypes.
///
/// Instant's samples include sub-millisecond, pre-epoch, and boundary readings,
/// which is the narrowing path that broke this law before.
fn cover_spellings(covered: &mut BTreeSet<String>) {
    spelling_law(
        covered,
        "Instant",
        &[
            Instant::new("0000-01-01T00:00:00Z".parse().expect("minimum wire year"))
                .expect("minimum wire instant"),
            Instant::new("2030-04-12T18:44:09.271828Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new("2030-04-12T18:44:09.271Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new("1969-12-31T23:59:59.999999Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new(jiff::Timestamp::MAX).expect("maximum wire instant"),
        ],
    );
    spelling_law(
        covered,
        "CanonicalUuid",
        &[
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        ],
    );
    spelling_law(
        covered,
        "TaskHandle",
        &[handle("SPICE-aBcD"), handle("WIRE-1kJJP2Qc")],
    );
    spelling_law(covered, "Phase", &[phase("todo"), phase("review")]);
    spelling_law(
        covered,
        "Origin",
        &[
            "task:SPICE-aBcD".parse::<Origin>().expect("origin"),
            "repo:genesis".parse::<Origin>().expect("origin"),
        ],
    );
    spelling_law(
        covered,
        "FoldName",
        &[
            "board::ready".parse::<FoldName>().expect("fold name"),
            "bus::refusals".parse::<FoldName>().expect("fold name"),
        ],
    );
    spelling_law(
        covered,
        "Actor",
        &[
            Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c2")),
            Actor::Operator,
        ],
    );
    // A position spells decimal digits but ranks as a number, and the journal
    // reads it as a number: `9` before `10`, which is the reverse of how the
    // two spell. Samples span that step so the fixture pins the numeric order
    // rather than a lexical one that happens to agree on the samples chosen.
    ranked_law(covered, "Seq", &[seq(1), seq(9), seq(10), seq(48213)]);
    spelling_law(
        covered,
        "Name",
        &[
            "next".parse::<Name>().expect("name"),
            "reads".parse::<Name>().expect("name"),
        ],
    );
    spelling_law(covered, "Digest", &[digest("a"), digest("b")]);
}

/// The frames that rest in the journal.
///
/// These derive only Eq and inherit the law structurally, so their samples vary
/// one field at a time against a shared base.
fn cover_journal_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "Envelope",
        &[
            Envelope {
                v: WireVersion(1),
                seq: seq(48213),
                at: at("2030-04-12T18:44:09.271Z"),
                actor: Actor::Operator,
                origin: "task:SPICE-aBcD".parse().expect("origin"),
                cause: Some(seq(48190)),
                fact: Fact::new(body("checked")),
            },
            Envelope {
                v: WireVersion(1),
                seq: seq(48214),
                at: at("2030-04-12T18:44:09.272Z"),
                actor: Actor::Operator,
                origin: "task:SPICE-aBcD".parse().expect("origin"),
                cause: None,
                fact: Fact::new(body("cargo test --workspace")),
            },
        ],
    );
    law(
        covered,
        "Fact",
        &[
            Fact::new(body("checked")),
            Fact::new(body("cargo test --workspace")),
        ],
    );

    // The opaque read of the same fact. It obeys the identity law for the same
    // reason the typed one does — a verifier that re-encodes a row it read must
    // get the row back — and the samples are the typed bodies above so a drift
    // between the two readings would show up here as a byte difference.
    law(
        covered,
        "AnyFact",
        &[
            any_fact(Fact::new(body("checked"))),
            any_fact(Fact::new(body("cargo test --workspace"))),
        ],
    );
}

/// Read a typed fact back the way a journal verifier reads a stored row.
fn any_fact<F: wire::Body>(fact: Fact<F>) -> AnyFact {
    let bytes = wire::encode(&fact).expect("a typed fact encodes");
    wire::decode(&bytes).expect("a stored fact reads back without its body type")
}

/// Task statement and workflow heads.
fn cover_task_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let task = handle("SPICE-aBcD");

    law(
        covered,
        "TaskHead",
        &[
            TaskHead {
                handle: task.clone(),
                uuid: first_uuid,
                queue_origin: seq(9),
                title: "Build the gate".to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: "gate".to_owned(),
                tags: vec!["guard".to_owned()],
                dependencies: Vec::new(),
                provenance: "repo:genesis".parse().expect("origin"),
                revision: seq(10),
            },
            TaskHead {
                handle: task.clone(),
                uuid: second_uuid,
                queue_origin: seq(9),
                title: "Review the gate".to_owned(),
                description: Some("Confirm every edge".to_owned()),
                acceptance: vec!["The gate passes".to_owned()],
                project: "gate".to_owned(),
                tags: vec!["guard".to_owned(), "review".to_owned()],
                dependencies: vec![handle("WIRE-eFgH")],
                provenance: "task:SPICE-aBcD".parse().expect("origin"),
                revision: seq(11),
            },
        ],
    );
    law(
        covered,
        "WorkflowHead",
        &[
            WorkflowHead {
                flow: vec![
                    "todo".parse().expect("phase"),
                    "review".parse().expect("phase"),
                ],
                phase: "todo".parse().expect("phase"),
                status: "pending".to_owned(),
                review: "unassigned".to_owned(),
                assignment: Some(Actor::Operator),
                deferral: Some(at("2030-04-12T18:44:09.271Z")),
                completion: None,
            },
            WorkflowHead {
                flow: vec![
                    "todo".parse().expect("phase"),
                    "review".parse().expect("phase"),
                ],
                phase: "review".parse().expect("phase"),
                status: "completed".to_owned(),
                review: "clean".to_owned(),
                assignment: Some(Actor::Lane(first_uuid)),
                deferral: None,
                completion: Some(at("2030-04-12T18:44:09.272Z")),
            },
        ],
    );
}

/// Claim possession and lease heads.
fn cover_claim_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");

    law(
        covered,
        "ClaimHead",
        &[
            ClaimHead {
                actor: Actor::Operator,
                fact: seq(12),
                claimed: seq(12),
                cell: wire::ClaimCell {
                    project: "gate".to_owned(),
                    phase: "todo".parse().expect("phase"),
                },
                lane: first_uuid,
                occupant: second_uuid,
                epoch: first_uuid,
                disposition: ClaimDisposition::Active,
            },
            ClaimHead {
                actor: Actor::Lane(first_uuid),
                fact: seq(13),
                claimed: seq(12),
                cell: wire::ClaimCell {
                    project: "gate".to_owned(),
                    phase: "review".parse().expect("phase"),
                },
                lane: second_uuid,
                occupant: first_uuid,
                epoch: second_uuid,
                disposition: ClaimDisposition::Lapsed,
            },
        ],
    );
    law(
        covered,
        "LeaseHead",
        &[
            LeaseHead {
                start: at("2030-04-12T18:44:09.271Z"),
                renewal: at("2030-04-12T18:44:09.272Z"),
                maximum: at("2030-04-12T18:45:09.271Z"),
                expiry: at("2030-04-12T18:45:00.271Z"),
                epoch: first_uuid,
            },
            LeaseHead {
                start: at("2030-04-12T18:44:10.271Z"),
                renewal: at("2030-04-12T18:44:10.272Z"),
                maximum: at("2030-04-12T18:46:09.271Z"),
                expiry: at("2030-04-12T18:46:00.271Z"),
                epoch: second_uuid,
            },
        ],
    );
}

/// Witness, trust, and gate-finding heads.
fn cover_policy_heads(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "WitnessHead",
        &[
            WitnessHead {
                name: "review".to_owned(),
                kind: WitnessClass::Review,
                predicate: "reviewer differs from author".to_owned(),
                head: seq(14),
            },
            WitnessHead {
                name: "budget".to_owned(),
                kind: WitnessClass::Budget,
                predicate: "remaining equals 2".to_owned(),
                head: seq(15),
            },
        ],
    );
    law(
        covered,
        "TrustHead",
        &[
            TrustHead {
                digest: digest("a"),
                state: TrustState::Pended,
            },
            TrustHead {
                digest: digest("b"),
                state: TrustState::Granted,
            },
        ],
    );
    law(
        covered,
        "FindingHead",
        &[
            FindingHead {
                identity: "gate::one".to_owned(),
                severity: "high".to_owned(),
                locus: "wire/src/lib.rs".to_owned(),
                owner: "wire".to_owned(),
                receipt: digest("c"),
                state: FindingState::Open,
            },
            FindingHead {
                identity: "gate::two".to_owned(),
                severity: "low".to_owned(),
                locus: "wire/src/head.rs".to_owned(),
                owner: "gate".to_owned(),
                receipt: digest("d"),
                state: FindingState::Cleared,
            },
        ],
    );
}

/// Resident generation, lane, and cursor heads.
fn cover_residency_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");

    let occupied = LaneResidency {
        lane: first_uuid,
        lineage_id: first_uuid,
        name: Some("Ava".to_owned()),
        worktree: "/worktrees/lane-a".to_owned(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(second_uuid),
        lineage: vec![first_uuid],
        capabilities: vec![second_uuid],
        session: Some(LaneSession {
            lane: first_uuid,
            occupant: second_uuid,
            worktree: "/worktrees/lane-a".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
            epoch: first_uuid,
            stream: "stdout".to_owned(),
            identity: "018f7777-7777-7444-8888-999999999999".to_owned(),
            locus: TapeLocus {
                member: 7,
                offset: 31,
                length: 93,
            },
        }),
        disposition: "seated".to_owned(),
    };
    let empty = LaneResidency {
        lane: second_uuid,
        lineage_id: second_uuid,
        name: None,
        worktree: "/worktrees/lane-b".to_owned(),
        life: "steer".to_owned(),
        vendor: "codex".parse().expect("fixture vendor"),
        occupant: None,
        lineage: Vec::new(),
        capabilities: Vec::new(),
        session: None,
        disposition: "empty".to_owned(),
    };
    let recorded_session = occupied.session.clone().expect("occupied session");
    let mut unrecorded_session = recorded_session.clone();
    unrecorded_session.launch = None;
    law(
        covered,
        "LaneSession",
        &[recorded_session, unrecorded_session],
    );
    law(covered, "LaneResidency", &[occupied.clone(), empty.clone()]);
    let drive = LineageSubscription {
        lineage_id: first_uuid,
        policy: wire::intent::LineageLife::Drive,
        manual_projects: vec!["task.manual".to_owned()],
        auto_projects: vec!["task.learned".to_owned()],
    };
    let steer = LineageSubscription {
        lineage_id: second_uuid,
        policy: wire::intent::LineageLife::Steer,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
    };
    law(
        covered,
        "LineageSubscription",
        &[drive.clone(), steer.clone()],
    );
    law(
        covered,
        "ResidencyHead",
        &[
            ResidencyHead {
                epoch: first_uuid,
                state: ResidentState::Seated,
                lanes: vec![occupied],
                subscriptions: vec![drive],
            },
            ResidencyHead {
                epoch: second_uuid,
                state: ResidentState::Dark,
                lanes: vec![empty],
                subscriptions: vec![steer],
            },
        ],
    );
    law(
        covered,
        "CursorHead",
        &[
            CursorHead {
                seq: None,
                wake: None,
            },
            CursorHead {
                seq: Some(seq(16)),
                wake: Some(seq(15)),
            },
        ],
    );
}

/// Closed enums carried inside authoritative heads.
fn cover_head_enums(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "ClaimDisposition",
        &[ClaimDisposition::Active, ClaimDisposition::Freed],
    );
    law(
        covered,
        "FindingState",
        &[FindingState::Open, FindingState::Cleared],
    );
    law(
        covered,
        "WitnessClass",
        &[WitnessClass::Assignment, WitnessClass::Transition],
    );
}

/// The frames a caller sends across the socket.
///
/// A command is checked under the encoder its identity binds rather than the
/// generic one, so the pairwise arm reads canonical bytes. Its samples carry
/// several witnesses in more than one order, because a single-witness sample
/// cannot tell a sorted encoding from an unsorted one.
fn cover_command_frames(covered: &mut BTreeSet<String>) {
    law_under(
        covered,
        "Command",
        &[
            command("task::done", vec![task_witness()]),
            command("task::done", vec![task_witness(), workflow_witness()]),
            command("task::done", vec![workflow_witness(), task_witness()]),
            command("task::done", vec![task_witness(), task_witness()]),
            Command {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
                kind: "task::note".parse::<CommandKind>().expect("command kind"),
                expected: Vec::new(),
                body: body("checked"),
            },
        ],
        canonical_command_identity,
    );
    cover_witness_values(covered);
    law(
        covered,
        "Hello",
        &[
            Hello {
                wire: WireVersion(1),
                capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            },
            Hello {
                wire: WireVersion(1),
                capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            },
        ],
    );
    law(
        covered,
        "Accepted",
        &[
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Operator,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Spice,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c4")),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
        ],
    );
}

/// Every client request and resident frame in the closed canvas stream family.
///
/// Both enums carry serde narrowing: the request omits an absent reconnect
/// cursor and a fault omits an unavailable correlation id. Sampling every arm,
/// both optional states, and both packed row types proves those renderings are
/// the complete value rather than an identity-losing projection.
fn cover_stream_frames(covered: &mut BTreeSet<String>) {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let next_epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c4");
    let first_id = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    let second_id = uuid("018f82d4-6f59-7d06-92e3-bce9131163d2");
    let cursor = StreamCursor {
        epoch,
        seq: seq(48213),
    };
    law(
        covered,
        "AgentMessage",
        &[
            AgentMessage {
                lane: first_id,
                author: second_id,
                locus: MessageLocus {
                    member: 1,
                    offset: 2,
                    length: 3,
                    event: 4,
                },
                activity: AgentActivityKind::Prose,
                text: "settled".to_owned(),
            },
            AgentMessage {
                lane: second_id,
                author: first_id,
                locus: MessageLocus {
                    member: 5,
                    offset: 6,
                    length: 7,
                    event: 8,
                },
                activity: AgentActivityKind::Tool,
                text: "Bash".to_owned(),
            },
        ],
    );
    let requests = stream_request_samples(epoch, first_id, second_id, cursor);
    law(covered, "StreamRequest", &requests);

    let first_fact = opaque_stream_envelope(48213, None, "checked");
    let second_fact = opaque_stream_envelope(48214, Some(48213), "verified");
    let lane_fact = stream_lane_seated(48215, Some(48214));
    law(
        covered,
        "StreamFact",
        &[first_fact.clone(), second_fact.clone(), lane_fact],
    );
    let facts = Packed {
        rows: vec![first_fact.clone(), second_fact.clone()],
        overflow: Some(Overflow {
            omitted: 3,
            continuation: seq(48215),
        }),
        horizon: seq(48220),
    };
    let trust = Packed {
        rows: vec![TrustView {
            position: seq(48213),
            head: TrustHead {
                digest: digest("a"),
                state: TrustState::Granted,
            },
        }],
        overflow: None,
        horizon: seq(48220),
    };
    let mut frames = stream_data_samples(epoch, first_id, second_id, second_fact, facts, trust);
    frames.extend(stream_boundary_samples(
        epoch, next_epoch, first_id, second_id, cursor,
    ));
    law(covered, "StreamFrame", &frames);
}

fn stream_request_samples(
    epoch: CanonicalUuid,
    first_id: CanonicalUuid,
    second_id: CanonicalUuid,
    cursor: StreamCursor,
) -> [StreamRequest; 4] {
    [
        StreamRequest::Subscribe {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            subjects: vec![ViewSubject::Facts],
            after: None,
        },
        StreamRequest::Subscribe {
            v: WireVersion::CURRENT,
            id: second_id,
            epoch,
            subjects: vec![ViewSubject::Facts, ViewSubject::Trust],
            after: Some(cursor),
        },
        StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            subject: ViewSubject::Trust,
            cursor,
        },
        StreamRequest::Thread {
            v: WireVersion::CURRENT,
            id: second_id,
            epoch,
            cursor,
        },
    ]
}

fn stream_data_samples(
    epoch: CanonicalUuid,
    first_id: CanonicalUuid,
    second_id: CanonicalUuid,
    fact: StreamFact,
    facts: Packed<StreamFact>,
    trust: Packed<TrustView>,
) -> Vec<StreamFrame> {
    vec![
        StreamFrame::Facts {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            packed: facts.clone(),
        },
        StreamFrame::Trust {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            packed: trust,
        },
        StreamFrame::Thread {
            v: WireVersion::CURRENT,
            id: second_id,
            epoch,
            packed: facts,
        },
        StreamFrame::Fact {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            fact,
        },
    ]
}

fn stream_boundary_samples(
    epoch: CanonicalUuid,
    next_epoch: CanonicalUuid,
    first_id: CanonicalUuid,
    second_id: CanonicalUuid,
    cursor: StreamCursor,
) -> Vec<StreamFrame> {
    vec![
        StreamFrame::Refusal {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            refusal: stale_witness(),
        },
        StreamFrame::Fault {
            v: WireVersion::CURRENT,
            epoch,
            id: None,
            fault: moved_epoch(),
        },
        StreamFrame::Fault {
            v: WireVersion::CURRENT,
            epoch,
            id: Some(second_id),
            fault: Fault {
                class: FaultClass::Store,
                reason: "stream::snapshot-failed".to_owned(),
                detail: "authority snapshot failed".to_owned(),
            },
        },
        StreamFrame::Overflow {
            v: WireVersion::CURRENT,
            id: first_id,
            epoch,
            after: cursor,
            dropped: 7,
        },
        StreamFrame::Closing {
            v: WireVersion::CURRENT,
            id: second_id,
            epoch: next_epoch,
            after: StreamCursor {
                epoch: next_epoch,
                seq: seq(48220),
            },
        },
    ]
}

fn opaque_stream_envelope(position: u64, cause: Option<u64>, validation: &str) -> StreamFact {
    let typed = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(position),
        at: at("2030-04-12T18:44:09.271Z"),
        actor: Actor::Operator,
        origin: "task:SPICE-aBcD".parse().expect("origin"),
        cause: cause.map(seq),
        fact: Fact::new(body(validation)),
    };
    let opaque: Envelope<AnyFact> =
        wire::decode(&wire::encode(&typed).expect("typed envelope encodes"))
            .expect("opaque envelope decodes");
    StreamFact::from(opaque)
}

fn stream_lane_seated(position: u64, cause: Option<u64>) -> StreamFact {
    let typed = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(position),
        at: at("2030-04-12T18:44:09.273Z"),
        actor: Actor::Spice,
        origin: "resident:018f82d4-6f59-7d06-92e3-bce9131163c3"
            .parse()
            .expect("origin"),
        cause: cause.map(seq),
        fact: Fact::new(body::LaneSeated {
            lane: uuid("018f82d4-6f59-7d06-92e3-bce9131163d8"),
            lineage_id: uuid("018f82d4-6f59-7d06-92e3-bce9131163d8"),
            capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163d9"),
            name: Some("Ava".to_owned()),
            worktree: "/tmp/lane".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("vendor"),
        }),
    };
    let opaque: Envelope<AnyFact> =
        wire::decode(&wire::encode(&typed).expect("lane envelope encodes"))
            .expect("lane envelope decodes");
    StreamFact::from(opaque)
}

/// Concrete command predicates and their nested state vocabularies.
///
/// Every type here derives `Ord` and `Hash` over a declared sequence rather
/// than over its spelling, so each states that sequence in full and the
/// fixture pins it. Reordering a variant is then a failing test naming the
/// pair that moved, not a silent change to how a rendered column sorts.
fn cover_witness_values(covered: &mut BTreeSet<String>) {
    ranked_law(
        covered,
        "Witness",
        &[
            Witness::Task {
                task: handle("SPICE-aBcD"),
                head: seq(48190),
            },
            Witness::Workflow {
                task: handle("SPICE-aBcD"),
                head: seq(48190),
                phase: "todo".parse().expect("phase"),
            },
            Witness::Directive {
                steer: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                disposition: Disposition::Live,
            },
            Witness::Consent {
                request: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                answer: Answer::Asked,
            },
            Witness::Trust {
                digest: digest("c"),
                standing: TrustState::Pended,
            },
            Witness::Transition {
                from: ResidentState::Seated,
                to: ResidentState::Quiescing,
            },
        ],
    );
}

/// The paged frames, sampled with and without an overflow tail.
fn cover_packed_frames(covered: &mut BTreeSet<String>) {
    let rows = vec![Row {
        task: handle("SPICE-aBcD"),
    }];
    let plain = Packed {
        rows: rows.clone(),
        overflow: None,
        horizon: seq(48213),
    };
    let overflowing = Packed {
        rows,
        overflow: Some(Overflow {
            omitted: 7,
            continuation: seq(48220),
        }),
        horizon: seq(48213),
    };

    law(covered, "Packed", &[plain.clone(), overflowing.clone()]);
    law(
        covered,
        "Derived",
        &[
            Derived {
                fold: "board::ready".parse().expect("fold name"),
                r#gen: 12,
                cursor: seq(48213),
                packed: plain,
            },
            Derived {
                fold: "board::ready".parse().expect("fold name"),
                r#gen: 13,
                cursor: seq(48214),
                packed: overflowing,
            },
        ],
    );
}

/// The frames that carry a refusal or a fault back to a caller.
fn cover_refusal_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "SeqRange",
        &[
            SeqRange {
                first: seq(1),
                last: seq(2),
            },
            SeqRange {
                first: seq(1),
                last: seq(3),
            },
        ],
    );
    law(
        covered,
        "Refusal",
        &[
            Refusal {
                reason: "wire::mismatch".to_owned(),
                witness: None,
                expected: None,
                observed: None,
                remedy: "reconnect with the resident's current wire".to_owned(),
            },
            stale_witness(),
        ],
    );
    law(
        covered,
        "Fault",
        &[
            moved_epoch(),
            Fault {
                class: FaultClass::Store,
                reason: "journal::unreadable".to_owned(),
                detail: "journal read failed".to_owned(),
            },
        ],
    );
}

/// The frames a caller reads back after a command lands.
fn cover_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "DemoAnswer",
        &[
            DemoAnswer {
                root: "/tmp/spice-demo-one".to_owned(),
                transcript: "/tmp/spice-demo-one/spice-demo-transcript.jsonl".to_owned(),
                mode: "seed".to_owned(),
                lifecycle: "preserved".to_owned(),
                endpoint: None,
                launch_command: "spice demo --reuse /tmp/spice-demo-one".to_owned(),
                remove_command: "rm -rf -- /tmp/spice-demo-one".to_owned(),
            },
            DemoAnswer {
                root: "/tmp/spice-demo-two".to_owned(),
                transcript: "/tmp/spice-demo-two/spice-demo-transcript.jsonl".to_owned(),
                mode: "canvas".to_owned(),
                lifecycle: "removed".to_owned(),
                endpoint: Some("http://127.0.0.1:48213".to_owned()),
                launch_command: "spice demo --reuse /tmp/spice-demo-two".to_owned(),
                remove_command: "rm -rf -- /tmp/spice-demo-two".to_owned(),
            },
        ],
    );
    cover_journal_reply_frames(covered);
    cover_restore_reply_frames(covered);
    cover_fold_reply_frames(covered);
    cover_transport_reply_frames(covered);
}

fn cover_journal_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "JournalReport",
        &[
            JournalReport {
                schema: 1,
                cursor: None,
                facts: 0,
                entries: Packed {
                    rows: Vec::new(),
                    overflow: None,
                    horizon: seq(1),
                },
            },
            JournalReport {
                schema: 1,
                cursor: Some(seq(19)),
                facts: 19,
                entries: Packed {
                    rows: Vec::new(),
                    overflow: Some(Overflow {
                        omitted: 19,
                        continuation: seq(1),
                    }),
                    horizon: seq(19),
                },
            },
        ],
    );
}

fn cover_restore_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "JournalRestore",
        &[
            JournalRestore {
                plan: digest("1"),
                destination: "/repo/.git/.spice-rs/data/spicejournal.sqlite3".to_owned(),
                previous: None,
                verification: restore_verification("2", 0),
                applied: false,
            },
            JournalRestore {
                plan: digest("3"),
                destination: "/repo/.git/.spice-rs/data/spicejournal.sqlite3".to_owned(),
                previous: Some(digest("4")),
                verification: restore_verification("5", 19),
                applied: true,
            },
        ],
    );
}

fn cover_fold_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "FoldState",
        &[
            FoldState {
                fold: "canvas::flow".to_owned(),
                generation: None,
                reducer_version: None,
                cursor: None,
                horizon: None,
                recovery: "spice journal replay canvas::flow".to_owned(),
                sources: vec!["task::*".to_owned(), "task".to_owned()],
                payload: None,
            },
            FoldState {
                fold: "release::history".to_owned(),
                generation: Some(2),
                reducer_version: Some(1),
                cursor: Some(seq(19)),
                horizon: Some(seq(19)),
                recovery: "spice journal replay release::history".to_owned(),
                sources: vec!["proof::*".to_owned(), "release::*".to_owned()],
                payload: Some(serde_json::json!({"facts": []})),
            },
        ],
    );
}

fn cover_transport_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "ProofEvidence",
        &[
            ProofEvidence {
                target: ReleaseTarget::Aarch64AppleDarwin,
                payload: digest("1"),
                receipt: digest("2"),
                battery: digest("3"),
                replay: digest("4"),
            },
            ProofEvidence {
                target: ReleaseTarget::X86_64UnknownLinuxMusl,
                payload: digest("5"),
                receipt: digest("6"),
                battery: digest("7"),
                replay: digest("8"),
            },
        ],
    );
    law(
        covered,
        "ReplyPosition",
        &[
            committed(48213),
            ReplyPosition::Snapshot(seq(48213)),
            ReplyPosition::Direct {},
        ],
    );
    law(
        covered,
        "Reply",
        &[
            Reply {
                position: committed(48213),
                body: body("checked"),
            },
            Reply {
                position: ReplyPosition::Snapshot(seq(48213)),
                body: body("checked"),
            },
            Reply {
                position: ReplyPosition::Direct {},
                body: body("checked"),
            },
        ],
    );
    law(
        covered,
        "Outcome",
        &[
            Outcome::Reply(Reply {
                position: committed(48213),
                body: body("checked"),
            }),
            Outcome::Reply(Reply {
                position: ReplyPosition::Direct {},
                body: body("checked directly"),
            }),
            Outcome::Refusal(stale_witness()),
        ],
    );
    law(
        covered,
        "Response",
        &[
            Response {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                outcome: Outcome::Reply(Reply {
                    position: committed(48213),
                    body: body("checked"),
                }),
            },
            Response {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                outcome: Outcome::Reply(Reply {
                    position: ReplyPosition::Direct {},
                    body: body("checked directly"),
                }),
            },
            Response {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                outcome: Outcome::Fault(moved_epoch()),
            },
        ],
    );
}

fn restore_verification(nibble: &str, facts: u64) -> RestoreVerification {
    RestoreVerification {
        bundle: digest(nibble),
        wire: WireVersion::CURRENT,
        journal_schema: 1,
        authority: digest(nibble),
        members: 1,
        facts,
        receipts: 0,
        heads: 0,
        threads: facts,
    }
}
