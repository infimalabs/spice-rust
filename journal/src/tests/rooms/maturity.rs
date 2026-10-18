use super::*;
use wire::rooms::{
    DesignMaturity, DesignNoticeEvidence, DesignNoticeOpportunity, DesignNoticed, DesignSession,
    RoomAuthorityStamp, RoomContributed, RoomContributionKind, RoomLeft,
};

fn current_statement(journal: &Journal, task: &TaskStamp) -> Seq {
    journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("task exists")
        .position()
}
fn current_workflow(journal: &Journal, task: &TaskStamp) -> Seq {
    journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("task exists")
        .position()
}

fn design(journal: &mut Journal, offset: i64) -> TaskStamp {
    design_task(journal, offset)
}

fn noticed(
    journal: &mut Journal,
    design: &TaskStamp,
    claimed: &TaskStamp,
    lane: CanonicalUuid,
) -> DesignNoticed {
    let completed = Seq::new(2).expect("completion");
    let body = DesignNoticed {
        design: design.clone(),
        statement: current_statement(journal, design),
        lane,
        occupant: lane,
        session: DesignSession {
            vendor: "omp".parse().expect("vendor"),
            identity: "fixture-session".to_owned(),
        },
        compaction: None,
        opportunity: DesignNoticeOpportunity::TaskCompletion {
            task: claimed.clone(),
            completed,
        },
        evidence: DesignNoticeEvidence::Task {
            task: claimed.clone(),
            statement: current_statement(journal, claimed),
            completed,
            support: wire::rooms::DesignSupport::Implemented,
        },
        excerpt: "measured design maturity".to_owned(),
        terms: vec![
            "design".to_owned(),
            "maturity".to_owned(),
            "measure".to_owned(),
        ],
    };
    let mut write = JournalWrite::new(
        Actor::Spice,
        "fixture:maturity".parse().expect("origin"),
        instant(),
        Producer::ResidentRecognition,
    );
    write
        .fact(body.clone(), FactCause::Root)
        .expect("notice builds");
    journal.apply(write).expect("notice commits");
    body
}

fn contributed(
    journal: &mut Journal,
    design: &TaskStamp,
    lane: CanonicalUuid,
    kind: RoomContributionKind,
    statement_head: Option<Seq>,
    micros: i64,
) -> RoomContributed {
    let head = statement(journal, design);
    let handle = wire::TaskHandle::for_filing(&head.project, design, head.filing);
    let publication = RoomAuthorityStamp::for_micros(micros).expect("publication");
    let body = RoomContributed {
        room: design.clone(),
        publication: publication.clone(),
        lane,
        contribution: kind,
        statement: statement_head,
        text: "Substantive contribution.".to_owned(),
        sources: Vec::new(),
    };
    let epoch = self::lane(250);
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: format!("10000000-0000-4000-8000-{micros:012}")
            .parse()
            .expect("command id"),
        epoch,
        kind: wire::CommandKind::RoomContribute,
        expected: vec![
            wire::Witness::Task {
                task: design.clone(),
                head: current_statement(journal, design),
            },
            wire::Witness::Workflow {
                task: design.clone(),
                head: current_workflow(journal, design),
                phase: phase("todo"),
            },
        ],
        body: wire::intent::RoomContribute {
            room: handle.clone(),
            kind,
            text: body.text.clone(),
            sources: Vec::new(),
        },
    };
    let answer =
        wire::intent::RoomContributeAnswer::new(handle, publication, kind).expect("answer");
    let mut write = JournalWrite::new(
        Actor::Lane(lane),
        "fixture:maturity".parse().expect("origin"),
        instant(),
        Producer::CommandDoor,
    );
    write
        .fact(body.clone(), FactCause::Root)
        .expect("contribution builds");
    write
        .command_result(command, epoch, answer)
        .expect("receipt builds");
    journal.apply(write).expect("contribution commits");
    body
}

fn left(journal: &mut Journal, design: &TaskStamp, lane: CanonicalUuid, micros: i64) -> RoomLeft {
    let body = RoomLeft {
        room: design.clone(),
        publication: RoomAuthorityStamp::for_micros(micros).expect("publication"),
        lane,
        clause: "nothing to add".to_owned(),
    };
    let mut write = JournalWrite::new(
        Actor::Lane(lane),
        "fixture:maturity".parse().expect("origin"),
        instant(),
        Producer::CommandDoor,
    );
    write
        .fact(body.clone(), FactCause::Root)
        .expect("leave builds");
    journal.apply(write).expect("leave commits");
    body
}

fn lane(last: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{last:012}")
        .parse()
        .expect("fixture lane")
}

#[test]
fn indexed_and_live_maturity_are_the_same_bounded_measure() {
    let mut journal = Journal::in_memory().expect("journal");
    let design = design(&mut journal, 1);
    let claimed = task(&mut journal, 2, None);
    let statement = current_statement(&journal, &design);
    let notices = [
        noticed(&mut journal, &design, &claimed, lane(1)),
        noticed(&mut journal, &design, &claimed, lane(2)),
        noticed(&mut journal, &design, &claimed, lane(3)),
        noticed(&mut journal, &design, &claimed, lane(5)),
    ];
    let contributions = [
        contributed(
            &mut journal,
            &design,
            lane(1),
            RoomContributionKind::ReasonFor,
            Some(statement),
            1_000,
        ),
        contributed(
            &mut journal,
            &design,
            lane(2),
            RoomContributionKind::ReasonAgainst,
            Some(statement),
            1_001,
        ),
        contributed(
            &mut journal,
            &design,
            lane(3),
            RoomContributionKind::Evidence,
            Some(statement),
            1_002,
        ),
        contributed(
            &mut journal,
            &design,
            lane(4),
            RoomContributionKind::Question,
            None,
            1_003,
        ),
    ];
    let declined = left(&mut journal, &design, lane(5), 1_004);
    let mut expected = DesignMaturity::default();
    for notice in &notices {
        expected.observe_request(notice);
    }
    for contribution in &contributions {
        expected.observe_contribution(contribution);
    }
    expected.observe_leave(&declined);

    let (actual, page) = journal
        .read(|read| {
            Ok((
                read.design_maturity(&design)?,
                read.design_notices(&design, None, FactReadBudget::RETAINED)?,
            ))
        })
        .expect("projection reads");
    assert_eq!(actual, expected);
    assert_eq!(page.rows.len(), notices.len());
    assert_eq!(actual.unrecorded_head.contributions, 1);
    assert_eq!(actual.recent_heads[0].requested_contributors(), 3);
    assert_eq!(actual.recent_heads[0].requested_decliners(), 1);
    assert_eq!(actual.recent_heads[0].requested_unanswered(), 0);
    assert_eq!(actual.state(), wire::rooms::DesignMaturityState::Steeped);
}
