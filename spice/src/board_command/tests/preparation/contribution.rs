//! A lane's typed contribution to an open Design room: admitted against one
//! snapshot and its cited sources, read back grouped by kind beside the
//! complete chronological history, and authorizing nothing.

use super::*;

use serde_json::{Value, json};
use wire::intent::{Intent, RoomContribute, RoomContributeAnswer};
use wire::rooms::{
    ROOM_CONTRIBUTION_SOURCE_LIMIT, ROOM_CONTRIBUTION_TEXT_LIMIT, RoomContributed,
    RoomContributionKind, RoomPost, TranscriptHunk,
};

use crate::room_command::SourceOwners;

fn projects() -> ProjectPolicy {
    ProjectPolicy::new(
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("contribution fixture project policy")
}

/// The task artifact store under `root`.
fn artifacts(root: &Path) -> artifact::Store {
    fs::create_dir_all(root.join("artifacts")).expect("artifact directory exists");
    fs::create_dir_all(root.join("locks")).expect("artifact lock directory exists");
    artifact::Store::new(
        root.join("artifacts"),
        root.join("locks/task-artifacts.lock"),
    )
}

fn retain(root: &Path, task: &TaskStamp, bytes: &[u8]) -> wire::Digest {
    artifacts(root)
        .add_bytes(artifact::AddBytesRequest {
            task,
            bytes,
            name: "evidence.txt",
            media_type: Some(artifact::MediaType::TEXT_PLAIN),
            retention: artifact::Retention::Permanent,
            created_at: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        })
        .expect("fixture artifact publishes")
        .digest
}

fn contribution_body(
    room: &TaskHandle,
    kind: RoomContributionKind,
    text: &str,
    sources: Vec<RoomSource>,
) -> RoomContribute {
    RoomContribute {
        room: room.clone(),
        kind,
        text: text.to_owned(),
        sources,
    }
}

/// Contribute `body` as `actor`, witnessing the Design as the board reads now.
fn contribute(
    bus: &Bus,
    epoch: CanonicalUuid,
    root: &Path,
    actor: Actor,
    body: RoomContribute,
) -> Outcome<CommandAnswer> {
    let visible = board(bus, epoch, Actor::Operator);
    let heads = at(&visible, &body.room);
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::RoomContribute,
        vec![statement_witness(heads), workflow_witness(heads)],
        &body,
    );
    let Intent::RoomContribute(request) = command.body() else {
        panic!("fixture command carries a contribution");
    };
    let store = artifacts(root);
    let tape = root.join("tape");
    room_contribute(
        bus,
        &command,
        request,
        Some(SourceOwners {
            tape: &tape,
            artifacts: &store,
        }),
        &projects(),
        &board::NextConfig::default(),
    )
}

fn contributed(outcome: Outcome<CommandAnswer>) -> RoomContributeAnswer {
    match assert_committed(outcome) {
        CommandAnswer::RoomContribute(answer) => answer,
        other => panic!("contribution answered {other:?}"),
    }
}

fn refused(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// One `spice room history` page, its sources read under `artifacts_root`.
fn history(root: &Path, artifacts_root: &Path, words: &[&str]) -> Value {
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "room", "history"].iter().chain(words))
        .expect("room history parses");
    let contract = crate::cli::selected_contract(&matches).expect("room history is registered");
    let journal = journal::JournalReader::open(root.join("authority.sqlite3"))
        .expect("fixture journal opens to read");
    let store = artifacts(artifacts_root);
    let tape = root.join("tape");
    let outcome = crate::room_command::answer(
        &journal,
        SourceOwners {
            tape: &tape,
            artifacts: &store,
        },
        &projects(),
        crate::cli::selected_matches(&matches),
        contract.path,
    );
    let Outcome::Reply(reply) = outcome else {
        panic!("room history returned another outcome: {outcome:?}");
    };
    serde_json::to_value(reply.body).expect("room history encodes")
}

