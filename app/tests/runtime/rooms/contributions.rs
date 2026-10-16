//! Typed Design contributions: grouped by kind and interleaved with ordinary
//! conversation in journal order, crediting their lanes, with each cited
//! source resolved once per shown Design by the resident that owns it.

use super::*;
use app::{ContributionSources, RoomEventKind};
use wire::intent::RoomContributionSourcesAnswer;
use wire::rooms::{
    DesignNoticed, RoomContributed, RoomContributionKind, RoomSource, RoomSourceUnavailable,
    RoomSourceView,
};

fn journal(fact: u64) -> RoomSource {
    RoomSource::Journal {
        fact: wire::SequenceCode::new(fact),
    }
}

fn artifact() -> RoomSource {
    RoomSource::Artifact {
        digest: "ab".repeat(32).parse().expect("fixture digest"),
    }
}

fn contributed(
    position: u64,
    publication: &RoomAuthorityStamp,
    lane: &str,
    contribution: RoomContributionKind,
    sources: Vec<RoomSource>,
) -> serde_json::Value {
    envelope(
        position,
        "room::contributed",
        &RoomContributed {
            room: room(),
            publication: publication.clone(),
            lane: uuid(lane),
            contribution,
            statement: None,
            text: format!("{} at {position}", contribution.as_str()),
            sources,
        },
    )
}
fn attributed(
    position: u64,
    statement: u64,
    publication: &RoomAuthorityStamp,
    lane: &str,
    contribution: RoomContributionKind,
) -> serde_json::Value {
    envelope(
        position,
        "room::contributed",
        &RoomContributed {
            room: room(),
            publication: publication.clone(),
            lane: uuid(lane),
            contribution,
            statement: wire::Seq::new(statement),
            text: format!("{} at {position}", contribution.as_str()),
            sources: Vec::new(),
        },
    )
}

