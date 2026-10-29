use super::*;
use wire::rooms::{
    DesignMaturity, DesignMaturityState, DesignNoticeEvidence, DesignNoticeOpportunity,
    DesignNoticed, DesignSession, RoomAuthorityStamp, RoomContributed, RoomContributionKind,
    RoomLeft, RoomQuestion, RoomQuestionItem,
};

/// The one question item item-bearing fixture posts charge.
const ITEM: &str = "Which input steeps a Design?";

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
        admission: wire::rooms::DesignAdmission::Scored(
            wire::rooms::DesignNoticeMatch::new(wire::rooms::DesignReach::Wide, 0.2, 0.1, [1; 32])
                .expect("fixture match clears its floor"),
        ),
        asks: Vec::new(),
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
    contributed_on(journal, design, lane, kind, statement_head, micros, None)
}

/// [`contributed`] naming question item `(position, text)` of the statement.
fn contributed_on(
    journal: &mut Journal,
    design: &TaskStamp,
    lane: CanonicalUuid,
    kind: RoomContributionKind,
    statement_head: Option<Seq>,
    micros: i64,
    item: Option<(u16, &str)>,
) -> RoomContributed {
    let question = item.and_then(|(position, _)| RoomQuestion::new(position));
    let head = statement(journal, design);
    let handle = wire::TaskHandle::for_filing(&head.project, design, head.filing);
    let publication = RoomAuthorityStamp::for_micros(micros).expect("publication");
    let body = RoomContributed {
        room: design.clone(),
        publication: publication.clone(),
        lane,
        contribution: kind,
        question,
        item: item.map(|(_, text)| RoomQuestionItem::of(text)),
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
            question,
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
        contributed_on(
            &mut journal,
            &design,
            lane(1),
            RoomContributionKind::charge(1),
            Some(statement),
            1_000,
            Some((1, ITEM)),
        ),
        contributed_on(
            &mut journal,
            &design,
            lane(2),
            RoomContributionKind::charge(-1),
            Some(statement),
            1_001,
            Some((1, ITEM)),
        ),
        contributed(
            &mut journal,
            &design,
            lane(3),
            RoomContributionKind::charge(0),
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
        contributed_on(
            &mut journal,
            &design,
            lane(1),
            RoomContributionKind::charge(-1),
            Some(statement),
            1_005,
            Some((1, ITEM)),
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
    assert_eq!(
        actual.state(&[ITEM.to_owned()]),
        DesignMaturityState::Steeped
    );
}

/// Schema 89 labelled contributions. The crossing rewrites each label to its
/// charge on the fact, its request, and its reply together, so replay still
/// proves every receipt, retained posts name no question item, and maturity
/// re-folds to exactly the measure the labels gave.
#[test]
fn retained_contribution_labels_cross_to_charges_with_their_receipts() {
    let scratch = Scratch::new();
    let path = scratch.path().join("contribution-charges.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let design = design(&mut journal, 1);
    let statement = current_statement(&journal, &design);
    let labels = [
        ("reason-for", RoomContributionKind::charge(1)),
        ("reason-against", RoomContributionKind::charge(-1)),
        ("evidence", RoomContributionKind::charge(0)),
        ("question", RoomContributionKind::Question),
    ];
    for (offset, (_, kind)) in (1_u8..).zip(labels) {
        contributed(
            &mut journal,
            &design,
            lane(offset),
            kind,
            Some(statement),
            2_000 + i64::from(offset),
        );
    }
    let before = journal
        .read(|read| read.design_maturity(&design))
        .expect("labelled maturity reads");
    for (label, kind) in labels {
        let RoomContributionKind::Charge(charge) = kind else {
            continue;
        };
        let charge = charge.get();
        journal
            .connection
            .execute_batch(&format!(
                "UPDATE command_receipts SET
                     request = CAST(json_set(CAST(request AS TEXT), '$.body.kind', '{label}') AS BLOB),
                     response = CAST(json_set(CAST(response AS TEXT), '$.outcome.body.kind', '{label}') AS BLOB)
                 WHERE command_kind = 'room::contribute'
                   AND json_extract(CAST(request AS TEXT), '$.body.kind') = {charge};
                 UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), '$.fact.contribution', '{label}') AS BLOB)
                 WHERE kind = 'room::contributed'
                   AND json_extract(CAST(envelope AS TEXT), '$.fact.contribution') = {charge};"
            ))
            .expect("restore the retained label");
    }
    let retained: (u32, u32, u32) = journal
        .connection
        .query_row(
            "SELECT
                 (SELECT count(*) FROM fact_envelopes WHERE kind = 'room::contributed'
                    AND json_type(CAST(envelope AS TEXT), '$.fact.contribution') = 'text'),
                 (SELECT count(*) FROM command_receipts WHERE command_kind = 'room::contribute'
                    AND json_type(CAST(request AS TEXT), '$.body.kind') = 'text'),
                 (SELECT count(*) FROM command_receipts WHERE command_kind = 'room::contribute'
                    AND json_type(CAST(response AS TEXT), '$.outcome.body.kind') = 'text')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("retained labels count");
    assert_eq!(retained, (4, 4, 4), "every fixture post carries a label");
    crate::tests::schema::restore_predecessor(&journal, 89);
    drop(journal);

    let journal = Journal::open(&path).expect("labelled contributions cross");
    journal
        .verify()
        .expect("every crossed receipt still proves its fact");
    let crossed = journal
        .connection
        .prepare(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'room::contributed' ORDER BY seq",
        )
        .expect("contributions select")
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("contributions query")
        .map(|bytes| {
            wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes.expect("row reads"))
                .expect("crossed fact decodes")
                .fact
                .read::<RoomContributed>()
                .expect("contribution fact")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        crossed
            .iter()
            .map(|contribution| (contribution.contribution, contribution.question))
            .collect::<Vec<_>>(),
        labels.map(|(_, kind)| (kind, None)),
        "each label takes its charge and no question item"
    );
    assert_eq!(
        journal
            .read(|read| read.design_maturity(&design))
            .expect("crossed maturity reads"),
        before
    );
    drop(journal);
    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}

/// Schema 90 named a charged item by position alone. The crossing records each
/// item's text identity from the statement the post witnessed while that
/// statement is still current, re-folding to the same series; a post whose
/// witnessed statement the journal no longer keeps refuses, naming the
/// release that wrote it.
#[test]
fn retained_item_positions_cross_to_text_identities_from_their_current_statement() {
    let items = ["  Which INPUT\tsteeps a Design? ", "Who answers it?"];
    let fixture = |name: &str| {
        let scratch = Scratch::new();
        let path = scratch.path().join(name);
        let mut journal = authority_file_fixture(&path);
        let design = design_with_acceptance(&mut journal, 1, &items);
        let statement = current_statement(&journal, &design);
        for (micros, (by, value, position)) in
            (3_000..).zip([(1, 1, 1), (2, -1, 1), (1, -1, 1), (3, 1, 2)])
        {
            contributed_on(
                &mut journal,
                &design,
                lane(by),
                RoomContributionKind::charge(value),
                Some(statement),
                micros,
                Some((position, items[usize::from(position) - 1])),
            );
        }
        let before = journal
            .read(|read| read.design_maturity(&design))
            .expect("positioned maturity reads");
        journal
            .connection
            .execute_batch(
                "UPDATE fact_envelopes
                 SET envelope = CAST(json_remove(CAST(envelope AS TEXT), '$.fact.item') AS BLOB)
                 WHERE kind = 'room::contributed'",
            )
            .expect("restore the retained position-only shape");
        (scratch, path, journal, design, before)
    };

    let (_scratch, path, journal, design, before) = fixture("contribution-items.sqlite3");
    crate::tests::schema::restore_predecessor(&journal, 90);
    drop(journal);
    let journal = Journal::open(&path).expect("positioned contributions cross");
    journal
        .verify()
        .expect("every crossed receipt still proves its fact");
    let crossed = journal
        .read(|read| read.design_maturity(&design))
        .expect("crossed maturity reads");
    assert_eq!(
        crossed, before,
        "each series re-folds under its text identity"
    );
    let acceptance = items.map(str::to_owned);
    assert_eq!(crossed.state(&acceptance), DesignMaturityState::Steeped);
    drop(journal);
    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");

    let (_scratch, path, journal, _, _) = fixture("superseded-items.sqlite3");
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_set(CAST(envelope AS TEXT), '$.fact.statement', 999999) AS BLOB)
             WHERE kind = 'room::contributed'",
        )
        .expect("witness a statement the journal no longer keeps");
    crate::tests::schema::restore_predecessor(&journal, 90);
    drop(journal);
    let error = Journal::open(&path).expect_err("a superseded witness refuses");
    let OpenError::Migration(MigrationError::RetainedRow { release, .. }) = &error else {
        panic!("expected a retained-row refusal, got {error}");
    };
    assert_eq!(*release, "schema-90", "{error}");
}