fn publications(page: &Value) -> Vec<String> {
    page["page"]["rows"]
        .as_array()
        .expect("history rows")
        .iter()
        .map(|row| row["publication"].as_str().expect("publication").to_owned())
        .collect()
}

/// Every authority a contribution must leave alone, as the operator reads it.
fn authority(bus: &Bus, epoch: CanonicalUuid, root: &FixtureRoot, design: &TaskHandle) -> Value {
    let visible = board(bus, epoch, Actor::Operator);
    let task = at(&visible, design);
    let mut preparation = preparation_page(root, &[design.as_str()]);
    if let Some(object) = preparation.as_object_mut() {
        object.remove("position");
    }
    json!({
        "statement": serde_json::to_value(&task.statement).expect("statement encodes"),
        "workflow": serde_json::to_value(&task.workflow).expect("workflow encodes"),
        "claim": serde_json::to_value(&task.claim).expect("claim encodes"),
        "preparation": preparation,
    })
}

#[test]
fn lanes_contribute_typed_records_that_read_grouped_and_authorize_nothing() {
    let (root, bus, epoch, seats) = fixture("design-contribution", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Contributed design",
        wire::TaskIntent::Design,
        None,
    );
    let born = design.born().expect("fixture Design has a birth");
    let digest = retain(root.as_ref(), &born, b"p99 latency 41ms");
    let visible = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![statement_witness(at(&visible, &design))],
        &RoomPost {
            room: design.clone(),
            text: "Weigh the journal index.".to_owned(),
            grooming: None,
        },
    ));
    let cited = journal_source(&visible, &design);
    let before = authority(&bus, epoch, &root, &design);

    let written = write_contributions(&bus, epoch, root.as_ref(), &lane, &design, &cited, &digest);
    assert_eq!(
        authority(&bus, epoch, &root, &design),
        before,
        "contributing changes no statement, workflow, claim, candidate, decision, or authorization"
    );

    let page = history(root.as_ref(), root.as_ref(), &[design.as_str()]);
    let rows = page["page"]["rows"].as_array().expect("history rows");
    assert_eq!(
        rows.len(),
        6,
        "the complete history keeps every publication"
    );
    assert_eq!(
        rows[0]["content"],
        json!({ "state": "text", "text": "Weigh the journal index." }),
        "an untyped post stays conversation"
    );
    assert_eq!(publications(&page)[1..], written[..]);
    let first = &rows[1];
    assert_eq!(first["actor"], serde_json::to_value(&lane).expect("actor"));
    assert!(
        first["at"].is_string(),
        "the record names its time: {first}"
    );
    assert_eq!(
        first["content"],
        json!({
            "state": "contribution",
            "kind": "reason-for",
            "text": "Reuse the journal index.",
            "sources": [{ "state": "available", "source": cited }],
        })
    );
    assert_eq!(
        rows[4]["content"]["text"], "Pages already exist.",
        "the recorded text is the trimmed contribution"
    );
    let grouped = json!([
        { "kind": "reason-for", "publications": [written[0], written[3]] },
        { "kind": "reason-against", "publications": [written[4]] },
        { "kind": "evidence", "publications": [written[2]] },
        { "kind": "question", "publications": [written[1]] },
    ]);
    assert_eq!(page["contributions"], grouped);

    assert_contribution_pages(root.as_ref(), &design, &written, &grouped);

    // An artifact the reader's store no longer retains reads as a typed
    // unavailable link, never as fabricated text.
    let elsewhere = root.as_ref().join("pruned");
    let pruned = history(root.as_ref(), &elsewhere, &[design.as_str()]);
    assert_eq!(
        pruned["page"]["rows"][3]["content"]["sources"],
        json!([{
            "state": "unavailable",
            "source": { "kind": "artifact", "digest": digest },
            "unavailable": { "reason": "missing" },
        }])
    );
    assert_eq!(
        pruned["page"]["rows"][3]["content"]["text"],
        "Measured under load."
    );
    clean(&root, bus);
}

