use super::*;

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

/// Retire one fixture directive at the header locus `event()` reads.
fn retire(directives: &mut PresentationDirectives, key: &str) {
    let key = key.parse::<steer::Key>().expect("fixture key parses");
    let directive = directives.0.get_mut(&key).expect("fixture directive");
    directive.disposition = wire::Disposition::Acked;
    directive.retirement = Some(wire::body::TranscriptLocus {
        source: None,
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column: 5,
    });
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
        "> [ACK PENDING] Keep one tested, visible current path.\n\nLanded in the stream presentation path.",
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
            "> [ACK PENDING] Keep the current path.\n\nApplied.",
            "wrong presentation for {text:?}",
        );
    }
}

#[test]
fn presented_response_prose_normalizes_only_a_terminal_colon() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    for (body, presented) in [
        ("implementing:", "Implementing."),
        ("implementing:\n", "Implementing.\n"),
        ("implementing.", "Implementing."),
        ("implementing", "Implementing"),
    ] {
        let text = format!("ACK 1kQnrzNZ: {body}");
        assert_eq!(
            present_steering_acknowledgements_with_directives(&text, &directives, event())
                .expect("acknowledgment presents"),
            format!("> [ACK PENDING] Keep the current path.\n\n{presented}"),
            "wrong terminal punctuation for {body:?}",
        );
    }
}

#[test]
fn message_badges_require_authenticated_positive_directive_evidence() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    assert_eq!(
        message_badges("ACK 1kQnrzNZ: applied.", Some(&directives), event(),),
        ClassificationCounts {
            acknowledgements: 0,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
            pending: 1,
            declines: 0,
        },
    );
    assert_eq!(
        message_badges("NACK 1kQnrzNZ: unsafe.", Some(&directives), event()),
        ClassificationCounts {
            declines: 1,
            ..ClassificationCounts::default()
        },
        "a refusal is an answer, counted apart from credit",
    );
    for text in [
        "ACK not-a-key: prose only.",
        "NACK not-a-key: prose only.",
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
        ClassificationCounts {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 1,
            reviews: 0,
            pending: 0,
            declines: 0,
        },
    );
    assert!(message_badges("NACK 1kQnrzNZ: refused.", Some(&directives), event()).is_empty());
}

#[test]
fn acknowledged_review_feedback_earns_a_source_bound_semantic_badge() {
    assert!(is_review_feedback(Some("review-feedback:1kKRRsDR:42")));
    assert!(!is_review_feedback(Some("task-generation:1kKRRsDR:42:3")));
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
        ClassificationCounts {
            acknowledgements: 0,
            tastes: 0,
            broadcasts: 0,
            reviews: 1,
            pending: 0,
            declines: 0,
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
    assert_eq!(rendered.matches("ACK 1kQwPZt").count(), 0);
    assert_eq!(rendered.matches(wire::stream::ACK_MARKER).count(), 0);
    assert_eq!(
        rendered.matches(wire::stream::ACK_PENDING_MARKER).count(),
        2
    );
    assert!(rendered.contains("> [ACK PENDING] Keep one current completion path."));
    assert!(rendered.contains("> [ACK PENDING] Do not execute historical records."));
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
            "> [ACK PENDING] Keep one current completion path.\n\n",
            "> [ACK PENDING] Do not execute historical records.\n\n",
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
        let context = match marker {
            "ACK" => "> [ACK PENDING] Keep the recognized direction visible.",
            _ => "> [NACK] Keep the recognized direction visible.",
        };
        let expected =
            format!("{context}\n\nNope 1kQwPZtM: Shared prose remains attached to the attempt.");

        let rendered =
            present_steering_acknowledgements_with_directives(&text, &directives, event())
                .expect("mixed acknowledgment presents");
        assert_eq!(rendered, expected);
        assert_eq!(rendered.matches("1kQwPZtK").count(), 0);
        assert!(!rendered.starts_with(marker));
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
        concat!(
            "> [ACK PENDING] Apply the current path.\n\nApplied.\n",
            "> [NACK] Remove the safety check.\n\nUnsafe."
        ),
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
            "> [ACK PENDING] Keep the first path.\n\n",
            "First applied.\n",
            "> [ACK PENDING] Keep the second path.\n\n",
            "Second applied."
        ),
    );
}

#[test]
fn the_marker_lands_on_the_operator_words_below_a_quote() {
    let mut directives = directives(&[(
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
        "> > The original assertion remains visible.\n>\n> [ACK PENDING] approved\n\nApplied.",
    );
    retire(&mut directives, "1kQwPZtK");
    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: Applied.",
            &directives,
            event(),
        )
        .expect("retired nested acknowledgment presents"),
        "> > The original assertion remains visible.\n>\n> [ACK] approved\n\nApplied.",
    );
}

