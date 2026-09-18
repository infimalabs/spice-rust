use super::*;

/// The resident stopped admitting: the socket refusal `closing_refusal`
/// sends once admission has closed, bound to the command it answers.
fn not_admitting(id: CanonicalUuid, epoch: CanonicalUuid) -> Vec<u8> {
    wire::encode(&Response::<serde_json::Value> {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            "resident::not-admitting",
            Some("resident-state".to_owned()),
            Some("seated".to_owned()),
            Some("quiescing".to_owned()),
        ))),
    })
    .expect("not-admitting refusal encodes")
}

/// A resident ending under one command, and what the client learns from it.
///
/// The predecessor answers the command with `resident::not-admitting`, then
/// withdraws its discovery and drops the lock exactly as `spice down` does.
/// With `successor` set, a second resident seats after a pause the client
/// spends watching an unheld lock, holds its own epoch, publishes, and serves
/// the resubmitted envelope. Without it, nothing ever publishes again and the
/// client reaches its bound.
fn restart_under_command(
    name: &str,
    successor: bool,
    bound: Duration,
) -> (
    Outcome<CommandAnswer>,
    Command<serde_json::Value>,
    Option<Command<serde_json::Value>>,
    Vec<String>,
) {
    let fixture = Arc::new(Mutex::new(Fixture::new(name)));
    let predecessor_listener = TcpListener::bind("127.0.0.1:0").expect("predecessor peer binds");
    let successor_listener = TcpListener::bind("127.0.0.1:0").expect("successor peer binds");
    let predecessor_epoch = uuid(50);
    let successor_epoch = uuid(51);
    let canvas = fixture.lock().expect("fixture locks").canvas.clone();
    canvas::Discovery {
        endpoint: format!(
            "http://{}",
            predecessor_listener
                .local_addr()
                .expect("predecessor peer address")
        ),
        capability: uuid(52),
        epoch: predecessor_epoch,
    }
    .publish(&canvas)
    .expect("predecessor discovery publishes");
    fixture
        .lock()
        .expect("fixture locks")
        .hold(predecessor_epoch);
    let successor_discovery = canvas::Discovery {
        endpoint: format!(
            "http://{}",
            successor_listener
                .local_addr()
                .expect("successor peer address")
        ),
        capability: uuid(53),
        epoch: successor_epoch,
    };

    let predecessor_peer = std::thread::spawn(move || {
        let mut socket = accept_fixture_session(predecessor_listener, predecessor_epoch);
        let command = read_fixture_command(&mut socket);
        socket
            .send(Message::Binary(
                not_admitting(command.id, predecessor_epoch).into(),
            ))
            .expect("predecessor refusal sends");
        command
    });
    let successor_peer = successor.then(|| {
        std::thread::spawn(move || {
            let mut socket = accept_fixture_session(successor_listener, successor_epoch);
            let command = read_fixture_command(&mut socket);
            socket
                .send(Message::Binary(
                    response(command.id, successor_epoch, committed(), state_body()).into(),
                ))
                .expect("successor reply sends");
            command
        })
    });

    let command = mapped();
    let command_id = command.id;
    let mut transients = Vec::new();
    let mut seating = None;
    let outcome = execute_mapped_within(
        &canvas,
        &canvas,
        &COMMAND_CONTRACT,
        command,
        bound,
        |fault| {
            transients.push(fault.reason.clone());
            if fault.reason != "resident::not-admitting" {
                return;
            }
            // `spice down`: discovery is withdrawn before the lock drops.
            fs::remove_file(&canvas).expect("predecessor withdraws discovery");
            fixture.lock().expect("fixture locks").release_publisher();
            if !successor {
                return;
            }
            // `spice up`, a separate command the operator runs afterwards:
            // the lock stays unheld for a while, then the successor takes it
            // and publishes only once it holds its own epoch.
            let fixture = Arc::clone(&fixture);
            let successor_discovery = successor_discovery.clone();
            seating = Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                let mut fixture = fixture.lock().expect("fixture locks");
                fixture.seat_successor();
                fixture.hold(successor_epoch);
                successor_discovery
                    .publish(&fixture.canvas)
                    .expect("successor discovery publishes");
            }));
        },
    );
    if let Some(seating) = seating {
        seating.join().expect("successor seating joins");
    }
    let predecessor = predecessor_peer.join().expect("predecessor peer joins");
    assert_eq!(predecessor.id, command_id);
    let served = successor_peer.map(|peer| peer.join().expect("successor peer joins"));
    (outcome, predecessor, served, transients)
}

