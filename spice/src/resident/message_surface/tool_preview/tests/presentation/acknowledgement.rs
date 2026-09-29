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
            presented_text(text, &directives, event()).expect("acknowledgment presents"),
            "> [ACK PENDING] Keep the current path.\n\nApplied.",
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
            "> [ACK PENDING] Keep the current path.\n\nApplied.",
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
            "> [ACK PENDING] Keep the current path.\n\n",
            "Applied."
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
            "> [ACK PENDING] Apply the current path.\n\n",
            "Applied [within scope] – keep — these.\n",
            "> [NAK] Keep the safety check.\n\n",
            "Unsafe [without review] — retained."
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
        message_badges("NAK 1kQnrzNZ: unsafe.", Some(&directives), event()),
        ClassificationCounts {
            declines: 1,
            ..ClassificationCounts::default()
        },
        "a refusal is an answer, counted apart from credit",
    );
    for text in [
        "ACK not-a-key: prose only.",
        "NAK not-a-key: prose only.",
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
    assert!(message_badges("NAK 1kQnrzNZ: refused.", Some(&directives), event()).is_empty());
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
                "[REVIEW] CANVAS-1kKRRsDR\n\n",
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
                    "[REVIEW] CANVAS-1kKRRsDR\n\n",
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
        presented_text(
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
    for marker in ["ACK", "NAK"] {
        let text = format!(
            "{marker} 1kQwPZtK nope 1kQwPZtM: Shared prose remains attached to the attempt."
        );
        let context = match marker {
            "ACK" => "> [ACK PENDING] Keep the recognized direction visible.",
            _ => "> [NAK] Keep the recognized direction visible.",
        };
        let expected =
            format!("{context}\n\nNope 1kQwPZtM: Shared prose remains attached to the attempt.");

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
            "> [ACK PENDING] Apply the current path.\n\nApplied.\n",
            "> [NAK] Remove the safety check.\n\nUnsafe."
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
        presented_text("ACK 1kQwPZtK: Applied.", &directives, event(),)
            .expect("nested acknowledgment presents"),
        "> > The original assertion remains visible.\n>\n> [ACK PENDING] approved\n\nApplied.",
    );
    retire(&mut directives, "1kQwPZtK");
    assert_eq!(
        presented_text("ACK 1kQwPZtK: Applied.", &directives, event(),)
            .expect("retired nested acknowledgment presents"),
        "> > The original assertion remains visible.\n>\n> [ACK] approved\n\nApplied.",
    );
}

#[test]
fn the_marker_lands_on_the_operator_words_below_a_quote_card() {
    let card = quote_target("dTVLR0qL0", "The quoted agent words.").markdown();
    for (directive, context) in [
        (
            format!("{card}\n\nWhat is this about?"),
            format!("> {card}\n>\n> [ACK] What is this about?"),
        ),
        (
            format!("{card}\nWhat is this about?"),
            format!("> {card}\n>\n> [ACK] What is this about?"),
        ),
    ] {
        let mut directives = directives(&[("1kQwPZtK", &directive)]);
        retire(&mut directives, "1kQwPZtK");
        assert_eq!(
            presented_text("ACK 1kQwPZtK: Applied.", &directives, event())
                .expect("retired acknowledgment presents"),
            format!("{context}\n\nApplied."),
            "{directive:?}",
        );
    }
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
        presented_text("ACK 1kQwPZtK: Applied.", &directives, event(),)
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
        presented_text(
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
        presented_text(text, &quoted, event()).expect("pending acknowledgment presents"),
        presented(wire::stream::ACK_PENDING_MARKER),
        "pending credit marks the operator's own words, not the card they quoted",
    );
    retire(&mut quoted, "1kQwPZtK");
    assert_eq!(
        presented_text(text, &quoted, event()).expect("retired acknowledgment presents"),
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
            (live, 1, "Ship it".to_owned()),
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
fn system_acknowledgment_badge_reaches_canvas_without_speech_eligibility() {
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
        "> [ACK PENDING] Keep the current path.\n\nApplied."
    );
    let [context] = messages[0].directive_contexts.as_slice() else {
        panic!("one directive context should be quoteable")
    };
    assert_eq!(
        &messages[0].text[context.start as usize..context.end as usize],
        "> [ACK PENDING] Keep the current path."
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
        "> [ACK PENDING] Keep the current path.\n\nApplied, checking the log"
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
        presented_text(text, &directives, event()).expect("cross-lane key remains visible"),
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
        "> [ACK] Keep the original response context.\n\nApplied.",
    );
    let mut later = event();
    later.locus = listen::Locus::new(1, 8, 1);
    assert_eq!(
        presented_text(text, &directives, later).expect("repeated acknowledgement presents"),
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
            "> [ACK] Continue\n\n",
            "> [TASTE] fallbacks: Keep one deterministic path.\n\n",
            "Continuing with the explicit two-context contract only."
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
                actor: wire::Actor::Operator,
                published: published(1),
                lane: lane(),
                text: "Accepted lane renewal context.".to_owned(),
                authored: false,
                taste: false,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target("dRyzkGX8b", "Accepted lane renewal context."),
                retirement: Some(retirement(5)),
            },
        ),
        (
            taste,
            PresentationDirective {
                actor: wire::Actor::Operator,
                published: published(2),
                lane: lane(),
                text: "[TASTE] compatibility: Keep one tested path.".to_owned(),
                authored: true,
                taste: true,
                review: false,
                disposition: wire::Disposition::Acked,
                quote_target: quote_target(
                    "dRyzkQp1P",
                    "[TASTE] compatibility: Keep one tested path.",
                ),
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
        presented_text(text, &directives, event())
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
    let expected = "> [ACK] Keep the original response context.\n\nApplied.";

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
            presented_text(&text, &directives, acknowledged,)
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