/// Write five contributions, one with each source kind it can cite, in order.
fn write_contributions(
    bus: &Bus,
    epoch: CanonicalUuid,
    root: &Path,
    lane: &Actor,
    design: &TaskHandle,
    cited: &RoomSource,
    digest: &wire::Digest,
) -> Vec<String> {
    let artifact = RoomSource::Artifact {
        digest: digest.clone(),
    };
    [
        (
            RoomContributionKind::ReasonFor,
            "Reuse the journal index.",
            vec![cited.clone()],
        ),
        (
            RoomContributionKind::Question,
            "Who owns retention?",
            Vec::new(),
        ),
        (
            RoomContributionKind::Evidence,
            "Measured under load.",
            vec![artifact],
        ),
        (
            RoomContributionKind::ReasonFor,
            "  Pages already exist.\n",
            Vec::new(),
        ),
        (
            RoomContributionKind::ReasonAgainst,
            "Index rebuilds are slow.",
            Vec::new(),
        ),
    ]
    .into_iter()
    .map(|(kind, text, sources)| {
        let body = contribution_body(design, kind, text, sources);
        let answer = contributed(contribute(bus, epoch, root, lane.clone(), body));
        assert_eq!(answer.kind, kind);
        answer.publication.to_string()
    })
    .collect()
}

/// `--contributions` pages only contributions, grouping just what it shows,
/// and a continuation names the first withheld contribution.
fn assert_contribution_pages(
    root: &Path,
    design: &TaskHandle,
    written: &[String],
    grouped: &Value,
) {
    let room = design.as_str();
    let only = history(root, root, &["--contributions", room]);
    assert_eq!(publications(&only), written, "only contributions, in order");
    assert_eq!(&only["contributions"], grouped);
    let first_page = history(root, root, &["--contributions", "--limit", "2", room]);
    assert_eq!(publications(&first_page), written[..2]);
    assert_eq!(
        first_page["contributions"],
        json!([
            { "kind": "reason-for", "publications": [written[0]] },
            { "kind": "question", "publications": [written[1]] },
        ]),
        "a page groups only the contributions it shows"
    );
    let rest = history(
        root,
        root,
        &["--contributions", "--after", &written[2], room],
    );
    assert_eq!(publications(&rest), written[2..]);
}

#[test]
fn contribution_admission_refuses_typed_and_writes_nothing() {
    let (root, bus, epoch, seats) = fixture("design-contribution-refusals", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Refusing design",
        wire::TaskIntent::Design,
        None,
    );
    let plan = add_plan(&bus, epoch, "Not a design");
    let other = retain(
        root.as_ref(),
        &plan.born().expect("fixture Plan has a birth"),
        b"retained elsewhere",
    );
    let journal = |fact: u64| RoomSource::Journal {
        fact: wire::SequenceCode::new(fact),
    };
    let attempt = |actor: Actor, room: &TaskHandle, text: &str, sources: Vec<RoomSource>| {
        let body = contribution_body(room, RoomContributionKind::ReasonFor, text, sources);
        contribute(&bus, epoch, root.as_ref(), actor, body)
    };
    let operator = attempt(Actor::Operator, &design, "Operator view.", Vec::new());
    assert_eq!(refused(&operator), "room::contribution-lane-only");
    let wrong_room = attempt(lane.clone(), &plan, "Wrong room.", Vec::new());
    assert_eq!(refused(&wrong_room), "room::contribution-not-design");
    let too_long = "x".repeat(ROOM_CONTRIBUTION_TEXT_LIMIT + 1);
    let too_many = (1..=ROOM_CONTRIBUTION_SOURCE_LIMIT as u64 + 1)
        .map(journal)
        .collect();
    let unavailable = "room::contribution-source-unavailable";
    for (text, sources, reason) in [
        (" \n ", Vec::new(), "room::contribution-empty"),
        (too_long.as_str(), Vec::new(), "room::contribution-too-long"),
        ("Too many.", too_many, "room::contribution-sources-exceeded"),
        (
            "Twice.",
            vec![journal(1), journal(1)],
            "room::contribution-source-duplicate",
        ),
        ("Future fact.", vec![journal(1_000_000)], unavailable),
        (
            "Another task's artifact.",
            vec![RoomSource::Artifact { digest: other }],
            unavailable,
        ),
        (
            "Unwritten tape.",
            vec![RoomSource::Transcript(unwritten_hunk(seats[0].lane))],
            unavailable,
        ),
    ] {
        let outcome = attempt(lane.clone(), &design, text, sources);
        assert_eq!(refused(&outcome), reason, "{outcome:?}");
    }
    let empty = history(
        root.as_ref(),
        root.as_ref(),
        &["--contributions", design.as_str()],
    );
    assert_eq!(publications(&empty), Vec::<String>::new());

    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
    ));
    let late = attempt(lane, &design, "After the fact.", Vec::new());
    assert_eq!(refused(&late), "room::contribution-design-concluded");
    clean(&root, bus);
}