fn contribution_runtime(session: &Session, snapshot: &[serde_json::Value]) -> AppRuntime {
    let accepted = acceptance(session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let horizon = snapshot.len() as u64;
    runtime.apply_stream(&operator_facts(session, horizon));
    runtime.apply_stream(&room_page(session, snapshot, None, horizon));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    runtime
}

/// Publication identities deliberately ordered against the journal, so only
/// journal position can produce the asserted order.
fn stamps() -> [RoomAuthorityStamp; 5] {
    [50, 40, 30, 20, 10].map(later_publication)
}

#[test]
fn contributions_group_by_kind_and_interleave_with_conversation_in_journal_order() {
    let session = session(OPERATOR, EPOCH);
    let [first, second, third, fourth, fifth] = stamps();
    let snapshot = [
        contributed(
            1,
            &first,
            LANE,
            RoomContributionKind::Evidence,
            vec![journal(1)],
        ),
        room_message(2),
        contributed(
            3,
            &second,
            SECOND_LANE,
            RoomContributionKind::Question,
            Vec::new(),
        ),
        contributed(
            4,
            &third,
            LANE,
            RoomContributionKind::ReasonAgainst,
            vec![artifact()],
        ),
        contributed(
            5,
            &fourth,
            SECOND_LANE,
            RoomContributionKind::ReasonFor,
            Vec::new(),
        ),
        contributed(
            6,
            &fifth,
            SECOND_LANE,
            RoomContributionKind::Evidence,
            Vec::new(),
        ),
    ];
    let runtime = contribution_runtime(&session, &snapshot);
    let projection = runtime.room_projection();

    let groups = projection
        .contribution_groups(&room())
        .into_iter()
        .map(|group| {
            (
                group.kind,
                group
                    .entries
                    .iter()
                    .map(|entry| (entry.position.get(), entry.lane))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        groups,
        vec![
            (
                RoomContributionKind::ReasonFor,
                vec![(5, uuid(SECOND_LANE))]
            ),
            (RoomContributionKind::ReasonAgainst, vec![(4, uuid(LANE))]),
            (
                RoomContributionKind::Evidence,
                vec![(1, uuid(LANE)), (6, uuid(SECOND_LANE))]
            ),
            (RoomContributionKind::Question, vec![(3, uuid(SECOND_LANE))]),
        ],
        "every kind groups in canonical order, each oldest first by journal position"
    );

    let chronology = projection
        .room(&room())
        .expect("fixture room")
        .in_journal_order()
        .into_iter()
        .map(|(_, event)| match &event.kind {
            RoomEventKind::Contributed { contribution, .. } => contribution.as_str(),
            RoomEventKind::Messaged { .. } => "message",
            other => panic!("unexpected room event {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        chronology,
        vec![
            "evidence",
            "message",
            "question",
            "reason-against",
            "reason-for",
            "evidence"
        ],
        "contributions and conversation interleave as the journal admitted them"
    );

    let credited = participants(&runtime, aged())
        .into_iter()
        .find(|participant| participant.lane == uuid(LANE))
        .expect("the contributing lane participates");
    assert!(
        credited
            .reasons
            .contains(&app::ParticipantReason::Contribution(third)),
        "a typed contribution credits its lane as a participant"
    );
}

#[test]
fn an_empty_room_has_no_contribution_groups() {
    let session = session(OPERATOR, EPOCH);
    let runtime = contribution_runtime(&session, &[room_message(1)]);
    assert!(
        runtime
            .room_projection()
            .contribution_groups(&room())
            .is_empty()
    );
    assert!(runtime.room_projection().contributions(&room()).is_empty());
}

fn sources_reply(
    session: &Session,
    publication: &RoomAuthorityStamp,
    sources: Vec<RoomSourceView>,
) -> app::Admitted<Response<RoomContributionSourcesAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(9)),
            body: RoomContributionSourcesAnswer {
                publication: publication.clone(),
                sources,
            },
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("sources response is admitted")
}

fn sources_refusal(
    session: &Session,
    reason: &str,
) -> app::Admitted<Response<RoomContributionSourcesAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::<RoomContributionSourcesAnswer>::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new(reason.to_owned(), None, None, None),
        )),
    );
    session
        .admit_frame(&bytes)
        .expect("sources refusal is admitted")
}

