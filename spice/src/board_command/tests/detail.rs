use super::*;

fn detail(bus: &Bus, epoch: CanonicalUuid, task: wire::TaskStamp) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDetail,
        Vec::new(),
        &wire::intent::TaskDetail { task },
    )
}

fn detail_with_repository(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    task: wire::TaskStamp,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::TaskDetail,
        Vec::new(),
        &wire::intent::TaskDetail { task },
    );
    let projects = ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("board command test project policy");
    dispatch_with_repository(
        bus,
        &command,
        Some(repository),
        &projects,
        &board::NextConfig::default(),
    )
    .expect("board command owns task detail")
}

#[test]
fn task_detail_returns_one_task_and_ordered_attributed_notes() {
    let (root, bus, epoch, seats) = fixture("task-detail", 1);
    let born = stamp("task-detail");
    let task = add_born(&bus, epoch, born.clone(), "Inspect one task");
    let statement = at(&board(&bus, epoch, Actor::Operator), &task)
        .statement
        .body
        .born
        .clone();

    for (actor, text) in [
        (Actor::Operator, "operator observation"),
        (Actor::Lane(seats[0].lane), "resident observation"),
    ] {
        assert_committed(issue(
            &bus,
            epoch,
            actor,
            CommandKind::TaskNote,
            vec![Witness::Existence {
                task: statement.clone(),
            }],
            &TaskNote {
                task: task.clone(),
                note: text.to_owned(),
            },
        ));
    }

    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::TaskDetail(answer),
    }) = detail(&bus, epoch, born.clone())
    else {
        panic!("task detail did not return a snapshot")
    };
    assert!(position >= answer.notes[1].position);
    assert_eq!(answer.task.statement.body.born, born);
    assert_eq!(answer.notes.len(), 2);
    assert_eq!(answer.notes[0].text, "operator observation");
    assert_eq!(answer.notes[0].author, Actor::Operator);
    assert_eq!(answer.notes[1].text, "resident observation");
    assert_eq!(answer.notes[1].author, Actor::Lane(seats[0].lane));
    assert!(answer.notes[0].position < answer.notes[1].position);
    assert!(answer.notes[0].at <= answer.notes[1].at);

    clean(&root, bus);
}

#[test]
fn task_detail_carries_browser_renderable_artifacts_without_text_sidecars() {
    let (root, bus, epoch, _) = fixture("task-detail-media", 0);
    let born = stamp("task-detail-media");
    add_born(&bus, epoch, born.clone(), "Inspect task media");
    fs::create_dir(root.join(".git")).expect("fixture Git directory exists");
    let repository = Repository::discover(&*root).expect("fixture repository resolves");
    let state = repository
        .admit_state()
        .expect("fixture repository state admits");
    fs::create_dir_all(state.artifacts()).expect("artifact directory exists");
    fs::create_dir_all(state.locks()).expect("artifact lock directory exists");
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    let created_at = "2030-04-12T18:44:09.271Z".parse().expect("fixture instant");
    store
        .add_bytes(artifact::AddBytesRequest {
            task: &born,
            bytes: b"\x89PNG\r\n\x1a\n",
            name: "operator-capture.png",
            media_type: Some(artifact::MediaType::IMAGE_PNG),
            retention: artifact::Retention::Permanent,
            created_at,
        })
        .expect("image artifact publishes");
    store
        .add_bytes(artifact::AddBytesRequest {
            task: &born,
            bytes: b"diagnostic text",
            name: "diagnostic.txt",
            media_type: Some(artifact::MediaType::TEXT_PLAIN),
            retention: artifact::Retention::Permanent,
            created_at,
        })
        .expect("text artifact publishes");

    let Outcome::Reply(Reply {
        body: CommandAnswer::TaskDetail(answer),
        ..
    }) = detail_with_repository(&bus, &repository, epoch, born)
    else {
        panic!("task detail did not return media")
    };
    assert_eq!(answer.media.len(), 1);
    assert_eq!(answer.media[0].name, "operator-capture.png");
    assert_eq!(answer.media[0].content_type, "image/png");
    assert_eq!(
        answer.media[0]
            .decoded_bytes()
            .expect("projected media remains admitted"),
        b"\x89PNG\r\n\x1a\n"
    );

    clean(&root, bus);
}

#[test]
fn task_detail_refuses_an_unknown_durable_identity() {
    let (root, bus, epoch, _) = fixture("task-detail-unknown", 0);
    let outcome = detail(&bus, epoch, stamp("missing-detail"));
    assert!(matches!(
        &outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "task::detail-unknown"
    ));
    clean(&root, bus);
}