#[test]
fn trailing_quotation_is_passed_over_and_a_block_hunk_keeps_its_own_paragraph() {
    let directives = directives(&[(
        "1kQwPZtK",
        concat!(
            "> quoted context\n",
            "\n",
            "1. First step\n",
            "2. Second step\n",
            "\n",
            "> trailing quotation\n",
        ),
    )]);

    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: Applied.",
            &directives,
            event(),
        )
        .expect("acknowledgment presents"),
        concat!(
            "> > quoted context\n",
            ">\n",
            "> [ACK PENDING]\n",
            ">\n",
            "> 1. First step\n",
            "> 2. Second step\n",
            ">\n",
            "> > trailing quotation\n",
            ">\n\n",
            "Applied.",
        ),
    );
}

#[test]
fn a_directive_that_is_only_quotation_keeps_its_marker_above() {
    let mut quoted = directives(&[(
        "1kQwPZtK",
        "> [ACK] Tell me what the task is even about\n> \n> GIT-dSfJfCXfp is the admission-depth task.\n",
    )]);
    retire(&mut quoted, "1kQwPZtK");
    assert_eq!(
        present_steering_acknowledgements_with_directives(
            "ACK 1kQwPZtK: deleting GIT-dSfJfCXfp as directed.",
            &quoted,
            event(),
        )
        .expect("retired acknowledgment presents"),
        concat!(
            "> [ACK]\n>\n",
            "> > [ACK] Tell me what the task is even about\n",
            "> > \n",
            "> > GIT-dSfJfCXfp is the admission-depth task.\n",
            ">\n\n",
            "Deleting GIT-dSfJfCXfp as directed.",
        ),
    );
}

#[test]
fn a_quoted_card_never_moves_the_marker_off_the_directive() {
    let text = "ACK 1kQwPZtK: deleting GIT-dSfJfCXfp as directed.";
    let quoted_card = concat!(
        "> > [ACK] Tell me what the task is even about because I don't recognize it\n",
        "> \n",
        "> GIT-dSfJfCXfp is the task-git admission-depth task; summary follows after the ack.\n",
        "\n",
        "Delete it",
    );
    let mut quoted = directives(&[("1kQwPZtK", quoted_card)]);
    let presented = |marker: &str| {
        format!(
            concat!(
                "> > > [ACK] Tell me what the task is even about because I don't recognize it\n",
                "> > \n",
                "> > GIT-dSfJfCXfp is the task-git admission-depth task; summary follows after the ack.\n",
                ">\n",
                "> {} Delete it\n",
                "\n",
                "Deleting GIT-dSfJfCXfp as directed.",
            ),
            marker,
        )
    };

    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &quoted, event())
            .expect("pending acknowledgment presents"),
        presented(wire::stream::ACK_PENDING_MARKER),
        "pending credit marks the operator's own words, not the card they quoted",
    );
    retire(&mut quoted, "1kQwPZtK");
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &quoted, event())
            .expect("retired acknowledgment presents"),
        presented(wire::stream::ACK_MARKER),
        "the retired marker stays on the directive the operator wrote",
    );
    assert_eq!(
        message_badges(text, Some(&quoted), event()),
        ClassificationCounts {
            acknowledgements: 1,
            ..ClassificationCounts::default()
        },
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
fn system_acknowledgment_hides_framing_and_earns_no_authored_context() {
    assert!(!presents_as_authored_steering(Some(
        "task-generation:1kKRRsDP:33:4"
    )));
    let key = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current system key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Accepted lane renewal context.".to_owned(),
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
        ClassificationCounts {
            acknowledgements: 1,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
            pending: 0,
            declines: 0,
        },
    );
}

#[test]
fn system_acknowledgment_badge_reaches_canvas_without_speech_eligibility() {
    let key = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current system key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            lane: lane(),
            text: "Accepted lane renewal context.".to_owned(),
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
    assert!(
        messages[0].classifications.is_empty(),
        "a system acknowledgment has no authored context marker"
    );
    assert!(messages[0].automatic_speech.is_none());
}

