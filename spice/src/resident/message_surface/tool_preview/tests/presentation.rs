use std::sync::Mutex;

use super::super::super::*;

#[derive(Default)]
struct Messages(Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for Messages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        self.0.lock().expect("message lock").push(message);
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        let Some(existing) = messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}

fn lane() -> CanonicalUuid {
    "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("fixture lane")
}

fn event() -> PresentationEvent {
    PresentationEvent {
        lane: lane(),
        source: None,
        locus: listen::Locus::new(1, 0, 1),
        ordinal: 0,
    }
}

fn capture_record(bytes: &[u8], offset: u64) -> lane::capture::CaptureRecord {
    capture_record_for(listen::Dialect::Codex, bytes, offset)
}

fn capture_record_for(
    dialect: listen::Dialect,
    bytes: &[u8],
    offset: u64,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(1, offset, bytes.len() as u64);
    lane::capture::CaptureRecord {
        lane: lane(),
        occupant: lane(),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::decode(dialect, bytes, locus).expect("fixture transcript record decodes"),
    }
}

#[test]
fn completed_codex_tool_publishes_its_operation_before_the_outcome() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let bytes = br#"{"type":"item.completed","item":{"id":"item-command","type":"command_execution","command":"cargo test -p spice","aggregated_output":"ok","exit_code":0,"status":"completed"}}"#;
    let record = capture_record(bytes, 0);

    publish_agent_messages(&publisher, &record, &mut state, None)
        .expect("completed tool record publishes");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(
        messages.len(),
        2,
        "one record carries its operation and outcome"
    );
    assert_eq!(messages[0].text, "cargo test -p spice");
    assert_eq!(messages[1].text, "cargo test -p spice succeeded");
    assert!(
        messages
            .iter()
            .all(|message| message.activity == wire::AgentActivityKind::Tool),
    );
}

mod file_change;

#[test]
fn successful_codex_cli_turn_promotes_only_its_latest_agent_message() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"type":"item.completed","item":{"id":"commentary","type":"agent_message","text":"working through it"}}"#.as_slice(),
        br#"{"type":"item.completed","item":{"id":"final","type":"agent_message","text":"settled final"}}"#.as_slice(),
        br#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("CLI transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "working through it");
    assert!(!messages[0].final_answer);
    assert_eq!(messages[1].text, "settled final");
    assert!(messages[1].final_answer);
    assert_eq!(
        messages[1]
            .automatic_speech
            .as_ref()
            .map(|speech| speech.kind),
        Some(wire::AutomaticSpeechKind::Final),
    );
}

#[test]
fn failed_codex_cli_turn_leaves_its_agent_message_nonfinal() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"type":"item.completed","item":{"id":"failed","type":"agent_message","text":"not a final answer"}}"#.as_slice(),
        br#"{"type":"turn.failed","error":{"message":"model failed"}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("failed CLI transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert!(!messages[0].final_answer);
    assert!(messages[0].automatic_speech.is_none());
}

#[test]
fn persisted_codex_final_and_task_complete_companion_share_one_card() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"timestamp":"2030-04-12T18:44:09.270Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"message-1","content":[{"type":"Text","text":"settled final"}],"phase":"final_answer"}}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"settled final"}]}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.272Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"settled final"}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record(bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("persisted transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "settled final");
    assert!(messages[0].final_answer);
}

#[test]
fn claude_promotes_terminal_stop_reasons_but_not_tool_use() {
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let records = [
        br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"tool-bound prose"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"timestamp":"2030-04-12T18:44:09.272Z","type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"terminal prose"}],"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
        br#"{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}}"#.as_slice(),
    ];
    let mut offset = 0;
    for bytes in records {
        let record = capture_record_for(listen::Dialect::Claude, bytes, offset);
        offset += bytes.len() as u64;
        publish_agent_messages(&publisher, &record, &mut state, None)
            .expect("Claude transcript record publishes");
    }

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "tool-bound prose");
    assert!(!messages[0].final_answer);
    assert_eq!(messages[1].text, "terminal prose");
    assert!(messages[1].final_answer);
}

