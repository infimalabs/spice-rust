use super::*;
use crate::stream::{MessageHistoryPage, MessageHistoryProvider, MessagePublisher};

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
    assert_eq!(
        more,
        [wire::MessageHistoryCursor {
            lane: identity(0xe5),
            before: Some(messages[0].locus),
        }]
    );
    assert_eq!(message_offsets(&messages), (55..80).collect::<Vec<_>>());

    let mut before = Some(messages[0].locus);
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
        before = Some(messages[0].locus);
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

/// A history provider the test holds shut until it releases it.
///
/// `history` reports that it was entered and then blocks its thread on the
/// latch. On the surface runtime thread that would freeze every socket, the
/// test included, so a read arriving on the thread that built the provider
/// refuses instead and drops the entry report, which fails the test at once
/// rather than hanging it. On the blocking pool only the one request that
/// asked stands still.
#[derive(Debug)]
struct LatchedHistory {
    runtime: std::thread::ThreadId,
    entered: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    released: std::sync::Mutex<bool>,
    release: std::sync::Condvar,
}

impl LatchedHistory {
    fn new() -> (Arc<Self>, oneshot::Receiver<()>) {
        let (entered, observed) = oneshot::channel();
        let provider = Arc::new(Self {
            runtime: std::thread::current().id(),
            entered: std::sync::Mutex::new(Some(entered)),
            released: std::sync::Mutex::new(false),
            release: std::sync::Condvar::new(),
        });
        (provider, observed)
    }

    fn release(&self) {
        *self.released.lock().expect("latch lock") = true;
        self.release.notify_all();
    }
}

impl MessageHistoryProvider for LatchedHistory {
    fn history(
        &self,
        _lane: wire::CanonicalUuid,
        _before: Option<&wire::AgentMessage>,
        _limit: usize,
    ) -> Result<Option<MessageHistoryPage>, String> {
        let entered = self.entered.lock().expect("entered lock").take();
        if std::thread::current().id() == self.runtime {
            drop(entered);
            return Err("history ran on the surface runtime thread".to_owned());
        }
        if let Some(entered) = entered {
            let _ = entered.send(());
        }
        let mut released = self.released.lock().expect("latch lock");
        while !*released {
            released = self.release.wait(released).expect("latch condvar");
        }
        Ok(Some(MessageHistoryPage {
            messages: vec![agent_message(0xe6, 5, "held page")],
            more: false,
        }))
    }
}

/// Authenticate one socket and subscribe it to the message view.
///
/// Returns the socket and the history cursors its initial snapshot carried.
async fn subscribe_messages(
    address: SocketAddr,
    id: wire::CanonicalUuid,
) -> (ClientSocket, Vec<wire::MessageHistoryCursor>) {
    let (mut socket, _) = authenticate(address, operator(), epoch()).await;
    send_client(
        &mut socket,
        &wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: epoch(),
            subjects: vec![wire::ViewSubject::Messages],
            after: None,
        },
    )
    .await;
    let wire::StreamFrame::Messages { more, .. } = stream_frame(&mut socket).await else {
        panic!("expected bounded message snapshot")
    };
    (socket, more)
}

/// Ask for the newest history page of the open lane.
async fn request_history(socket: &mut ClientSocket, id: wire::CanonicalUuid) {
    send_client(
        socket,
        &wire::StreamRequest::MessageHistory {
            v: wire::WireVersion::CURRENT,
            id,
            epoch: epoch(),
            lane: identity(0xe5),
            before: None,
        },
    )
    .await;
}

/// A socket is answered in full while another socket waits on its read.
///
/// The watcher is pinged and then handed a card published during the hold.
async fn assert_served_during_hold(
    watcher: &mut ClientSocket,
    watch: wire::CanonicalUuid,
    publisher: &MessagePublisher,
    live: &wire::AgentMessage,
) {
    send_client(
        watcher,
        &wire::StreamRequest::Ping {
            v: wire::WireVersion::CURRENT,
            id: identity(0xd3),
            epoch: epoch(),
        },
    )
    .await;
    assert!(matches!(
        stream_frame(watcher).await,
        wire::StreamFrame::Pong { id, .. } if id == identity(0xd3)
    ));
    assert!(
        publisher
            .publish(live.clone())
            .expect("live message publishes")
    );
    assert!(matches!(
        stream_frame(watcher).await,
        wire::StreamFrame::Message { id, message, .. }
            if id == watch && message == *live
    ));
}

/// The held page reaches the reader unchanged, then the card published meanwhile.
async fn assert_held_page_answered(reader: &mut ClientSocket, live: &wire::AgentMessage) {
    let wire::StreamFrame::MessageHistory {
        id,
        lane,
        before,
        more,
        messages,
        ..
    } = stream_frame(reader).await
    else {
        panic!("expected the held history page")
    };
    assert_eq!(id, identity(0xd1));
    assert_eq!(lane, identity(0xe5));
    assert_eq!(before, None);
    assert!(!more);
    assert_eq!(message_offsets(&messages), [5]);
    assert!(
        matches!(
            stream_frame(reader).await,
            wire::StreamFrame::Message { id, message, .. }
                if id == identity(0xd0) && message == *live
        ),
        "the live card published during the read follows the answered page",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn held_history_read_leaves_the_runtime_serving_other_sockets() {
    let app = TestApp::new();
    let bus = bus::Bus::start(journal::Journal::in_memory().expect("authority opens"))
        .expect("bus starts");
    commit_stream(&bus, "held history horizon", false);
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
    let (provider, entered) = LatchedHistory::new();
    publisher
        .set_history_provider(provider.clone())
        .expect("latched provider installs");
    publisher
        .set_history_open([identity(0xe5)])
        .expect("lane opens durable history");
    let address = server.local_addr().expect("address");
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));

    // The reader asks for the lane bootstrap page and its provider is held.
    let (mut reader, more) = subscribe_messages(address, identity(0xd0)).await;
    assert_eq!(
        more,
        [wire::MessageHistoryCursor {
            lane: identity(0xe5),
            before: None,
        }]
    );
    request_history(&mut reader, identity(0xd1)).await;
    timeout(ASSERTION_DEADLINE, entered)
        .await
        .expect("provider is entered before the deadline")
        .expect("provider reports its entry");

    // A second socket is served in full while the first read is still held.
    let (mut watcher, _) = subscribe_messages(address, identity(0xd2)).await;
    let live = agent_message(0xe7, 90, "published while the read is held");
    assert_served_during_hold(&mut watcher, identity(0xd2), &publisher, &live).await;

    // Releasing the latch lets the held page reach the reader unchanged.
    provider.release();
    assert_held_page_answered(&mut reader, &live).await;

    shutdown.send(()).expect("shutdown signals");
    for socket in [&mut reader, &mut watcher] {
        assert!(matches!(
            stream_frame(socket).await,
            wire::StreamFrame::Closing { .. }
        ));
        assert_socket_terminated(socket).await;
    }
    server_task
        .await
        .expect("server task joins")
        .expect("server stops");
    bus.shutdown().expect("bus shuts down");
}