/// A hunk of a transcript no tape holds.
fn unwritten_hunk(lane: CanonicalUuid) -> TranscriptHunk {
    TranscriptHunk {
        head: wire::TranscriptSourceHead {
            lane,
            vendor: "omp".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid(91_001),
            next: wire::body::TranscriptPosition {
                member: 1,
                offset: 64,
            },
        },
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 64,
            event: 0,
        },
        text: wire::rooms::TextRange { start: 0, end: 6 },
    }
}

#[test]
fn the_journal_admits_only_a_lane_own_bounded_contribution_to_an_open_design() {
    let (root, bus, epoch, seats) = fixture("design-contribution-journal", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Journal design",
        wire::TaskIntent::Design,
        None,
    );
    let plan = add_plan(&bus, epoch, "Journal plan");
    let author = seats[0].lane;
    let minted = std::cell::Cell::new(1_790_000_000_000_001);
    let commit = |actor: Actor, room: &TaskHandle, text: &str, sources: Vec<RoomSource>| {
        let room = room.born().expect("fixture task has a birth");
        let publication =
            RoomAuthorityStamp::for_micros(minted.get()).expect("fixture publication identity");
        minted.set(minted.get() + 1);
        let mut write = JournalWrite::new(
            actor,
            "fixture:contribution".parse().expect("fixture origin"),
            Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
            Producer::CommandDoor,
        );
        write
            .fact(
                RoomContributed {
                    room,
                    publication,
                    lane: author,
                    contribution: RoomContributionKind::Evidence,
                    text: text.to_owned(),
                    sources,
                },
                FactCause::Root,
            )
            .expect("fixture contribution builds");
        bus.commit(write)
            .expect("fixture contribution reaches writer")
    };
    let refused = |outcome: CommitOutcome| match outcome {
        CommitOutcome::Refused(refusal) => refusal.reason().to_owned(),
        other => panic!("the journal admitted an unsound contribution: {other:?}"),
    };
    let ahead = RoomSource::Journal {
        fact: wire::SequenceCode::new(1_000_000),
    };
    for outcome in [
        commit(Actor::Operator, &design, "Operator-authored.", Vec::new()),
        commit(
            Actor::Lane(seats[1].lane),
            &design,
            "Another lane.",
            Vec::new(),
        ),
        commit(Actor::Lane(author), &design, "   ", Vec::new()),
        commit(
            Actor::Lane(author),
            &design,
            "Cites the future.",
            vec![ahead],
        ),
        commit(Actor::Lane(author), &plan, "Not a design.", Vec::new()),
    ] {
        assert_eq!(refused(outcome), journal::ROOM_AUTHORITY_REFUSED);
    }
    assert!(matches!(
        commit(Actor::Lane(author), &design, "Sound.", Vec::new()),
        CommitOutcome::Committed(_)
    ));
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
    ));
    assert_eq!(
        refused(commit(
            Actor::Lane(author),
            &design,
            "Too late.",
            Vec::new()
        )),
        journal::ROOM_AUTHORITY_REFUSED,
        "a concluded Design accepts no contribution at commit"
    );
    clean(&root, bus);
}
