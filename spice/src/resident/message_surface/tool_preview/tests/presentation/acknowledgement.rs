use super::*;

fn quote_target(key: &str, text: &str) -> wire::QuoteTarget {
    wire::QuoteTarget::Steering {
        key: key.to_owned(),
        digest: wire::QuoteTarget::text_digest(text),
    }
}

fn published(millis: u16) -> wire::Instant {
    format!("2030-04-12T18:44:09.{millis:03}Z")
        .parse()
        .expect("fixture publication instant")
}

fn directives(rows: &[(&str, &str)]) -> PresentationDirectives {
    PresentationDirectives(
        rows.iter()
            .map(|(key, text)| {
                (
                    key.parse::<steer::Key>().expect("fixture key parses"),
                    PresentationDirective {
                        actor: wire::Actor::Operator,
                        published: published(1),
                        lane: lane(),
                        text: (*text).to_owned(),
                        authored: true,
                        taste: false,
                        review: false,
                        disposition: wire::Disposition::Live,
                        quote_target: quote_target(key, text),
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

fn presented_text(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<String, String> {
    present_steering_acknowledgements_with_directives(text, directives, event)
        .map(|presented| presented.text)
}

fn presented_kinds(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Vec<wire::MessageClassificationKind> {
    present_steering_acknowledgements(text, Some(directives), event)
        .expect("message presents")
        .classifications
        .into_iter()
        .map(|classification| classification.kind)
        .collect()
}

#[test]
fn steering_acknowledgement_renders_key_context_and_agent_prose_once() {
    let directives = directives(&[("1kQnrzNZ", "Keep one tested, visible current path.")]);
    assert_eq!(
        presented_text(
            "ACK 1kQnrzNZ: Landed in the stream presentation path.",
            &directives,
            event(),
        )
        .expect("acknowledgment presents"),
        "> Keep one tested, visible current path.\n\n[ACK PENDING] Landed in the stream presentation path.",
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
            presented_text(text, &directives, event()).expect("acknowledgment presents"),
            "> Keep the current path.\n\n[ACK PENDING] Applied.",
            "wrong presentation for {text:?}",
        );
    }
}

#[test]
fn decorative_brackets_and_unicode_dashes_do_not_frame_presented_context() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    for text in [
        "[ACK 1kQnrzNZ] applied.",
        "[ACK 1kQnrzNZ] – applied.",
        "[ACK 1kQnrzNZ] — applied.",
        "— [ACK 1kQnrzNZ] — applied.",
    ] {
        assert_eq!(
            presented_text(text, &directives, event()).expect("acknowledgment presents"),
            "> Keep the current path.\n\n[ACK PENDING] Applied.",
            "wrong presentation for {text:?}",
        );
    }
    assert_eq!(
        presented_text(
            "Context [kept] — ACK 1kQnrzNZ: applied.",
            &directives,
            event(),
        )
        .expect("acknowledgment after ordinary prose presents"),
        concat!(
            "Context [kept] —\n",
            "> Keep the current path.\n\n",
            "[ACK PENDING] Applied."
        ),
    );
}

#[test]
fn framing_trim_preserves_response_punctuation_and_structural_ranges() {
    let directives = directives(&[
        ("1kQwPZtK", "Apply the current path."),
        ("1kQwPZtL", "Keep the safety check."),
    ]);
    let presented = present_steering_acknowledgements(
        concat!(
            "[ACK 1kQwPZtK] — applied [within scope] – keep — these.\n",
            "[NAK 1kQwPZtL] – unsafe [without review] — retained."
        ),
        Some(&directives),
        event(),
    )
    .expect("framed acknowledgments present");

    assert_eq!(
        presented.text,
        concat!(
            "> Apply the current path.\n\n",
            "[ACK PENDING] Applied [within scope] – keep — these.\n",
            "> Keep the safety check.\n\n",
            "[NAK] Unsafe [without review] — retained."
        ),
    );
    assert!(
        presented
            .classifications
            .iter()
            .all(|classification| classification.is_valid_for(&presented.text))
    );
    assert!(wire::valid_message_directives(
        &presented.text,
        &presented.directive_contexts
    ));
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
            presented_text(&text, &directives, event()).expect("acknowledgment presents"),
            format!("> Keep the current path.\n\n[ACK PENDING] {presented}"),
            "wrong terminal punctuation for {body:?}",
        );
    }
}

#[test]
fn answer_markers_require_authenticated_directive_evidence() {
    use wire::MessageClassificationKind as Kind;
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    let kinds = |text: &str| presented_kinds(text, &directives, event());
    assert_eq!(
        kinds("ACK 1kQnrzNZ: applied."),
        [Kind::AcknowledgementPending]
    );
    assert_eq!(
        kinds("NAK 1kQnrzNZ: unsafe."),
        [Kind::Decline],
        "a refusal is an answer, marked apart from credit",
    );
    for text in [
        "ACK not-a-key: prose only.",
        "NAK not-a-key: prose only.",
        "Ordinary prose mentions ACK without a key.",
        "Agent prose writes [ACK] itself.",
    ] {
        assert!(kinds(text).is_empty(), "unearned marker for {text:?}");
    }
}

#[test]
fn acknowledged_broadcast_marker_earns_its_own_semantic_badge() {
    let directives = directives(&[("1kQnrzNZ", "[BROADCAST] Send this to every occupied agent.")]);

    assert_eq!(
        message_badges("ACK 1kQnrzNZ: applied.", Some(&directives), event()),
        ClassificationCounts {
            broadcasts: 1,
            ..ClassificationCounts::default()
        },
    );
    assert_eq!(
        presented_kinds("ACK 1kQnrzNZ: applied.", &directives, event()),
        [
            wire::MessageClassificationKind::Broadcast,
            wire::MessageClassificationKind::AcknowledgementPending,
        ],
    );
    assert_eq!(
        message_badges("NAK 1kQnrzNZ: refused.", Some(&directives), event()),
        ClassificationCounts {
            broadcasts: 1,
            ..ClassificationCounts::default()
        },
    );
    assert_eq!(
        presented_kinds("NAK 1kQnrzNZ: refused.", &directives, event()),
        [
            wire::MessageClassificationKind::Broadcast,
            wire::MessageClassificationKind::Decline,
        ],
    );
}

#[test]
fn taste_only_acknowledgement_carries_disposition_and_flavor() {
    let key = "dRyzkQp1P".parse::<steer::Key>().expect("taste key parses");
    let directive_text = "[TASTE] compatibility: Keep one tested path.";
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            actor: wire::Actor::Operator,
            published: published(1),
            lane: lane(),
            text: directive_text.to_owned(),
            authored: true,
            taste: true,
            review: false,
            disposition: wire::Disposition::Acked,
            quote_target: quote_target("dRyzkQp1P", directive_text),
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
    let text = "ACK dRyzkQp1P: Applied.";

    assert_eq!(
        presented_kinds(text, &directives, event()),
        [
            wire::MessageClassificationKind::Taste,
            wire::MessageClassificationKind::Acknowledgement,
        ],
    );
    assert_eq!(
        presented_text(text, &directives, event()).expect("taste acknowledgement presents"),
        "> [TASTE] compatibility: Keep one tested path.\n\n[ACK] Applied.",
    );
}

#[test]
fn acknowledged_review_feedback_earns_a_source_bound_semantic_badge() {
    assert!(is_review_feedback(Some("review-feedback:1kKRRsDR:42")));
    assert!(!is_review_feedback(Some("lane-renewal-context")));
    let source = "00000000-0000-4000-8000-000000000002"
        .parse()
        .expect("fixture source");
    let key = "1kQnrzNZ"
        .parse::<steer::Key>()
        .expect("fixture key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            actor: wire::Actor::Operator,
            published: published(1),
            lane: lane(),
            text: concat!(
                "[REVIEW] APP-1kKRRsDR\n\n",
                "> preserve the finding\n\n",
                "Keep working the current task."
            )
            .to_owned(),
            authored: true,
            taste: false,
            review: true,
            disposition: wire::Disposition::Acked,
            quote_target: quote_target(
                "1kQnrzNZ",
                concat!(
                    "[REVIEW] APP-1kKRRsDR\n\n",
                    "> preserve the finding\n\n",
                    "Keep working the current task."
                ),
            ),
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
            reviews: 1,
            ..ClassificationCounts::default()
        },
    );
    assert_eq!(
        presented_kinds(text, &directives, original),
        [
            wire::MessageClassificationKind::Review,
            wire::MessageClassificationKind::Acknowledgement,
        ],
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

    let rendered =
        presented_text(text, &directives, event()).expect("distributed acknowledgments present");
    assert_eq!(rendered.matches("The current completion path").count(), 1);
    assert_eq!(rendered.matches("Historical records").count(), 1);
    assert_eq!(rendered.matches("ACK 1kQwPZt").count(), 0);
    assert_eq!(rendered.matches(wire::stream::ACK_MARKER).count(), 0);
    assert_eq!(
        rendered.matches(wire::stream::ACK_PENDING_MARKER).count(),
        2
    );
    assert!(rendered.contains(
        "> Keep one current completion path.\n\n[ACK PENDING] The current completion path"
    ));
    assert!(
        rendered
            .contains("> Do not execute historical records.\n\n[ACK PENDING] Historical records")
    );
}

/// One header answering several directives quotes each of them, then opens its
/// one response with one marker beside the speaking agent.
#[test]
fn shared_acknowledgement_prose_renders_once_after_every_context() {
    let directives = directives(&[
        ("1kQwPZtK", "Keep one current completion path."),
        ("1kQwPZtL", "Do not execute historical records."),
    ]);

    assert_eq!(
        presented_text(
            "ACK 1kQwPZtK 1kQwPZtL: One response covers both directions.",
            &directives,
            event(),
        )
        .expect("shared acknowledgment presents"),
        concat!(
            "> Keep one current completion path.\n\n",
            "> Do not execute historical records.\n\n",
            "[ACK PENDING] One response covers both directions."
        ),
    );
}

#[test]
fn non_key_text_after_a_recognized_header_remains_response_prose() {
    let directives = directives(&[("1kQwPZtK", "Keep the recognized direction visible.")]);
    for marker in ["ACK", "NAK"] {
        let text = format!(
            "{marker} 1kQwPZtK nope 1kQwPZtM: Shared prose remains attached to the attempt."
        );
        let expected = format!(
            "> Keep the recognized direction visible.\n\n{} Nope 1kQwPZtM: Shared prose remains attached to the attempt.",
            if marker == "ACK" {
                "[ACK PENDING]"
            } else {
                "[NAK]"
            },
        );

        let rendered =
            presented_text(&text, &directives, event()).expect("mixed acknowledgment presents");
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
        presented_text(
            "[ACK 1kQwPZtK] — Applied.\n[NAK 1kQwPZtL] – Unsafe.",
            &directives,
            event(),
        )
        .expect("mixed acknowledgments present"),
        concat!(
            "> Apply the current path.\n\n[ACK PENDING] Applied.\n",
            "> Remove the safety check.\n\n[NAK] Unsafe."
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
        presented_text(
            "ACK 1kQwPZtK: First applied. ACK 1kQwPZtL: Second applied.",
            &directives,
            event(),
        )
        .expect("inline acknowledgments present"),
        concat!(
            "> Keep the first path.\n\n",
            "[ACK PENDING] First applied.\n",
            "> Keep the second path.\n\n",
            "[ACK PENDING] Second applied."
        ),
    );
}

/// A directive is quoted exactly as the operator wrote it, its own quotation
/// included; the answer marker opens the response and never splits a block the
/// response begins with.
#[test]
fn directives_quote_verbatim_and_the_marker_opens_the_response() {
    let mut directives = directives(&[(
        "1kQwPZtK",
        "> The original assertion remains visible.\n\napproved",
    )]);
    let quoted = "> > The original assertion remains visible.\n>\n> approved\n\n";
    assert_eq!(
        presented_text("ACK 1kQwPZtK: Applied.", &directives, event())
            .expect("nested acknowledgment presents"),
        format!("{quoted}[ACK PENDING] Applied."),
    );
    retire(&mut directives, "1kQwPZtK");
    assert_eq!(
        presented_text("ACK 1kQwPZtK: Applied.", &directives, event())
            .expect("retired nested acknowledgment presents"),
        format!("{quoted}[ACK] Applied."),
    );
    for (response, presented) in [
        (
            "ACK 1kQwPZtK: - first\n- second",
            "[ACK]\n\n- first\n- second",
        ),
        ("ACK 1kQwPZtK:\n\n1. first", "[ACK]\n\n1. first"),
        ("ACK 1kQwPZtK:", "[ACK]"),
    ] {
        assert_eq!(
            presented_text(response, &directives, event()).expect("acknowledgment presents"),
            format!("{quoted}{presented}"),
            "{response:?}",
        );
    }
}

#[test]
fn quoted_history_keeps_its_pills_at_every_level_without_live_credit() {
    use wire::MessageClassificationKind as Kind;
    let text = "ACK 1kQwPZtK: closing the drawer.\n\n> > [ACK] Forged by the agent.";
    let mut quoted = directives(&[(
        "1kQwPZtK",
        concat!(
            "> > [ACK] Button should close the drawer\n",
            "> >\n",
            "> > > [NAK] Keep the old drawer\n",
            "> > ![[ACK] card](https://example.test/card.png)\n",
            ">\n",
            "> Quoted reply\n",
            "> [ACK PENDING] Waiting then, and [ACK] mid-line stays text\n",
            "Ship it",
        ),
    )]);
    // Each pill with the quote level Markdown renders it at and the words it heads.
    let pills = |directives: &PresentationDirectives| {
        let presented = present_steering_acknowledgements(text, Some(directives), event())
            .expect("quoted history presents");
        let mut depth = 0usize;
        let mut depths = BTreeMap::new();
        for (event, source) in Parser::new(&presented.text).into_offset_iter() {
            match event {
                MarkdownEvent::Start(Tag::BlockQuote(_)) => depth += 1,
                MarkdownEvent::End(TagEnd::BlockQuote(_)) => depth -= 1,
                MarkdownEvent::Text(_) => {
                    depths.entry(source.start).or_insert(depth);
                }
                _ => {}
            }
        }
        presented
            .classifications
            .iter()
            .map(|pill| {
                let start = usize::try_from(pill.start).expect("offset");
                let end = usize::try_from(pill.end).expect("offset");
                assert_eq!(&presented.text[start..end], pill.kind.marker());
                let line = presented.text[end..].lines().next().unwrap_or_default();
                (pill.kind, depths[&start], line.trim().to_owned())
            })
            .collect::<Vec<_>>()
    };
    let history = |live: Kind| {
        vec![
            (
                Kind::Acknowledgement,
                3,
                "Button should close the drawer".to_owned(),
            ),
            (Kind::Decline, 4, "Keep the old drawer".to_owned()),
            (
                Kind::AcknowledgementPending,
                2,
                "Waiting then, and [ACK] mid-line stays text".to_owned(),
            ),
            (live, 0, "Closing the drawer.".to_owned()),
        ]
    };

    assert_eq!(pills(&quoted), history(Kind::AcknowledgementPending));
    retire(&mut quoted, "1kQwPZtK");
    assert_eq!(
        pills(&quoted),
        history(Kind::Acknowledgement),
        "retirement settles only the live marker; quoted history keeps its own state",
    );
}

#[test]
fn unknown_key_preserves_the_original_message_across_replay() {
    let text = "[ACK 1kQwPZtK] — No directive owns this key.";
    let directives = PresentationDirectives::default();
    let first = presented_text(text, &directives, event()).expect("unknown key remains visible");
    let replayed =
        presented_text(text, &directives, event()).expect("unknown key replay remains visible");

    assert_eq!(first, text);
    assert_eq!(replayed, first);
}

#[test]
fn system_acknowledgment_badge_reaches_app_without_speech_eligibility() {
    let key = "dRyzkGX8b"
        .parse::<steer::Key>()
        .expect("current system key parses");
    let directives = PresentationDirectives(BTreeMap::from([(
        key,
        PresentationDirective {
            actor: wire::Actor::Operator,
            published: published(1),
            lane: lane(),
            text: "Accepted lane renewal context.".to_owned(),
            authored: false,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            quote_target: quote_target("dRyzkGX8b", "Accepted lane renewal context."),
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
        external_media: Vec::new(),
        locus: event().locus,
    });
    let publisher = Messages::default();

    publish_agent_event(
        &publisher,
        AgentEvent {
            lane: lane(),
            author: lane(),
            source: Some(TEST_TAPE_SOURCE.parse().expect("fixture source")),
            ordinal: 0,
            decoded: &prose,
            authored: None,
            observed: None,
            observations: &MessageObservations::default(),
            unavailable_tool_terminals: &[],
            prose_origin: None,
            suppress_intent: false,
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
        presented_text(text, &directives, event()).expect("pending acknowledgment presents"),
        "> Keep the current path.\n\n[ACK PENDING] Applied.",
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
    let record = capture_record(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ACK 1kQnrzNZ: Applied:"}]}}"#,
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
        "> Keep the current path.\n\n[ACK PENDING] Applied."
    );
    let [context] = messages[0].directive_contexts.as_slice() else {
        panic!("one directive context should be quoteable")
    };
    assert_eq!(
        &messages[0].text[context.start as usize..context.end as usize],
        "> Keep the current path."
    );
    assert_eq!(
        context.quote_target,
        quote_target("1kQnrzNZ", "Keep the current path.")
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
        presented_text(text, &directives, event()).expect("retired acknowledgment presents"),
        "> Keep the current path.\n\n[ACK] Applied.",
    );
    assert_eq!(
        presented_kinds(text, &directives, event()),
        [wire::MessageClassificationKind::Acknowledgement],
    );
    assert!(
        pending::PendingPresentation::of(text, Some(&directives), event(), locus).is_none(),
        "a retired key leaves nothing to settle"
    );
}

#[test]
fn a_narrated_messages_acknowledging_intent_presents_as_an_acknowledgement_card() {
    let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
    let publisher = Messages::default();
    let mut state = MessageState::default();
    let narrated = capture_record(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Keeping the path and checking the log."},{"type":"toolCall","id":"call-1","name":"shell","arguments":{"command":"git status"},"intent":"ACK 1kQnrzNZ: Applied, checking the log"},{"type":"toolCall","id":"call-2","name":"read","arguments":{"path":"a"},"intent":"Reading the next file"}],"stopReason":"toolUse"}}"#,
        0,
    );
    publish_agent_messages(&publisher, &narrated, &mut state, Some(&directives))
        .expect("narrated message publishes");

    let messages = publisher.0.lock().expect("message lock");
    let cards = messages
        .iter()
        .filter(|message| message.activity == wire::AgentActivityKind::Prose)
        .collect::<Vec<_>>();
    let [card] = cards.as_slice() else {
        panic!("only the acknowledging intent presents as a card: {cards:?}")
    };
    assert_eq!(
        card.text,
        "> Keep the current path.\n\n[ACK PENDING] Applied, checking the log"
    );
    assert_eq!(card.locus.event, 1);
    assert_eq!(
        state.pending.len(),
        1,
        "the intent card settles when the journal retires its key"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.text.contains("Reading the next file")),
        "an intent without a header stays tool activity"
    );
}

#[test]
fn every_allocated_key_a_header_names_presents_its_directive_without_earning_credit() {
    let other_lane = "00000000-0000-4000-8000-000000000009"
        .parse()
        .expect("other lane");
    let other_source = "00000000-0000-4000-8000-000000000003"
        .parse()
        .expect("other source");
    let retired_at = wire::body::TranscriptLocus {
        source: Some(other_source),
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column: 5,
    };
    let mut directives = directives(&[
        ("1kQwPZtK", "Another lane's live direction."),
        ("1kQwPZtL", "Another lane's settled direction."),
        ("1kQwPZtM", "[REVIEW] APP-1kKRRsDR\n\nKeep working."),
        ("1kQwPZtN", "Refused earlier on this lane."),
    ]);
    for (key, lane, disposition, review) in [
        ("1kQwPZtK", other_lane, wire::Disposition::Live, false),
        ("1kQwPZtL", other_lane, wire::Disposition::Acked, false),
        ("1kQwPZtM", lane(), wire::Disposition::Acked, true),
        ("1kQwPZtN", lane(), wire::Disposition::Declined, false),
    ] {
        let directive = directives
            .0
            .get_mut(&key.parse().expect("fixture key"))
            .expect("fixture directive");
        directive.lane = lane;
        directive.disposition = disposition;
        directive.review = review;
        directive.retirement = (disposition != wire::Disposition::Live).then_some(retired_at);
    }
    for (text, presented) in [
        (
            "ACK 1kQwPZtK: noted.",
            "> Another lane's live direction.\n\nNoted.",
        ),
        (
            "ACK 1kQwPZtL is done.",
            "> Another lane's settled direction.\n\nIs done.",
        ),
        (
            "ACK 1kQwPZtM: again.",
            "> [REVIEW] APP-1kKRRsDR\n>\n> Keep working.\n\nAgain.",
        ),
        (
            "ACK 1kQwPZtN: after all.",
            "> Refused earlier on this lane.\n\nAfter all.",
        ),
        (
            "ACK 1kQwPZtK 1kQwPZtZ: mixed.",
            "> Another lane's live direction.\n\nACK 1kQwPZtZ: mixed.",
        ),
    ] {
        assert_eq!(
            presented_text(text, &directives, event()).expect("referenced key presents"),
            presented,
            "wrong presentation for {text:?}",
        );
        assert!(
            message_badges(text, Some(&directives), event()).is_empty(),
            "a reference this response did not answer earns no badge: {text:?}",
        );
        let locus = wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
        };
        assert!(
            pending::PendingPresentation::of(text, Some(&directives), event(), locus).is_none(),
            "a reference never waits on another answer: {text:?}",
        );
    }
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
            actor: wire::Actor::Operator,
            published: published(1),
            lane: lane(),
            text: "Keep the original response context.".to_owned(),
            authored: true,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            quote_target: quote_target("1kQwPZtK", "Keep the original response context."),
            retirement: Some(retirement),
        },
    )]));
    let text = "ACK 1kQwPZtK: Applied.";

    assert_eq!(
        presented_text(text, &directives, event()).expect("retirement locus presents"),
        "> Keep the original response context.\n\n[ACK] Applied.",
    );
    let mut later = event();
    later.locus = listen::Locus::new(1, 8, 1);
    assert_eq!(
        presented_text(text, &directives, later).expect("repeated acknowledgement presents"),
        "> Keep the original response context.\n\n[ACK] Applied.",
    );
    assert_eq!(
        presented_kinds(text, &directives, later),
        [wire::MessageClassificationKind::Acknowledgement],
    );
    assert!(
        automatic_speech::classify(
            &listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: None,
                media: Vec::new(),
                external_media: Vec::new(),
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
                actor: wire::Actor::Operator,
                published: published(1),
                lane: lane(),
                text: "Continue".to_owned(),
                authored: true,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target("1kR5x8yR", "Continue"),
                retirement: Some(retirement(8, 5)),
            },
        ),
        (
            current,
            PresentationDirective {
                actor: wire::Actor::Operator,
                published: published(2),
                lane: lane(),
                text: "[TASTE] fallbacks: Keep one deterministic path.".to_owned(),
                authored: true,
                taste: true,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target(
                    "1kR5xJDD",
                    "[TASTE] fallbacks: Keep one deterministic path.",
                ),
                retirement: Some(retirement(0, 14)),
            },
        ),
    ]));
    let text = concat!(
        "ACK 1kR5x8yR 1kR5xJDD: Continuing with the explicit two-context contract ",
        "only."
    );

    let presented = present_steering_acknowledgements(text, Some(&directives), event())
        .expect("mixed historical and current keys present");
    assert_eq!(
        presented.text,
        concat!(
            "> Continue\n\n",
            "> [TASTE] fallbacks: Keep one deterministic path.\n\n",
            "[ACK] Continuing with the explicit two-context contract only."
        ),
    );
    assert_eq!(
        presented
            .directive_contexts
            .iter()
            .map(|context| context.authored)
            .collect::<Vec<_>>(),
        [published(1), published(2)],
        "each context keeps its directive's publication instant"
    );
    assert_eq!(
        message_badges(text, Some(&directives), event()),
        ClassificationCounts {
            tastes: 1,
            ..ClassificationCounts::default()
        },
    );
    assert_eq!(
        presented_kinds(text, &directives, event()),
        [
            wire::MessageClassificationKind::Taste,
            wire::MessageClassificationKind::Acknowledgement,
        ],
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
                actor: wire::Actor::Operator,
                published: published(1),
                lane: lane(),
                text: "Accepted lane renewal context.".to_owned(),
                authored: false,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target("dRyzkGX8b", "Accepted lane renewal context."),
                retirement: Some(retirement(13)),
            },
        ),
        (
            retained,
            PresentationDirective {
                actor: wire::Actor::Operator,
                published: published(2),
                lane: lane(),
                text: "Keep retained direction readable.".to_owned(),
                authored: true,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target("1kQwPZtK", "Keep retained direction readable."),
                retirement: Some(retirement(23)),
            },
        ),
    ]));
    let text = "Before. ACK dRyzkGX8b 1kQwPZtK: applied. After.";

    assert_eq!(
        presented_text(text, &directives, event())
            .expect("inline mixed-width acknowledgment presents"),
        "Before.\n> Keep retained direction readable.\n\n[ACK] Applied. After.",
    );
    assert_eq!(
        presented_kinds(text, &directives, event()),
        [wire::MessageClassificationKind::Acknowledgement],
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
            actor: wire::Actor::Operator,
            published: published(1),
            lane: lane(),
            text: "Keep the original response context.".to_owned(),
            authored: true,
            taste: false,
            review: false,
            disposition: wire::Disposition::Acked,
            quote_target: quote_target("1kQwPZtK", "Keep the original response context."),
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
    let expected = "> Keep the original response context.\n\n[ACK] Applied.";

    let mut original = event();
    original.source = Some(expected_source);
    assert_eq!(
        presented_text(text, &directives, original).expect("original source presents"),
        expected,
    );

    let mut replacement = event();
    replacement.source = Some(
        "00000000-0000-4000-8000-000000000003"
            .parse()
            .expect("replacement source"),
    );
    assert_eq!(
        presented_text(text, &directives, replacement).expect("replacement source presents"),
        expected,
    );
    assert_eq!(
        presented_kinds(text, &directives, replacement),
        [wire::MessageClassificationKind::Acknowledgement],
    );
    assert!(
        automatic_speech::classify(
            &listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: None,
                media: Vec::new(),
                external_media: Vec::new(),
                locus: replacement.locus,
            },
            Some(&directives),
            replacement,
        )
        .is_none(),
        "a later source may present context but cannot re-earn acknowledgement speech",
    );
    assert_eq!(
        presented_text(text, &directives, event()).expect("source-less history presents"),
        expected,
    );
}

#[test]
fn repeated_lane_seal_acknowledgements_render_without_fabricating_a_directive() {
    let seal = steer::Key::for_identity(lane());
    let text = format!("ACK {seal}: Continue.");
    let expected = "[ACK] Continue.";
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
            presented_text(&text, &directives, acknowledged,)
                .expect("lane seal acknowledgement presents"),
            expected,
        );
        assert_eq!(
            presented_kinds(&text, &directives, acknowledged),
            [wire::MessageClassificationKind::Acknowledgement],
        );
        assert!(
            automatic_speech::classify(
                &listen::Prose {
                    role: listen::ProseRole::Assistant,
                    text: text.clone(),
                    phase: None,
                    media: Vec::new(),
                    external_media: Vec::new(),
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
