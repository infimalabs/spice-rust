//! Lane answers, recall provenance, and typed reply construction.

use super::*;

pub(super) fn command_result<B: serde::Serialize>(
    write: &mut JournalWrite,
    admitted: &wire::AdmittedCommand,
    body: B,
    epoch: CanonicalUuid,
    answer: LaneChangeAnswer,
) -> CommandResult<()> {
    write
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: admitted.id(),
                epoch: admitted.epoch(),
                kind: admitted.kind(),
                expected: admitted.expected().to_vec(),
                body,
            },
            epoch,
            answer,
        )
        .map_err(|error| store_fault("lane::receipt-invalid", &error.to_string()))
}

const QUIET_HORIZON: Duration = Duration::from_secs(60);

pub(super) fn answer(
    head: &wire::ResidencyHead,
    selected: Option<CanonicalUuid>,
    seats: &Seats,
) -> CommandResult<LaneAnswer> {
    answer_at(head, selected, Monotonic::now(), |lane| {
        seats.activity(lane)
    })
}

fn answer_at(
    head: &wire::ResidencyHead,
    selected: Option<CanonicalUuid>,
    now: Monotonic,
    activity: impl Fn(CanonicalUuid) -> Option<lane::occupant::Activity>,
) -> CommandResult<LaneAnswer> {
    let lanes = head
        .lanes
        .iter()
        .filter(|row| selected.is_none_or(|lane| row.lane == lane))
        .map(|row| {
            let liveness = liveness(row, activity(row.lane), now)?;
            Ok(wire::intent::LaneAnswerRow {
                lane: row.lane,
                lineage_id: row.lineage_id,
                name: row.name.clone(),
                worktree: row.worktree.clone(),
                life: row.life.clone(),
                vendor: row.vendor.clone(),
                model: row
                    .session
                    .as_ref()
                    .and_then(|session| session.launch.as_ref())
                    .map(|launch| launch.model.clone()),
                effort: row
                    .session
                    .as_ref()
                    .and_then(|session| session.launch.as_ref())
                    .map(|launch| launch.effort.clone()),
                git: row
                    .session
                    .as_ref()
                    .and_then(|session| session.launch.as_ref())
                    .and_then(|launch| launch.git.clone()),
                occupant: row.occupant,
                lineage: row.lineage.clone(),
                session: row.session.as_ref().map(|session| session.identity.clone()),
                bound: row.session.is_some(),
                disposition: row.disposition.clone(),
                liveness,
            })
        })
        .collect::<CommandResult<Vec<_>>>()?;
    let lineage_ids = lanes
        .iter()
        .map(|row| row.lineage_id)
        .collect::<std::collections::BTreeSet<_>>();
    let subscriptions = lineage_ids
        .into_iter()
        .map(|lineage_id| {
            subscription_row(head, lineage_id)?.cloned().ok_or_else(|| {
                store_fault(
                    "lineage::subscription-missing",
                    "a returned lineage has no task-subscription policy",
                )
            })
        })
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(LaneAnswer {
        epoch: head.epoch,
        state: head.state,
        topology_revision: head.topology_revision,
        lanes,
        subscriptions,
    })
}

/// Admit one client-observed topology authority and derive its successor.
///
/// The caller still guards the exact residency head before commit. Together,
/// this comparison and that guard make observing and advancing the logical
/// revision one optimistic transaction rather than a per-command race check.
pub(super) fn next_topology_revision(
    head: &wire::ResidencyHead,
    observed: u64,
) -> Result<u64, Outcome<CommandAnswer>> {
    if observed != head.topology_revision {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::topology-moved",
                Some("topology_revision".to_owned()),
                Some(observed.to_string()),
                Some(head.topology_revision.to_string()),
            )
            .with_detail(
                "read the authoritative lane table and retry against its topology revision",
            ),
        )));
    }
    head.topology_revision.checked_add(1).ok_or_else(|| {
        typed_store_fault(
            "lane::topology-revision-exhausted",
            "the durable topology revision cannot advance",
        )
    })
}