#[test]
fn terminal_assistant_prose_preserves_final_answer_state() {
    let publisher = Messages::default();
    let prose = listen::Event::Prose(listen::Prose {
        role: listen::ProseRole::Assistant,
        text: "settled final".to_owned(),
        phase: Some("final_answer".to_owned()),
        media: Vec::new(),
        locus: listen::Locus::new(1, 0, 1),
    });

    publish_agent_event(
        &publisher,
        AgentEvent {
            lane: lane(),
            author: lane(),
            source: None,
            worktree: None,
            ordinal: 0,
            decoded: &prose,
            authored: None,
            observed: None,
            observations: &MessageObservations::default(),
            unavailable_tool_terminals: &[],
        },
        &mut CallPreviews::default(),
        None,
    )
    .expect("final prose publishes");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "settled final");
    assert!(messages[0].final_answer);
}

fn directives(rows: &[(&str, &str)]) -> PresentationDirectives {
    PresentationDirectives(
        rows.iter()
            .map(|(key, text)| {
                (
                    key.parse::<steer::Key>().expect("fixture key parses"),
                    PresentationDirective {
                        lane: lane(),
                        text: (*text).to_owned(),
                        authored: true,
                        taste: false,
                        review: false,
                        disposition: wire::Disposition::Live,
                        retirement: None,
                    },
                )
            })
            .collect(),
    )
}

#[test]
fn steering_acknowledgement_renders_key_context_and_agent_prose_once() {
    let directives = directives(&[("1kQnrzNZ", "Keep one tested, visible current path.")]);
    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQnrzNZ: Landed in the stream presentation path.",
            &directives,
            event(),
        )
        .expect("acknowledgment presents"),
        "> Keep one tested, visible current path.\n\nLanded in the stream presentation path.",
    );
}

#[test]
fn current_header_forms_hide_framing_and_capitalize_response_prose() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    for text in [
        "ACK 1kQnrzNZ: applied.",
        "ACK 1kQnrzNZ. applied.",
        "ACK 1kQnrzNZ applied.",
        "**ACK 1kQnrzNZ:** applied.",
        "**ACK** **1kQnrzNZ**:** applied.",
    ] {
        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("acknowledgment presents"),
            "> Keep the current path.\n\nApplied.",
            "wrong presentation for {text:?}",
        );
    }
}

#[test]
fn message_badges_require_authenticated_positive_directive_evidence() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    assert_eq!(
        message_badges("ACK 1kQnrzNZ: applied.", Some(&directives), event(),),
        wire::AgentMessageBadges {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
        },
    );
    for text in [
        "NACK 1kQnrzNZ: unsafe.",
        "ACK not-a-key: prose only.",
        "Ordinary prose mentions ACK without a key.",
    ] {
        assert!(
            message_badges(text, Some(&directives), event()).is_empty(),
            "unearned badge for {text:?}",
        );
    }
}

#[test]
fn acknowledged_broadcast_marker_earns_its_own_semantic_badge() {
    let directives = directives(&[("1kQnrzNZ", "[BROADCAST] Send this to every occupied agent.")]);

    assert_eq!(
        message_badges("ACK 1kQnrzNZ: applied.", Some(&directives), event()),
        wire::AgentMessageBadges {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 1,
            reviews: 0,
        },
    );
    assert!(message_badges("NACK 1kQnrzNZ: refused.", Some(&directives), event()).is_empty());
}

#[test]
fn acknowledged_review_feedback_earns_a_source_bound_semantic_badge() {
    assert!(is_review_feedback(Some("review-feedback:1kKRRsDR:42")));
    assert!(!is_review_feedback(Some("task-contract-edit:1kKRRsDR:42")));
    let source = "00000000-0000-4000-8000-000000000002"
        .parse()
        .expect("fixture source");
    let key = "1kQnrzNZ"
        .parse::<steer::Key>()
        .expect("fixture key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: concat!(
                "[REVIEW] Peer feedback for CANVAS-1kKRRsDR:\n\n",
                "> preserve the finding\n\n",
                "Keep working the current task."
            )
            .to_owned(),
            authored: true,
            taste: false,
            review: true,
            disposition: wire::Disposition::Acked,
            retirement: Some(wire::body::TranscriptLocus {
                source: Some(source),
                member: 1,
                offset: 0,
                length: 1,
                event: 0,
                line: 1,
                column: 5,
            }),
        },
    )]));
    let text = "ACK 1kQnrzNZ: Internalized.";
    let mut original = event();
    original.source = Some(source);
    assert_eq!(
        message_badges(text, Some(&directives), original),
        wire::AgentMessageBadges {
            acknowledgements: 0,
            tastes: 0,
            broadcasts: 0,
            reviews: 1,
        },
    );
    let mut other_source = original;
    other_source.source = Some(
        "00000000-0000-4000-8000-000000000003"
            .parse()
            .expect("other fixture source"),
    );
    assert!(
        message_badges(text, Some(&directives), other_source).is_empty(),
        "the same prose in another tape generation earns no review classification"
    );
}

