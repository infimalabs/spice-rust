use super::*;
#[cfg(unix)]
#[test]
fn live_changes_use_exact_requests_and_only_matching_provider_responses_apply() {
    let scratch = Scratch::new("live-retune");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    let reply = Path::new("/tmp").join(format!("spice-retune-test-{}.sock", std::process::id()));
    let _ = fs::remove_file(&reply);
    let socket = std::os::unix::net::UnixDatagram::bind(&reply).expect("callback door binds");
    socket
        .set_read_timeout(Some(ANSWER_BOUND))
        .expect("callback deadline sets");

    driver.retune(
        "live-effort".to_owned(),
        LiveRetune::Effort {
            effort: "xhigh".to_owned(),
        },
        reply.clone(),
    );
    assert_eq!(
        sink.answered(1),
        [serde_json::json!({
            "id": "live-effort",
            "type": "set_thinking_level",
            "level": "xhigh",
        })],
    );
    driver.observe(&decoded(
        br#"{"id":"another-request","type":"response","command":"set_thinking_level","success":true}"#,
    ));
    assert!(
        driver
            .retunes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key("live-effort"),
        "an unrelated response cannot settle the live change",
    );
    driver.observe(&decoded(
        br#"{"id":"live-effort","type":"response","command":"set_thinking_level","success":true}"#,
    ));
    let mut bytes = [0u8; RETUNE_REPLY_BYTES];
    let length = socket
        .recv(&mut bytes)
        .expect("the matching response answers");
    let answer: RetuneReply =
        serde_json::from_slice(&bytes[..length]).expect("the callback is typed");
    assert_eq!(answer.request, "live-effort");
    assert!(matches!(answer.result, RetuneReplyResult::Applied));

    driver.retune(
        "live-model".to_owned(),
        LiveRetune::Model {
            provider: "openai".to_owned(),
            model: "gpt-5.4".to_owned(),
        },
        reply.clone(),
    );
    assert_eq!(
        sink.answered(2)[1],
        serde_json::json!({
            "id": "live-model",
            "type": "set_model",
            "provider": "openai",
            "modelId": "gpt-5.4",
        }),
    );
    driver.observe(&decoded(
        br#"{"id":"live-model","type":"response","command":"set_thinking_level","success":true}"#,
    ));
    let length = socket
        .recv(&mut bytes)
        .expect("the mismatched command answers");
    let answer: RetuneReply =
        serde_json::from_slice(&bytes[..length]).expect("the refusal callback is typed");
    assert_eq!(answer.request, "live-model");
    assert!(matches!(answer.result, RetuneReplyResult::Refused { .. }));

    driver.retune(
        "live-model-success".to_owned(),
        LiveRetune::Model {
            provider: "openai".to_owned(),
            model: "gpt-5.4".to_owned(),
        },
        reply.clone(),
    );
    assert_eq!(
        sink.answered(3)[2],
        serde_json::json!({
            "id": "live-model-success",
            "type": "set_model",
            "provider": "openai",
            "modelId": "gpt-5.4",
        }),
    );
    driver.observe(&decoded(
        br#"{"id":"live-model-success","type":"response","command":"set_model","success":true}"#,
    ));
    let length = socket
        .recv(&mut bytes)
        .expect("the matching model response answers");
    let answer: RetuneReply =
        serde_json::from_slice(&bytes[..length]).expect("the model callback is typed");
    assert_eq!(answer.request, "live-model-success");
    assert!(matches!(answer.result, RetuneReplyResult::Applied));

    drop(socket);
    fs::remove_file(reply).expect("callback door cleans up");
}