fn liveness(
    row: &wire::LaneResidency,
    activity: Option<lane::occupant::Activity>,
    now: Monotonic,
) -> CommandResult<wire::intent::LaneLiveness> {
    let Some(occupant) = row.occupant else {
        if activity.is_some() {
            return Err(store_fault(
                "lane::liveness-occupied",
                "a durable vacant lane still has supervised activity",
            ));
        }
        return Ok(wire::intent::LaneLiveness::Vacant);
    };
    if crate::resident::is_simulated_lane(row) {
        if activity.is_some() {
            return Err(store_fault(
                "lane::simulated-supervised",
                "a simulated lane has real supervised activity",
            ));
        }
        if row.session.is_some() {
            return Err(store_fault(
                "lane::simulated-session-present",
                "a simulated lane carries real vendor session authority",
            ));
        }
        return Ok(wire::intent::LaneLiveness::Running);
    }
    let observed = activity.ok_or_else(|| {
        store_fault(
            "lane::liveness-unobserved",
            "a durable occupied lane has no supervised activity observation",
        )
    })?;
    if observed.occupant() != occupant {
        return Err(store_fault(
            "lane::liveness-occupant-moved",
            "the supervised activity occupant differs from durable residency",
        ));
    }
    let quiet = now.checked_duration_since(observed.at()).ok_or_else(|| {
        store_fault(
            "lane::liveness-clock-regressed",
            "the activity observation lies after the read clock",
        )
    })?;
    Ok(if quiet < QUIET_HORIZON {
        wire::intent::LaneLiveness::Running
    } else {
        wire::intent::LaneLiveness::RunningStale
    })
}

pub(super) fn relevant(fact: &wire::Envelope<wire::AnyFact>, lane: CanonicalUuid) -> bool {
    fact.actor == Actor::Lane(lane) || fact_lane(fact) == Some(lane)
}

pub(super) fn fact_lane(fact: &wire::Envelope<wire::AnyFact>) -> Option<CanonicalUuid> {
    fact.fact
        .body()
        .get("lane")
        .and_then(serde_json::Value::as_str)
        .and_then(|lane| lane.parse().ok())
        .or(match fact.actor {
            Actor::Lane(lane) => Some(lane),
            Actor::Operator | Actor::Spice => None,
        })
}