#[test]
fn a_witnessed_restart_is_outlasted_and_the_successor_serves_the_same_envelope() {
    let (outcome, predecessor, served, transients) =
        restart_under_command("restart-served", true, Duration::from_secs(10));

    let served = served.expect("the successor accepted the resubmitted command");
    assert_eq!(
        served.id, predecessor.id,
        "one command identity across the restart"
    );
    assert_eq!(served.kind, predecessor.kind);
    assert_eq!(served.expected, predecessor.expected);
    assert_eq!(served.body, predecessor.body);
    assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    assert_eq!(
        transients,
        ["resident::not-admitting", "canvas::discovery-unavailable"],
        "an unheld lock after a witnessed restart is the handoff, not a lost publisher"
    );
}

#[test]
fn a_witnessed_restart_nobody_finishes_is_one_not_admitting_refusal_naming_the_retry() {
    let (outcome, predecessor, served, transients) =
        restart_under_command("restart-unfinished", false, Duration::from_secs(1));

    assert!(served.is_none());
    assert_eq!(
        transients,
        ["resident::not-admitting", "canvas::discovery-unavailable"],
        "the wait names no lost publisher and no `spice up`"
    );
    let Outcome::Refusal(refusal) = outcome else {
        panic!("a restart nobody finishes must keep the resident's refusal: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "resident::not-admitting");
    let diagnostic = refusal
        .diagnostic()
        .expect("the refusal keeps the resident-state diagnostic");
    assert_eq!(diagnostic.witness(), Some("resident-state"));
    assert_eq!(diagnostic.expected(), Some("seated"));
    assert_eq!(diagnostic.observed(), Some("quiescing"));
    let detail = diagnostic
        .detail()
        .expect("the refusal explains the bound the client waited");
    assert!(detail.contains(&uuid(50).to_string()), "{detail}");
    assert!(detail.contains("1s"), "{detail}");
    assert!(detail.contains("run the command again"), "{detail}");
    assert!(!detail.contains("spice up"), "{detail}");
    assert_eq!(predecessor.kind, CommandKind::UpChange);
}

#[test]
fn a_second_refusal_from_the_successor_is_terminal() {
    // A successor that is itself ending answers the resubmitted envelope with
    // its own refusal; the client returns it as given instead of waiting for
    // a third resident.
    let fixture = Arc::new(Mutex::new(Fixture::new("restart-twice")));
    let predecessor_listener = TcpListener::bind("127.0.0.1:0").expect("predecessor peer binds");
    let successor_listener = TcpListener::bind("127.0.0.1:0").expect("successor peer binds");
    let predecessor_epoch = uuid(54);
    let successor_epoch = uuid(55);
    let canvas = fixture.lock().expect("fixture locks").canvas.clone();
    canvas::Discovery {
        endpoint: format!(
            "http://{}",
            predecessor_listener
                .local_addr()
                .expect("predecessor peer address")
        ),
        capability: uuid(56),
        epoch: predecessor_epoch,
    }
    .publish(&canvas)
    .expect("predecessor discovery publishes");
    fixture
        .lock()
        .expect("fixture locks")
        .hold(predecessor_epoch);
    let successor_discovery = canvas::Discovery {
        endpoint: format!(
            "http://{}",
            successor_listener
                .local_addr()
                .expect("successor peer address")
        ),
        capability: uuid(57),
        epoch: successor_epoch,
    };
    let peers = [
        (predecessor_listener, predecessor_epoch),
        (successor_listener, successor_epoch),
    ]
    .map(|(listener, epoch)| {
        std::thread::spawn(move || {
            let mut socket = accept_fixture_session(listener, epoch);
            let command = read_fixture_command(&mut socket);
            socket
                .send(Message::Binary(not_admitting(command.id, epoch).into()))
                .expect("refusal sends");
            command
        })
    });

    let mut transients = Vec::new();
    let outcome = execute_mapped_within(
        &canvas,
        &canvas,
        &COMMAND_CONTRACT,
        mapped(),
        Duration::from_secs(10),
        |fault| {
            transients.push(fault.reason.clone());
            let mut fixture = fixture.lock().expect("fixture locks");
            fs::remove_file(&canvas).expect("predecessor withdraws discovery");
            fixture.release_publisher();
            fixture.seat_successor();
            fixture.hold(successor_epoch);
            successor_discovery
                .publish(&fixture.canvas)
                .expect("successor discovery publishes");
        },
    );

    let [predecessor, successor] = peers.map(|peer| peer.join().expect("peer joins"));
    assert_eq!(predecessor.id, successor.id);
    assert_eq!(transients, ["resident::not-admitting"]);
    let Outcome::Refusal(refusal) = outcome else {
        panic!("the successor's refusal must be returned as given: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "resident::not-admitting");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::detail)
            .is_none(),
        "a refusal the successor gave carries no client wait account: {refusal:?}"
    );
}