#[test]
fn distributed_acknowledgements_render_every_context_without_repeating_the_message() {
    let text = concat!(
        "ACK 1kQwPZtK: The current completion path is singular and visible.\n\n",
        "ACK 1kQwPZtL: Historical records remain evidence only."
    );
    let directives = directives(&[
        ("1kQwPZtK", "Keep one current completion path."),
        ("1kQwPZtL", "Do not execute historical records."),
    ]);

    let rendered = present_steering_acknowledgements_with_directives(text, &directives, event())
        .expect("distributed acknowledgments present");
    assert_eq!(rendered.matches("The current completion path").count(), 1);
    assert_eq!(rendered.matches("Historical records").count(), 1);
    assert_eq!(rendered.matches("ACK ").count(), 0);
    assert!(rendered.contains("> Keep one current completion path."));
    assert!(rendered.contains("> Do not execute historical records."));
}

#[test]
fn shared_acknowledgement_prose_renders_once_after_every_context() {
    let directives = directives(&[
        ("1kQwPZtK", "Keep one current completion path."),
        ("1kQwPZtL", "Do not execute historical records."),
    ]);

    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK 1kQwPZtL: One response covers both directions.",
            &directives,
            event(),
        )
        .expect("shared acknowledgment presents"),
        concat!(
            "> Keep one current completion path.\n\n",
            "> Do not execute historical records.\n\n",
            "One response covers both directions."
        ),
    );
}

#[test]
fn non_key_text_after_a_recognized_header_remains_response_prose() {
    let directives = directives(&[("1kQwPZtK", "Keep the recognized direction visible.")]);
    for marker in ["ACK", "NACK"] {
        let text = format!(
            "{marker} 1kQwPZtK nope 1kQwPZtM: Shared prose remains attached to the attempt."
        );
        let expected = "> Keep the recognized direction visible.\n\n\
                        Nope 1kQwPZtM: Shared prose remains attached to the attempt.";

        let rendered =
            present_steering_acknowledgements_with_directives(&text, &directives, event())
                .expect("mixed acknowledgment presents");
        assert_eq!(rendered, expected);
        assert_eq!(rendered.matches("1kQwPZtK").count(), 0);
        assert!(!rendered.contains(marker));
        assert_eq!(rendered.matches("Nope").count(), 1);
        assert_eq!(rendered.matches("1kQwPZtM").count(), 1);
        assert_eq!(rendered.matches("Shared prose").count(), 1);
    }
}

#[test]
fn mixed_acknowledgement_dispositions_keep_their_own_keys_and_contexts() {
    let directives = directives(&[
        ("1kQwPZtK", "Apply the current path."),
        ("1kQwPZtL", "Remove the safety check."),
    ]);
    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: Applied.\nNACK 1kQwPZtL: Unsafe.",
            &directives,
            event(),
        )
        .expect("mixed acknowledgments present"),
        "> Apply the current path.\n\nApplied.\n> Remove the safety check.\n\nUnsafe.",
    );
}

#[test]
fn inline_packets_render_each_context_and_body_in_source_order() {
    let directives = directives(&[
        ("1kQwPZtK", "Keep the first path."),
        ("1kQwPZtL", "Keep the second path."),
    ]);

    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: First applied. ACK 1kQwPZtL: Second applied.",
            &directives,
            event(),
        )
        .expect("inline acknowledgments present"),
        concat!(
            "> Keep the first path.\n\n",
            "First applied.\n",
            "> Keep the second path.\n\n",
            "Second applied."
        ),
    );
}

#[test]
fn operator_reply_quotes_remain_nested_inside_acknowledgement_context() {
    let directives = directives(&[(
        "1kQwPZtK",
        "> The original assertion remains visible.\n\napproved",
    )]);

    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: Applied.",
            &directives,
            event(),
        )
        .expect("nested acknowledgment presents"),
        "> > The original assertion remains visible.\n>\n> approved\n\nApplied.",
    );
}

