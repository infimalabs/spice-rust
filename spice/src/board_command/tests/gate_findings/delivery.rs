use super::*;

const OLD_CANVAS_FRAME_BYTES: usize = 16 * 1_024 * 1_024;

#[tokio::test(flavor = "current_thread")]
async fn oversized_focused_evidence_survives_canvas_response_failure() {
    let (root, bus, epoch, _) = fixture("gate-findings-oversized-frame", 1);
    let mut dispatch = focused_dispatch(&root, "oversized-frame", &["oversized-frame"]);
    enlarge_focused_evidence(&mut dispatch.body);
    let command_bytes = gate_findings_command_bytes(epoch, &dispatch.body);
    assert!(
        command_bytes.len() > OLD_CANVAS_FRAME_BYTES,
        "fixture must exceed the former Canvas frame ceiling"
    );

    stage_canvas_app(&root);
    let capability = gate_findings_uuid(72);
    let capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(capability, Actor::Operator)
        .expect("operator capability registers");
    let app = canvas::AppSource::resolve(&root).expect("fixture Canvas app resolves");
    let version = crate::resident::canvas_version(&app, journal::SCHEMA_VERSION)
        .expect("Canvas identity builds");
    let (server, mut control) = canvas::axum::Server::bind(canvas::axum::Generation::new(
        epoch,
        capabilities,
        version,
        app,
        canvas::SpeechConfig::default(),
    ))
    .await
    .expect("Canvas server binds");
    let address = server.local_addr().expect("Canvas server address");
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (break_delivery, broken_delivery) = std::sync::mpsc::channel();
    let client = tokio::task::spawn_blocking(move || {
        send_gate_findings_frame(address, capability, epoch, command_bytes, broken_delivery)
    });

    let request = control
        .recv()
        .await
        .expect("oversized gate finding reaches resident control");
    let outcome = gate_findings_folded_in_process(
        &bus,
        request.command(),
        &dispatch.body,
        &dispatch.findings,
        gate::GateId::FileShape,
        false,
        &dispatch.repository,
        &gate_projects(),
        &board::NextConfig::default(),
    );
    break_delivery
        .send(())
        .expect("client waits to break Canvas delivery");
    client.await.expect("gate finding client joins");
    shutdown.send(()).expect("Canvas shutdown signals");
    server_task
        .await
        .expect("Canvas server task joins")
        .expect("Canvas server stops");
    assert!(
        request.respond(outcome).is_err(),
        "Canvas response delivery must be unavailable after dispatch"
    );

    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].presentation_title, dispatch.findings[0].title);
    let (journaled, answer) = recover_gate_finding_receipt(&root, gate_findings_uuid(73));
    assert_eq!(journaled.report, dispatch.body.report);
    assert_eq!(journaled.evidence, dispatch.body.evidence);
    assert!(
        text(answer).contains("findings=1 created=1"),
        "journal retains the command result after Canvas delivery fails"
    );
    clean(&root, bus);
}

fn enlarge_focused_evidence(body: &mut GateFindings) {
    body.evidence = "x".repeat(OLD_CANVAS_FRAME_BYTES + 1);
    body.report = Digest::compute(Domain::GateFocusedReport, body.evidence.as_bytes());
}

fn gate_findings_command_bytes(epoch: CanonicalUuid, body: &GateFindings) -> Vec<u8> {
    wire::encode(&wire::Command {
        v: wire::WireVersion::CURRENT,
        id: gate_findings_uuid(73),
        epoch,
        kind: CommandKind::GateFindings,
        expected: Vec::new(),
        body: body.clone(),
    })
    .expect("oversized gate findings command encodes")
}

fn send_gate_findings_frame(
    address: std::net::SocketAddr,
    capability: CanonicalUuid,
    epoch: CanonicalUuid,
    command: Vec<u8>,
    break_delivery: std::sync::mpsc::Receiver<()>,
) {
    use tungstenite::client::IntoClientRequest as _;

    let stream = std::net::TcpStream::connect(address).expect("gate finding client connects");
    let mut request = format!("ws://{address}/socket")
        .into_client_request()
        .expect("gate finding socket request");
    request.headers_mut().insert(
        tungstenite::http::header::ORIGIN,
        format!("http://{address}")
            .parse()
            .expect("gate finding origin"),
    );
    let (mut socket, _) = tungstenite::client(request, stream).expect("gate finding socket opens");
    socket
        .send(tungstenite::Message::Binary(
            wire::encode(&wire::Hello {
                wire: wire::WireVersion::CURRENT,
                capability,
                epoch,
            })
            .expect("gate finding hello encodes")
            .into(),
        ))
        .expect("gate finding hello sends");
    let accepted: wire::Accepted = wire::decode(&socket.read().expect("acceptance").into_data())
        .expect("gate finding acceptance decodes");
    assert_eq!((accepted.actor, accepted.epoch), (Actor::Operator, epoch));
    socket
        .send(tungstenite::Message::Binary(command.into()))
        .expect("oversized gate finding frame sends");
    break_delivery
        .recv()
        .expect("test releases the broken delivery");
    socket
        .get_mut()
        .shutdown(std::net::Shutdown::Both)
        .expect("gate finding socket disconnects before its response");
}

fn recover_gate_finding_receipt(
    root: &Path,
    command: CanonicalUuid,
) -> (GateFindings, CommandAnswer) {
    let connection = rusqlite::Connection::open_with_flags(
        root.join("authority.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("journal receipt opens for read-only recovery");
    let (request, response): (Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT request, response FROM command_receipts WHERE command_id = ?1",
            [command.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("gate finding command receipt remains journaled");
    let request: wire::Command<GateFindings> =
        wire::decode(&request).expect("journaled gate finding request decodes");
    assert_eq!(request.id, command);
    let response: wire::Response<serde_json::Value> =
        wire::decode(&response).expect("journaled gate finding response decodes");
    assert_eq!(response.id, command);
    let Outcome::Reply(reply) = response.outcome else {
        panic!(
            "journaled gate finding did not commit: {:?}",
            response.outcome
        );
    };
    let body = wire::encode(&reply.body).expect("gate finding response body re-encodes");
    let answer = CommandKind::GateFindings
        .binding()
        .answer
        .decode(&body)
        .expect("journaled response follows its answer binding");
    (request.body, answer)
}

fn stage_canvas_app(root: &Path) {
    const GENERATION: &str = "7777777777777777777777777777777777777777777777777777777777777777";

    let stage = root.join(canvas::APP_STAGE);
    let generation = stage.join(canvas::APP_STAGE_GENERATIONS).join(GENERATION);
    fs::create_dir_all(&generation).expect("gate finding Canvas generation");
    for asset in canvas::APP.assets() {
        fs::write(generation.join(asset.file()), b"gate finding fixture")
            .expect("gate finding Canvas asset");
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{GENERATION}\n"),
    )
    .expect("gate finding Canvas pointer");
}

fn gate_findings_uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
        .parse()
        .expect("canonical gate finding UUID")
}
