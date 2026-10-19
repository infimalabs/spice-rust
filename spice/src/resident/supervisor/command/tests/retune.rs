use super::*;
#[test]
fn live_changes_use_exact_requests_and_only_matching_provider_responses_apply() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let reply = door_directory().join(format!("spice-retune-test-{}.sock", std::process::id()));
    let callback = door::Door::bind(&reply, CALLBACK_REPLY_BYTES).expect("callback door binds");

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
    let bytes = callback
        .recv(Some(ANSWER_BOUND))
        .expect("the matching response answers");
    let answer: RetuneReply = serde_json::from_slice(&bytes).expect("the callback is typed");
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
    let bytes = callback
        .recv(Some(ANSWER_BOUND))
        .expect("the mismatched command answers");
    let answer: RetuneReply =
        serde_json::from_slice(&bytes).expect("the refusal callback is typed");
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
    let bytes = callback
        .recv(Some(ANSWER_BOUND))
        .expect("the matching model response answers");
    let answer: RetuneReply = serde_json::from_slice(&bytes).expect("the model callback is typed");
    assert_eq!(answer.request, "live-model-success");
    assert!(matches!(answer.result, RetuneReplyResult::Applied));
}

#[test]
fn fast_mode_requests_name_the_disposition_and_a_failed_response_refuses() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let reply = door_directory().join(format!("spice-fast-test-{}.sock", std::process::id()));
    let callback = door::Door::bind(&reply, CALLBACK_REPLY_BYTES).expect("callback door binds");

    driver.retune(
        "live-fast".to_owned(),
        LiveRetune::Fast { enabled: true },
        reply.clone(),
    );
    assert_eq!(
        sink.answered(1)[0],
        serde_json::json!({
            "id": "live-fast",
            "type": "set_fast_mode",
            "enabled": true,
        }),
    );
    // OMP refuses to enable fast mode on a model without a priority tier.
    driver.observe(&decoded(
        br#"{"id":"live-fast","type":"response","command":"set_fast_mode","success":false,"error":"Fast mode is unavailable for the current model."}"#,
    ));
    let bytes = callback
        .recv(Some(ANSWER_BOUND))
        .expect("the failed fast-mode response answers");
    let answer: RetuneReply = serde_json::from_slice(&bytes).expect("the fast callback is typed");
    assert_eq!(answer.request, "live-fast");
    assert!(matches!(answer.result, RetuneReplyResult::Refused { .. }));
}