#[test]
fn unknown_key_preserves_the_original_message_across_replay() {
    let text = "ACK 1kQwPZtK: No directive owns this key.";
    let directives = PresentationDirectives::default();
    let first = present_steering_acknowledgements_with_directives(text, &directives, event())
        .expect("unknown key remains visible");
    let replayed = present_steering_acknowledgements_with_directives(text, &directives, event())
        .expect("unknown key replay remains visible");

    assert_eq!(first, text);
    assert_eq!(replayed, first);
}

#[test]
fn task_contract_acknowledgment_hides_framing_and_earns_no_authored_context() {
    assert!(!presents_as_authored_steering(Some(
        "task-contract-edit:1kKRRsDP:44:33"
    )));
    let key = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current task-contract key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Accepted task contract revision.".to_owned(),
            authored: false,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            retirement: Some(wire::body::TranscriptLocus {
                source: None,
                member: 1,
                offset: 0,
                length: 1,
                event: 0,
                line: 1,
                column: 5,
            }),
        },
    )]));
    let text = "ACK dRyzkGX8b: Accepted revision 44.";
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("system acknowledgment presents"),
        "Accepted revision 44.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        wire::AgentMessageBadges {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
        },
    );
}

#[test]
fn task_contract_badge_reaches_canvas_without_speech_eligibility() {
    let key = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current task-contract key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Accepted task contract revision.".to_owned(),
            authored: false,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            retirement: Some(wire::body::TranscriptLocus {
                source: None,
                member: 1,
                offset: 0,
                length: 1,
                event: 0,
                line: 1,
                column: 5,
            }),
        },
    )]));
    let prose = listen::Event::Prose(listen::Prose {
        role: listen::ProseRole::Assistant,
        text: "ACK dRyzkGX8b: Accepted revision 44.".to_owned(),
        phase: None,
        media: Vec::new(),
        locus: event().locus,
    });
    let publisher = Messages::default();

    publish_agent_event(
        &publisher,
        AgentEvent {
            lane: lane(),
            author: lane(),
            source: None,
            worktree: None,
            ordinal: 0,
            decoded: &prose,
            authored: None,
            observed: None,
            observations: &MessageObservations::default(),
            unavailable_tool_terminals: &[],
        },
        &mut CallPreviews::default(),
        Some(&directives),
    )
    .expect("system acknowledgment publishes");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "Accepted revision 44.");
    assert_eq!(
        messages[0].badges,
        wire::AgentMessageBadges {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
        },
    );
    assert!(messages[0].automatic_speech.is_none());
}

#[test]
fn live_key_for_another_lane_preserves_the_original_message() {
    let text = "ACK 1kQwPZtK: This lane does not own the directive.";
    let mut directives = directives(&[("1kQwPZtK", "Do not quote this direction here.")]);
    directives
        .0
        .get_mut(&"1kQwPZtK".parse().expect("fixture key"))
        .expect("fixture directive")
        .lane = "00000000-0000-4000-8000-000000000002"
        .parse()
        .expect("other lane");

    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("cross-lane key remains visible"),
        text,
    );
}

#[test]
fn historical_key_is_presented_only_at_its_exact_retirement_locus() {
    let key = "1kQwPZtK"
        .parse::<steer::Key>()
        .expect("fixture key parses");
    let retirement = wire::body::TranscriptLocus {
        source: None,
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column: 5,
    };
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Keep the original response context.".to_owned(),
            authored: true,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            retirement: Some(retirement),
        },
    )]));
    let text = "ACK 1kQwPZtK: Applied.";

    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("retirement locus presents"),
        "> Keep the original response context.\n\nApplied.",
    );
    let mut later = event();
    later.locus = listen::Locus::new(1, 8, 1);
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, later)
            .expect("historical duplicate remains visible"),
        text,
    );
}

