use super::*;

fn message_offsets(messages: &[wire::AgentMessage]) -> Vec<u64> {
    messages
        .iter()
        .map(|message| message.locus.offset)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_socket_pages_member_history_before_the_initial_window() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "message history horizon", false);
    let capabilities = Capabilities::default();
    capabilities
        .insert(operator(), wire::Actor::Operator)
        .expect("operator capability");
    let (server, _control) = Server::bind_streaming(
        Binding::new(LOOPBACK_EPHEMERAL, Access::local()),
        generation(&app, capabilities),
        bus.reader(),
        8,
    )
    .await
    .expect("streaming server binds");
    let publisher = server
        .message_publisher()
        .expect("streaming server publishes messages");
    for offset in 0..80 {
        assert!(
            publisher
                .publish(agent_message(0xe6, offset, &format!("message {offset}")))
                .expect("message publishes")
        );
    }
    let address = server.local_addr().expect("address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    send_client(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: identity(0xd0),
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Messages],
            after: None,
        },
    )
    .await;
    let initial = stream_frame(&mut socket).await;
    let wire::StreamFrame::Messages { messages, more, .. } = initial else {
        panic!("expected bounded message snapshot")
    };
    assert_eq!(more, [identity(0xe5)]);
    assert_eq!(message_offsets(&messages), (55..80).collect::<Vec<_>>());

    let mut before = messages[0].locus;
    let mut recovered = messages;
    for (request, expected, has_more) in
        [(identity(0xd1), 5..55, true), (identity(0xd2), 0..5, false)]
    {
        send_client(
            &mut socket,
            &wire::StreamRequest::MessageHistory {
                v: wire::WireVersion::CURRENT,
                id: request,
                epoch: epoch(),
                lane: identity(0xe5),
                before,
            },
        )
        .await;
        let page = stream_frame(&mut socket).await;
        let wire::StreamFrame::MessageHistory {
            id,
            lane,
            before: answered_before,
            more,
            messages,
            ..
        } = page
        else {
            panic!("expected member history page")
        };
        assert_eq!(id, request);
        assert_eq!(lane, identity(0xe5));
        assert_eq!(answered_before, before);
        assert_eq!(more, has_more);
        assert_eq!(message_offsets(&messages), expected.collect::<Vec<_>>());
        before = messages[0].locus;
        recovered.splice(0..0, messages);
    }
    assert_eq!(
        message_offsets(&recovered),
        (0..80).collect::<Vec<_>>(),
        "history pages reconstruct the retained lane in stable order",
    );

    shutdown.send(()).expect("shutdown signals");
    assert!(matches!(
        stream_frame(&mut socket).await,
        wire::StreamFrame::Closing { .. }
    ));
    assert_socket_terminated(&mut socket).await;
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    bus.shutdown().expect("bus shuts down");
}