/// Only a shown Design's cited contributions owe a source read, each taken
/// once within the pump's limit; an unsent read is owed again, a settled read
/// shows each source's typed state, and a reconnect re-reads them all.
#[test]
fn shown_designs_resolve_each_cited_contributions_sources_once() {
    let session = session(OPERATOR, EPOCH);
    let [first, second, third, ..] = stamps();
    let snapshot = [
        contributed(
            1,
            &first,
            LANE,
            RoomContributionKind::Evidence,
            vec![journal(1)],
        ),
        contributed(2, &second, LANE, RoomContributionKind::Question, Vec::new()),
        contributed(
            3,
            &third,
            SECOND_LANE,
            RoomContributionKind::ReasonFor,
            vec![journal(2), artifact()],
        ),
    ];
    let mut runtime = contribution_runtime(&session, &snapshot);
    assert!(
        runtime.take_contribution_source_reads(8).is_empty(),
        "no surface shows the Design, so nothing is read"
    );

    runtime.show_task_details([room()]);
    assert_eq!(
        runtime.take_contribution_source_reads(1),
        vec![(room(), first.clone())],
        "the oldest cited contribution reads first, within the limit"
    );
    assert_eq!(
        runtime.take_contribution_source_reads(8),
        vec![(room(), third.clone())],
        "an uncited contribution owes no read"
    );
    assert!(runtime.take_contribution_source_reads(8).is_empty());
    assert_eq!(
        runtime.contribution_sources(&third),
        Some(&ContributionSources::Loading)
    );

    runtime.restore_contribution_source_read(&third);
    assert_eq!(
        runtime.take_contribution_source_reads(8),
        vec![(room(), third.clone())],
        "an unsent read is owed again"
    );

    let views = vec![
        RoomSourceView::Available { source: journal(2) },
        RoomSourceView::Unavailable {
            source: artifact(),
            unavailable: RoomSourceUnavailable::Missing {},
        },
    ];
    runtime.apply_contribution_sources(&third, &sources_reply(&session, &third, views.clone()));
    assert_eq!(
        runtime.contribution_sources(&third),
        Some(&ContributionSources::Read(views.clone()))
    );
    runtime.apply_contribution_sources(&third, &sources_reply(&session, &third, Vec::new()));
    assert_eq!(
        runtime.contribution_sources(&third),
        Some(&ContributionSources::Read(views)),
        "a settled read is not overwritten by a late answer"
    );

    runtime.apply_contribution_sources(
        &first,
        &sources_reply(
            &session,
            &third,
            vec![RoomSourceView::Available { source: journal(2) }],
        ),
    );
    assert!(
        matches!(
            runtime.contribution_sources(&first),
            Some(ContributionSources::Failed(_))
        ),
        "an answer naming another contribution never becomes this one's sources"
    );

    runtime.reconnect(&acceptance(&session, "operator", EPOCH));
    runtime.apply_stream(&operator_facts(&session, snapshot.len() as u64));
    runtime.apply_stream(&room_page(&session, &snapshot, None, snapshot.len() as u64));
    runtime.show_task_details([room()]);
    assert_eq!(
        runtime.take_contribution_source_reads(8),
        vec![(room(), first.clone()), (room(), third.clone())],
        "a reconnect re-reads every source, since availability may have changed"
    );
    runtime.apply_contribution_sources(
        &first,
        &sources_refusal(&session, "room::contribution-missing"),
    );
    assert_eq!(
        runtime.contribution_sources(&first),
        Some(&ContributionSources::Failed(
            "room::contribution-missing".to_owned()
        ))
    );
}

fn noticed_at(
    position: u64,
    statement: u64,
    design: &wire::TaskStamp,
    lane: &str,
) -> serde_json::Value {
    envelope(
        position,
        "room::noticed",
        &DesignNoticed {
            design: design.clone(),
            statement: wire::Seq::new(statement).expect("fixture statement"),
            lane: uuid(lane),
            occupant: uuid(REQUEST),
            task: room(),
            claim: wire::SequenceCode::new(position),
            terms: vec!["rooms".to_owned()],
        },
    )
}

#[test]
fn maturity_projection_is_incremental_bounded_and_attention_only() {
    const THIRD: &str = "33333333-3333-4333-8333-333333333333";
    let session = session(OPERATOR, EPOCH);
    let [first, second, third, ..] = stamps();
    let snapshot = [
        noticed_at(1, 10, &room(), LANE),
        noticed_at(2, 10, &room(), SECOND_LANE),
        noticed_at(3, 10, &room(), THIRD),
        attributed(4, 10, &first, LANE, RoomContributionKind::ReasonFor),
        attributed(
            5,
            10,
            &second,
            SECOND_LANE,
            RoomContributionKind::ReasonAgainst,
        ),
        attributed(6, 10, &third, THIRD, RoomContributionKind::Evidence),
    ];
    let runtime = contribution_runtime(&session, &snapshot);
    let projection = runtime.room_projection();
    let maturity = projection.maturity(&room()).expect("maturity projects");

    assert_eq!(maturity.state(), wire::rooms::DesignMaturityState::Steeped);
    assert_eq!(maturity.recent_heads.len(), 1);
    assert_eq!(maturity.recent_heads[0].requested_contributors(), 3);
    assert_eq!(
        projection.maturity_attention().cloned().collect::<Vec<_>>(),
        vec![room()]
    );
    assert!(
        projection.room(&room()).is_some(),
        "attention evidence does not alter ordinary room standing"
    );
}