#[test]
fn mixed_current_and_historical_keys_present_every_known_context() {
    let historical = "1kR5x8yR"
        .parse::<steer::Key>()
        .expect("historical fixture key parses");
    let current = "1kR5xJDD"
        .parse::<steer::Key>()
        .expect("current fixture key parses");
    let retirement = |offset, column| wire::body::TranscriptLocus {
        source: None,
        member: 1,
        offset,
        length: 1,
        event: 0,
        line: 1,
        column,
    };
    let directives = PresentationDirectives(BTreeMap::from([
        (
            historical,
            PresentationDirective {
                lane: lane(),
                text: "Continue".to_owned(),
                authored: true,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(8, 5)),
            },
        ),
        (
            current,
            PresentationDirective {
                lane: lane(),
                text: "[TASTE] fallbacks: Keep one deterministic path.".to_owned(),
                authored: true,
                taste: true,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(0, 14)),
            },
        ),
    ]));
    let text = concat!(
        "ACK 1kR5x8yR 1kR5xJDD: Continuing with the explicit two-context contract ",
        "only."
    );

    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("mixed historical and current keys present"),
        concat!(
            "> Continue\n\n",
            "> [TASTE] fallbacks: Keep one deterministic path.\n\n",
            "Continuing with the explicit two-context contract only."
        ),
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        wire::AgentMessageBadges {
            acknowledgements: 1,
            tastes: 1,
            broadcasts: 0,
            reviews: 0,
        },
    );
}

#[test]
fn current_task_contract_and_taste_ack_hide_one_shared_protocol_header() {
    let system = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current task-contract key parses");
    let taste = "dRyzkQp1P"
        .parse::<steer::Key>()
        .expect("current taste key parses");
    let retirement = |column| wire::body::TranscriptLocus {
        source: None,
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column,
    };
    let directives = PresentationDirectives(BTreeMap::from([
        (
            system,
            PresentationDirective {
                lane: lane(),
                text: "Accepted task contract revision.".to_owned(),
                authored: false,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(5)),
            },
        ),
        (
            taste,
            PresentationDirective {
                lane: lane(),
                text: "[TASTE] compatibility: Keep one tested path.".to_owned(),
                authored: true,
                taste: true,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(15)),
            },
        ),
    ]));
    let text = "ACK dRyzkGX8b dRyzkQp1P: The current path is applied.";

    assert_eq!(
        message_badges(text, Some(&directives), event()),
        wire::AgentMessageBadges {
            acknowledgements: 2,
            tastes: 1,
            broadcasts: 0,
            reviews: 0,
        },
    );
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("mixed system and taste acknowledgment presents"),
        "> [TASTE] compatibility: Keep one tested path.\n\nThe current path is applied.",
    );
}

#[test]
fn inline_current_control_and_retained_direction_share_classification() {
    let system = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current task-contract key parses");
    let retained = "1kQwPZtK"
        .parse::<steer::Key>()
        .expect("retained direction key parses");
    let retirement = |column| wire::body::TranscriptLocus {
        source: None,
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column,
    };
    let directives = PresentationDirectives(BTreeMap::from([
        (
            system,
            PresentationDirective {
                lane: lane(),
                text: "Accepted task contract revision.".to_owned(),
                authored: false,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(13)),
            },
        ),
        (
            retained,
            PresentationDirective {
                lane: lane(),
                text: "Keep retained direction readable.".to_owned(),
                authored: true,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement(23)),
            },
        ),
    ]));
    let text = "Before. ACK dRyzkGX8b 1kQwPZtK: applied. After.";

    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("inline mixed-width acknowledgment presents"),
        "Before.\n> Keep retained direction readable.\n\nApplied. After.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        wire::AgentMessageBadges {
            acknowledgements: 2,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
        },
    );
}

#[test]
fn source_aware_retirement_requires_the_original_tape_generation() {
    let expected_source = "00000000-0000-4000-8000-000000000002"
        .parse()
        .expect("fixture source");
    let key = "1kQwPZtK"
        .parse::<steer::Key>()
        .expect("fixture key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Keep the original response context.".to_owned(),
            authored: true,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            retirement: Some(wire::body::TranscriptLocus {
                source: Some(expected_source),
                member: 1,
                offset: 0,
                length: 1,
                event: 0,
                line: 1,
                column: 5,
            }),
        },
    )]));
    let text = "ACK 1kQwPZtK: Applied.";

    let mut original = event();
    original.source = Some(expected_source);
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, original)
            .expect("original source presents"),
        "> Keep the original response context.\n\nApplied.",
    );

    let mut replacement = event();
    replacement.source = Some(
        "00000000-0000-4000-8000-000000000003"
            .parse()
            .expect("replacement source"),
    );
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, replacement)
            .expect("replacement source remains historical"),
        text,
    );
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("unknown source remains historical"),
        text,
    );
}