#[test]
fn a_spoken_acknowledgement_reads_pending_until_the_journal_retires_its_key() {
    let key = "1kQnrzNZ"
        .parse::<steer::Key>()
        .expect("fixture key parses");
    let text = "ACK 1kQnrzNZ: Applied:";
    let mut directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("pending acknowledgment presents"),
        "> [ACK PENDING] Keep the current path.\n\nApplied.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        ClassificationCounts {
            pending: 1,
            ..ClassificationCounts::default()
        },
    );
    let locus = wire::MessageLocus {
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
    };
    let registered = pending::PendingPresentation::of(text, Some(&directives), event(), locus)
        .expect("a pending key registers its card");
    assert_eq!(registered.keys(), &BTreeSet::from([key]));
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let record = capture_record_for(
        listen::Dialect::Claude,
        br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"ACK 1kQnrzNZ: Applied:"}]}}"#,
        0,
    );
    publish_agent_messages(&publisher, &record, &mut state, Some(&directives))
        .expect("pending acknowledgment publishes");
    assert_eq!(
        state.pending.len(),
        1,
        "the published card waits on its key"
    );
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].text,
        "> [ACK PENDING] Keep the current path.\n\nApplied."
    );
    assert_eq!(
        messages[0]
            .classifications
            .iter()
            .map(|classification| classification.kind)
            .collect::<Vec<_>>(),
        [wire::MessageClassificationKind::AcknowledgementPending]
    );
    drop(messages);

    retire(&mut directives, "1kQnrzNZ");
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("retired acknowledgment presents"),
        "> [ACK] Keep the current path.\n\nApplied.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        ClassificationCounts {
            acknowledgements: 1,
            ..ClassificationCounts::default()
        },
    );
    assert!(
        pending::PendingPresentation::of(text, Some(&directives), event(), locus).is_none(),
        "a retired key leaves nothing to settle"
    );
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
fn repeated_acknowledgement_keeps_context_without_reearning_speech() {
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
        "> [ACK] Keep the original response context.\n\nApplied.",
    );
    let mut later = event();
    later.locus = listen::Locus::new(1, 8, 1);
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, later)
            .expect("repeated acknowledgement presents"),
        "> [ACK] Keep the original response context.\n\nApplied.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), later).acknowledgements,
        1,
    );
    assert!(
        automatic_speech::classify(
            &listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: None,
                media: Vec::new(),
                locus: later.locus,
            },
            Some(&directives),
            later,
        )
        .is_none(),
        "displaying the retired context does not authorize automatic speech",
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
            "> [ACK] Continue\n\n",
            "> [TASTE] fallbacks: Keep one deterministic path.\n\n",
            "Continuing with the explicit two-context contract only."
        ),
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        ClassificationCounts {
            acknowledgements: 2,
            tastes: 1,
            broadcasts: 0,
            reviews: 0,
            pending: 0,
            declines: 0,
        },
    );
}

#[test]
fn current_system_and_taste_ack_hide_one_shared_protocol_header() {
    let system = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current system key parses");
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
                text: "Accepted lane renewal context.".to_owned(),
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
        ClassificationCounts {
            acknowledgements: 2,
            tastes: 1,
            broadcasts: 0,
            reviews: 0,
            pending: 0,
            declines: 0,
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
        .expect("current system key parses");
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
                text: "Accepted lane renewal context.".to_owned(),
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
        "Before.\n> [ACK] Keep retained direction readable.\n\nApplied. After.",
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        ClassificationCounts {
            acknowledgements: 2,
            tastes: 0,
            broadcasts: 0,
            reviews: 0,
            pending: 0,
            declines: 0,
        },
    );
}

#[test]
fn acknowledged_context_survives_transcript_source_replacement() {
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
    let expected = "> [ACK] Keep the original response context.\n\nApplied.";

    let mut original = event();
    original.source = Some(expected_source);
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, original)
            .expect("original source presents"),
        expected,
    );

    let mut replacement = event();
    replacement.source = Some(
        "00000000-0000-4000-8000-000000000003"
            .parse()
            .expect("replacement source"),
    );
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, replacement)
            .expect("replacement source presents"),
        expected,
    );
    assert_eq!(
        message_badges(text, Some(&directives), replacement).acknowledgements,
        1,
    );
    assert!(
        automatic_speech::classify(
            &listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: None,
                media: Vec::new(),
                locus: replacement.locus,
            },
            Some(&directives),
            replacement,
        )
        .is_none(),
        "a later source may present context but cannot re-earn acknowledgement speech",
    );
    assert_eq!(
        present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("source-less history presents"),
        expected,
    );
}

#[test]
fn repeated_lane_seal_acknowledgements_render_without_fabricating_a_directive() {
    let seal = steer::Key::for_identity(lane());
    let text = format!("ACK {seal}: Continue.");
    let expected = "> [ACK]\n\nContinue.";
    let directives = PresentationDirectives::default();

    let first = event();
    let mut later = event();
    later.source = Some(
        "00000000-0000-4000-8000-000000000003"
            .parse()
            .expect("later transcript source"),
    );
    later.locus = listen::Locus::new(1, 8, 1);

    for acknowledged in [first, later] {
        assert_eq!(
            present_steering_acknowledgements_with_directives(&text, &directives, acknowledged,)
                .expect("lane seal acknowledgement presents"),
            expected,
        );
        assert_eq!(
            message_badges(&text, Some(&directives), acknowledged).acknowledgements,
            1,
        );
        assert!(
            automatic_speech::classify(
                &listen::Prose {
                    role: listen::ProseRole::Assistant,
                    text: text.clone(),
                    phase: None,
                    media: Vec::new(),
                    locus: acknowledged.locus,
                },
                Some(&directives),
                acknowledged,
            )
            .is_none(),
            "the lane seal is presentation identity, not retirement evidence",
        );
    }
}
