use super::*;

pub(super) fn command_client(address: SocketAddr) {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::http::header::ORIGIN;

    let stream = std::net::TcpStream::connect(address).expect("connects");
    let mut request = format!("ws://{address}/socket")
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert(ORIGIN, format!("http://{address}").parse().expect("origin"));
    let (mut socket, _) = tungstenite::client(request, stream).expect("opens");
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: operator(),
        epoch: epoch(),
    };
    socket
        .send(tungstenite::Message::Binary(
            wire::encode(&hello).expect("hello encodes").into(),
        ))
        .expect("hello sends");
    let opening = socket.read().expect("acceptance").into_data();
    let accepted: wire::Accepted = wire::decode(&opening).expect("accepted frame");
    assert_eq!(accepted.actor, wire::Actor::Operator);

    for (id, to, succeeds) in command_cases() {
        let command = wire::Command {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: epoch(),
            kind: wire::CommandKind::UpChange,
            expected: vec![wire::Witness::Transition {
                from: wire::ResidentState::Seated,
                to,
            }],
            body: wire::intent::UpChange { state: to },
        };
        socket
            .send(tungstenite::Message::Binary(
                wire::encode(&command).expect("command encodes").into(),
            ))
            .expect("command sends");
        let frame = socket.read().expect("response").into_data();
        let response: wire::Response<serde_json::Value> =
            wire::decode(&frame).expect("response decodes");
        assert_eq!((response.id, response.epoch), (id, epoch()));
        assert_eq!(
            matches!(response.outcome, wire::Outcome::Reply(_)),
            succeeds
        );
    }
}

fn command_cases() -> [(wire::CanonicalUuid, wire::ResidentState, bool); 3] {
    [
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
            wire::ResidentState::Dark,
            false,
        ),
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c7"),
            wire::ResidentState::Seated,
            true,
        ),
        (
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c8"),
            wire::ResidentState::Dark,
            true,
        ),
    ]
}

pub(super) async fn answer_commands(control: &mut Control) {
    for (index, (_, to, _)) in command_cases().into_iter().enumerate() {
        let request = control.recv().await.expect("resident request");
        assert_eq!(request.command().actor(), &wire::Actor::Operator);
        assert!(matches!(
            request.command().body(),
            wire::intent::Intent::UpChange(wire::intent::UpChange { state }) if *state == to
        ));
        let outcome = if index == 0 {
            transition_refusal()
        } else {
            state_reply(to)
        };
        request.respond(outcome).expect("client waits for answer");
    }
}

fn transition_refusal() -> wire::Outcome<wire::intent::CommandAnswer> {
    wire::Outcome::Refusal(wire::Refusal {
        reason: "resident::transition-moved".to_owned(),
        witness: Some("transition".to_owned()),
        expected: Some("seated->dark".to_owned()),
        observed: Some("quiescing->dark".to_owned()),
        remedy: "refresh the transition".to_owned(),
    })
}

pub(super) fn state_reply(to: wire::ResidentState) -> wire::Outcome<wire::intent::CommandAnswer> {
    wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::intent::CommandAnswer::State(wire::intent::StateAnswer {
            from: wire::ResidentState::Seated,
            to,
        }),
    })
}

#[test]
fn duplicate_capability_refuses_without_replacing_its_principal() {
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("first registration");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c5");

    assert_eq!(
        capabilities.insert(operator(), wire::Actor::Lane(lane)),
        Err(CapabilityError::Duplicate)
    );
    let accepted = capabilities
        .admit(
            &wire::Hello {
                wire: wire::WireVersion::CURRENT,
                capability: operator(),
                epoch: epoch(),
            },
            epoch(),
        )
        .expect("original registration survives");
    assert_eq!(accepted.actor, wire::Actor::Operator);
}

#[test]
fn unknown_capability_refuses_without_echoing_the_credential() {
    let capabilities = Capabilities::default();
    let unknown = operator();
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: unknown,
        epoch: epoch(),
    };

    let refusal = capabilities
        .admit(&hello, epoch())
        .expect_err("unknown capability refuses");

    assert!(matches!(
        refusal,
        OpeningError::Refused(wire::Refusal { ref reason, .. })
            if reason == "hello::capability-unknown"
    ));
    assert!(!format!("{refusal:?}").contains(&unknown.to_string()));
}

#[test]
fn host_and_origin_must_name_the_published_endpoint() {
    let state = SurfaceState {
        authority: Authority::Only("127.0.0.1:48123".to_owned()),
        access: Access::local(),
        origin: "http://127.0.0.1:48123".to_owned(),
        epoch: epoch(),
        capabilities: Capabilities::default(),
        app: embedded(),
        speech: speech(),
        speech_slot: Semaphore::new(1),
        admission: Mutex::new(Admission::Open),
        commands: mpsc::channel(1).0,
        stream: None,
        closed: watch::channel(false).0,
        connections: CancellationToken::new(),
        sessions: AtomicUsize::new(0),
        sessions_closed: Notify::new(),
        admission_observed: watch::channel(None).0,
    };
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("attacker.example"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://attacker.example"));
    assert!(admit_headers(&state, &headers, true).is_err());

    headers.insert(HOST, HeaderValue::from_static("127.0.0.1:48123"));
    headers.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:48123"));
    assert_eq!(admit_headers(&state, &headers, true), Ok("127.0.0.1:48123"));

    // A specific bind admits its own name alone, so a second address of this
    // machine is refused even though the surface is listening on this port.
    headers.insert(HOST, HeaderValue::from_static("10.0.0.5:48123"));
    headers.insert(ORIGIN, HeaderValue::from_static("http://10.0.0.5:48123"));
    assert!(admit_headers(&state, &headers, true).is_err());
}