pub(super) fn recall_provenance(
    fact: &wire::Envelope<wire::AnyFact>,
    steering: &steer::Ledger,
) -> CommandResult<Option<RecallProvenance>> {
    if let Some(provenance) = recall_live_steering_provenance(fact)? {
        return Ok(Some(provenance));
    }
    if let Some(provenance) = recall_terminal_steering_provenance(fact, steering)? {
        return Ok(Some(provenance));
    }
    let provenance = match fact.fact.kind() {
        wire::FactKind::LaneSessionDeclared => {
            let body: wire::body::LaneSessionDeclared = recall_body(fact)?;
            Some(RecallProvenance::Session {
                occupant: body.occupant,
                worktree: body.worktree,
                vendor: body.vendor,
                epoch: body.epoch,
                stream: body.stream,
                identity: body.identity,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::LaneCompacted => {
            let body: wire::body::LaneCompacted = recall_body(fact)?;
            Some(RecallProvenance::Compaction {
                occupant: body.occupant,
                stream: body.stream,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::LaneRenewed => {
            let body: wire::body::LaneRenewed = recall_body(fact)?;
            Some(RecallProvenance::Renewal {
                occupant: body.occupant,
                ancestor: body.ancestor,
                epoch: body.epoch,
            })
        }
        _ => None,
    };
    Ok(provenance)
}

fn recall_live_steering_provenance(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<Option<RecallProvenance>> {
    let provenance = match fact.fact.kind() {
        wire::FactKind::SteerSent => {
            let body: wire::body::SteerSent = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                Some(wire::body::SteeringStage::Published),
                Vec::new(),
                None,
                None,
            ))
        }
        wire::FactKind::SteerEscalated => {
            let body: wire::body::SteerEscalated = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                Some(body.stage),
                Vec::new(),
                None,
                None,
            ))
        }
        // A reminder opens a lineage, so history reads it as the send it is.
        //
        // Its transcript locus is deliberately dropped here: this row's locus
        // means where a lane answered, and a reminder's locus means where the
        // prose that fired the taste sits. Carrying it across would read as an
        // acknowledgment nobody made.
        wire::FactKind::TasteReminded => {
            let body: wire::body::TasteReminded = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                Some(wire::body::SteeringStage::Published),
                Vec::new(),
                None,
                None,
            ))
        }
        _ => None,
    };
    Ok(provenance)
}

fn recall_terminal_steering_provenance(
    fact: &wire::Envelope<wire::AnyFact>,
    steering: &steer::Ledger,
) -> CommandResult<Option<RecallProvenance>> {
    let provenance = match fact.fact.kind() {
        wire::FactKind::SteerAcked => {
            let body: wire::body::SteerAcked = recall_body(fact)?;
            Some(stable_terminal_provenance(
                steering,
                &body.key,
                wire::Disposition::Acked,
                Some(body.epoch),
                Some(body.locus),
            )?)
        }
        wire::FactKind::SteerDeclined => {
            let body: wire::body::SteerDeclined = recall_body(fact)?;
            Some(stable_terminal_provenance(
                steering,
                &body.key,
                wire::Disposition::Declined,
                Some(body.epoch),
                Some(body.locus),
            )?)
        }
        wire::FactKind::SteerLapsed => {
            let body: wire::body::SteerLapsed = recall_body(fact)?;
            Some(stable_terminal_provenance(
                steering,
                &body.key,
                wire::Disposition::Lapsed,
                None,
                None,
            )?)
        }
        wire::FactKind::SteerRecovered => {
            let body: wire::body::SteerRecovered = recall_body(fact)?;
            Some(stable_terminal_provenance(
                steering,
                &body.key,
                wire::Disposition::Live,
                None,
                None,
            )?)
        }
        wire::FactKind::SteerCanceled => {
            let body: wire::body::SteerCanceled = recall_body(fact)?;
            Some(stable_terminal_provenance(
                steering,
                &body.key,
                wire::Disposition::Canceled,
                None,
                None,
            )?)
        }
        _ => None,
    };
    Ok(provenance)
}

/// Name one steering fact as the provenance a recall answer carries.
fn steering_provenance(
    directive: Option<CanonicalUuid>,
    key: String,
    disposition: wire::Disposition,
    stage: Option<wire::body::SteeringStage>,
    escalations: Vec<wire::body::SteeringTransition>,
    epoch: Option<CanonicalUuid>,
    locus: Option<wire::body::TranscriptLocus>,
) -> RecallProvenance {
    RecallProvenance::Steering {
        directive,
        key,
        disposition,
        stage,
        escalations,
        epoch,
        locus,
    }
}

fn stable_terminal_provenance(
    ledger: &steer::Ledger,
    key: &str,
    disposition: wire::Disposition,
    epoch: Option<CanonicalUuid>,
    locus: Option<wire::body::TranscriptLocus>,
) -> CommandResult<RecallProvenance> {
    let key = key.parse::<steer::Key>().map_err(|error| {
        store_fault(
            "recall::steering-key-invalid",
            &format!("terminal steering key is invalid: {error}"),
        )
    })?;
    let directive = ledger
        .directives()
        .into_iter()
        .find(|directive| directive.key() == &key)
        .ok_or_else(|| {
            store_fault(
                "recall::steering-root-missing",
                &format!("terminal steering key {key} has no stable directive"),
            )
        })?;
    Ok(steering_provenance(
        Some(directive.directive()),
        directive.key().to_string(),
        disposition,
        Some(directive.stage()),
        directive.escalations().to_vec(),
        epoch,
        locus,
    ))
}

/// Read one recalled fact body back out of its admitted envelope.
fn recall_body<T: serde::de::DeserializeOwned>(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<T> {
    let bytes = wire::encode(fact.fact.body())
        .map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))?;
    wire::decode(&bytes).map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))
}

#[cfg(test)]
#[path = "reply_tests.rs"]
mod terminal_tests;

pub(super) fn committed_lane_reply(
    committed: bus::JournalCommit,
    answer: LaneChangeAnswer,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneChange(Box::new(answer)),
    })
}

pub(super) fn authority_refusal(actor: &Actor, expected: &str) -> Outcome<CommandAnswer> {
    lane_refusal(
        "command::authority",
        Some(expected.to_owned()),
        Some(actor.to_string()),
        "reconnect with a capability admitted for this command family",
    )
}

pub(super) fn missing_lane(lane: CanonicalUuid) -> Outcome<CommandAnswer> {
    lane_refusal(
        "lane::missing",
        Some("one current lane".to_owned()),
        Some(lane.to_string()),
        "read lane and retry with a current lane identity",
    )
}

pub(super) fn lane_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("lane".to_owned()),
            expected,
            observed,
        )
        .with_detail(detail),
    ))
}

pub(super) fn typed_environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture uuid")
    }

    fn occupied() -> wire::LaneResidency {
        wire::LaneResidency {
            lane: uuid(1),
            lineage_id: uuid(1),
            name: Some("Ava".to_owned()),
            worktree: "/worktrees/main-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid(2)),
            lineage: vec![uuid(2)],
            capabilities: vec![uuid(3)],
            session: Some(wire::LaneSession {
                lane: uuid(1),
                occupant: uuid(2),
                worktree: "/worktrees/main-a".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                launch: Some(
                    wire::body::LaneLaunchIdentity::new("sonnet", "high").with_git("current"),
                ),
                epoch: uuid(4),
                stream: "stdout".to_owned(),
                identity: "private-session".to_owned(),
                locus: wire::TapeLocus {
                    member: 1,
                    offset: 2,
                    length: 3,
                },
            }),
            disposition: "occupied".to_owned(),
        }
    }

    fn simulated() -> wire::LaneResidency {
        let mut row = occupied();
        row.vendor = crate::resident::SIMULATED_VENDOR
            .parse()
            .expect("simulated fixture vendor");
        row.session = None;
        row.disposition = crate::resident::SIMULATED_DISPOSITION.to_owned();
        row
    }

    fn subscriptions() -> Vec<wire::LineageSubscription> {
        vec![wire::LineageSubscription {
            lineage_id: uuid(1),
            policy: wire::intent::LineageLife::Drive,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }]
    }

    #[test]
    fn the_same_occupied_lane_turns_quiet_at_the_exact_sixty_second_boundary() {
        let row = occupied();
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![row.clone()],
            subscriptions: subscriptions(),
        };
        let unchanged = head.clone();
        let observed = Monotonic::now();
        let activity = lane::occupant::Activity::new(uuid(2), observed);

        let recent = answer_at(
            &head,
            None,
            observed + Duration::from_millis(59_999),
            |_| Some(activity),
        )
        .expect("recent activity projects");
        let quiet = answer_at(&head, None, observed + Duration::from_secs(60), |_| {
            Some(activity)
        })
        .expect("boundary activity projects");

        assert_eq!(
            recent.lanes[0].liveness,
            wire::intent::LaneLiveness::Running
        );
        assert_eq!(
            quiet.lanes[0].liveness,
            wire::intent::LaneLiveness::RunningStale
        );
        assert_eq!(recent.lanes[0].name.as_deref(), Some("Ava"));
        assert_eq!(recent.lanes[0].life, "drive");
        assert_eq!(recent.lanes[0].disposition, "occupied");
        assert_eq!(
            head, unchanged,
            "clock fold mutated durable lifecycle state"
        );
    }

    #[test]
    fn lane_reads_expose_display_identity_without_capabilities() {
        let row = occupied();
        let capability = row.capabilities[0].to_string();
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![row],
            subscriptions: subscriptions(),
        };
        let observed = Monotonic::now();
        let answer = answer_at(&head, None, observed, |_| {
            Some(lane::occupant::Activity::new(uuid(2), observed))
        })
        .expect("supervised lane projects");
        let encoded = String::from_utf8(wire::encode(&answer).expect("answer encodes"))
            .expect("wire encoding is UTF-8");

        assert!(answer.lanes[0].bound);
        assert_eq!(answer.topology_revision, 7);
        assert_eq!(answer.lanes[0].model.as_deref(), Some("sonnet"));
        assert_eq!(answer.lanes[0].effort.as_deref(), Some("high"));
        assert_eq!(answer.lanes[0].git.as_deref(), Some("current"));
        assert_eq!(answer.lanes[0].session.as_deref(), Some("private-session"));
        assert!(!encoded.contains(&capability));
        assert!(!encoded.contains("capabilities"));
        assert!(encoded.contains("private-session"));
    }

    #[test]
    fn lane_reads_expose_one_unrecorded_launch_state_without_inventing_either_half() {
        let mut row = occupied();
        row.session.as_mut().expect("fixture session").launch = None;
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![row],
            subscriptions: subscriptions(),
        };
        let observed = Monotonic::now();

        let answer = answer_at(&head, None, observed, |_| {
            Some(lane::occupant::Activity::new(uuid(2), observed))
        })
        .expect("unrecorded launch projects");

        assert_eq!(answer.lanes[0].model, None);
        assert_eq!(answer.lanes[0].effort, None);
        assert_eq!(answer.lanes[0].git, None);
        assert_eq!(answer.lanes[0].session.as_deref(), Some("private-session"));
    }

    #[test]
    fn a_durable_occupant_without_the_same_supervised_observation_faults() {
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![occupied()],
            subscriptions: subscriptions(),
        };
        let error = answer_at(&head, None, Monotonic::now(), |_| None)
            .expect_err("unobserved occupancy must fail loudly");
        let Outcome::Fault(fault) = error else {
            panic!("unobserved occupancy must be a fault")
        };
        assert_eq!(fault.reason, "lane::liveness-unobserved");
    }

    #[test]
    fn a_simulated_occupant_is_running_without_a_supervised_process() {
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![simulated()],
            subscriptions: subscriptions(),
        };

        let answer = answer_at(&head, None, Monotonic::now(), |_| None)
            .expect("simulated occupancy projects without a process");

        assert_eq!(
            answer.lanes[0].liveness,
            wire::intent::LaneLiveness::Running
        );
        assert!(!answer.lanes[0].bound);
        assert_eq!(
            answer.lanes[0].vendor.as_str(),
            crate::resident::SIMULATED_VENDOR
        );
        assert_eq!(
            answer.lanes[0].disposition,
            crate::resident::SIMULATED_DISPOSITION
        );
    }

    #[test]
    fn a_simulated_occupant_with_real_supervision_faults() {
        let row = simulated();
        let occupant = row.occupant.expect("simulated fixture occupant");
        let head = wire::ResidencyHead {
            epoch: uuid(4),
            state: ResidentState::Seated,
            topology_revision: 7,
            lanes: vec![row],
            subscriptions: subscriptions(),
        };
        let error = answer_at(&head, None, Monotonic::now(), |_| {
            Some(lane::occupant::Activity::new(occupant, Monotonic::now()))
        })
        .expect_err("simulated occupancy must never acquire real supervision");
        let Outcome::Fault(fault) = error else {
            panic!("simulated supervision must be a fault")
        };
        assert_eq!(fault.reason, "lane::simulated-supervised");
    }
}
