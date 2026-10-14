use super::*;

fn quote_fixture_id(tail: &str) -> wire::CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail}")
        .parse()
        .expect("fixture identity is canonical")
}

fn quote_source(card_id: &str, display_millis: i64) -> QuoteSource {
    QuoteSource {
        card_id: Some(card_id.to_owned()),
        display_millis,
        agent: None,
    }
}

fn nested_quote_sources(
    outer: &wire::QuoteTarget,
    inner: &wire::QuoteTarget,
    agent: QuoteAgent,
) -> QuoteSources {
    BTreeMap::from([
        (
            outer.to_string(),
            QuoteSource {
                card_id: Some("message-agent-outer-source".to_owned()),
                display_millis: 1_000,
                agent: Some(agent),
            },
        ),
        (
            inner.to_string(),
            QuoteSource {
                card_id: None,
                display_millis: 2_000,
                agent: None,
            },
        ),
    ])
}

fn assert_historical_agent_name_survives_reseating(
    outer: &wire::QuoteTarget,
    quotes: &[wire::QuotePresentation],
    sources: &QuoteSources,
    lane: wire::CanonicalUuid,
    source: wire::CanonicalUuid,
) {
    let successor = quote_fixture_id("e9");
    let agents = BTreeMap::from([(
        (lane, Some(successor)),
        QuoteAgent {
            accent: 4,
            label: "spice-rust-k".to_owned(),
        },
    )]);
    let rendered =
        render_received_with_quotes(&outer.markdown(), &[], &[], quotes, &agents, sources);
    assert!(rendered.html.contains(&format!(
        "class=\"message-quote-name\" data-quote-agent-lane=\"{lane}\" \
         data-quote-agent-occupant=\"{source}\" data-quote-author-state=\"historical\" \
         data-accent=\"3\">spice-rust-k</span>"
    )));
    assert!(rendered.html.contains("data-quote-age-authored=\"1000\""));
}

#[test]
fn resolved_quote_links_render_complete_nested_cards_synchronously() {
    let inner_text = "authenticated inner direction";
    let inner_target = steering_target("dTVQdQfLw", inner_text);
    let outer_text = format!("before {} after", inner_target.markdown());
    let lane = quote_fixture_id("e6");
    let source = quote_fixture_id("e7");
    let outer_target = wire::QuoteTarget::Tape {
        lane,
        source,
        locus: wire::MessageLocus {
            member: 1,
            offset: 24,
            length: 80,
            event: 2,
        },
        digest: wire::QuoteTarget::text_digest(&outer_text),
    };
    let quotes = vec![wire::QuotePresentation::Resolved {
        target: outer_target.to_string(),
        text: outer_text,
        author: wire::QuoteAuthor::Agent {
            lane,
            occupant: source,
        },
        disposition: None,
        quotes: vec![wire::QuotePresentation::Resolved {
            target: inner_target.to_string(),
            text: inner_text.to_owned(),
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Operator,
            },
            disposition: Some(wire::Disposition::Live),
            quotes: Vec::new(),
            packet: None,
            result: None,
            addressee: None,
        }],
        packet: None,
        result: None,
        addressee: None,
    }];

    let agents = BTreeMap::from([(
        (lane, Some(source)),
        QuoteAgent {
            accent: 3,
            label: "spice-rust-k".to_owned(),
        },
    )]);
    let sources = nested_quote_sources(
        &outer_target,
        &inner_target,
        agents
            .get(&(lane, Some(source)))
            .expect("fixture agent exists")
            .clone(),
    );
    assert_eq!(referenced_quote_sources(&quotes, &[], &sources), sources);
    let rendered = render_received_with_quotes(
        &outer_target.markdown(),
        &[],
        &[],
        &quotes,
        &agents,
        &sources,
    );

    for (needle, count) in [
        ("data-message-quote-depth=\"1\"", 2),
        ("data-message-quote-depth=\"2\"", 1),
        ("data-quote-state=\"resolved\"", 2),
    ] {
        assert_eq!(rendered.html.matches(needle).count(), count, "{needle}");
    }
    assert!(!rendered.html.contains("style=\"--message-quote-depth"));
    // Only the agent's quote wears its lane accent; the operator's nested quote stays neutral.
    let shell = " data-accent=\"3\"><div class=\"message-quote-body\">";
    assert_eq!(rendered.html.matches(shell).count(), 1);
    assert_eq!(
        rendered.html.matches("message-quote-age-slot").count(),
        3,
        "the outer source leads both hunks around the nested quote"
    );
    assert_eq!(rendered.html.matches("message-quote-name").count(), 3);
    assert_eq!(rendered.html.matches("data-quote-source=").count(), 2);
    assert!(!rendered.html.contains("Original"));
    assert!(rendered.html.contains(inner_text));
    assert!(rendered.html.contains("data-quote-disposition=\"live\""));
    let outer_body = rendered
        .html
        .find("before ")
        .expect("outer authenticated prose renders");
    let outer_age = rendered
        .html
        .find("data-quote-age-authored=\"1000\"")
        .expect("outer age renders");
    let outer_name = rendered
        .html
        .find(">spice-rust-k</span>")
        .expect("outer name renders");
    assert!(
        outer_age < outer_name && outer_name < outer_body,
        "the outer source attribution leads its first hunk"
    );

    assert_historical_agent_name_survives_reseating(&outer_target, &quotes, &sources, lane, source);
    assert_sourceless_quote_has_no_fabricated_byline(&outer_target, &quotes, &agents);
}

/// Without source chronology, no current timestamp or author name substitutes
/// for the missing evidence.
fn assert_sourceless_quote_has_no_fabricated_byline(
    outer: &wire::QuoteTarget,
    quotes: &[wire::QuotePresentation],
    agents: &QuoteAgents,
) {
    let sourceless = render_received_with_quotes(
        &outer.markdown(),
        &[],
        &[],
        quotes,
        agents,
        &BTreeMap::new(),
    );
    assert!(!sourceless.html.contains("message-quote-name"));
    assert!(!sourceless.html.contains("message-quote-age-slot"));
    assert!(!sourceless.html.contains("data-quote-source="));
}

#[test]
fn directive_quote_cards_cover_every_disposition_and_board_backed_navigation() {
    let cases = [
        (wire::Disposition::Live, "live"),
        (wire::Disposition::Acked, "acked"),
        (wire::Disposition::Declined, "declined"),
        (wire::Disposition::Lapsed, "lapsed"),
        (wire::Disposition::Canceled, "canceled"),
        (wire::Disposition::DeadLettered, "dead-letter"),
    ];
    let keys = [
        "dTVQdQfLw",
        "dTVQdQfLx",
        "dTVQdQfLy",
        "dTVQdQfLz",
        "dTVQdQfMA",
        "dTVQdQfMB",
    ];
    let mut markdown = Vec::new();
    let mut quotes = Vec::new();
    for ((disposition, _), key) in cases.iter().zip(keys) {
        let text = format!("direction {key}");
        let target = steering_target(key, &text);
        markdown.push(target.markdown());
        quotes.push(wire::QuotePresentation::Resolved {
            target: target.to_string(),
            text,
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Operator,
            },
            disposition: Some(*disposition),
            quotes: Vec::new(),
            packet: None,
            result: None,
            addressee: None,
        });
    }
    let spice_text = "direction from Spice";
    let spice_target = steering_target("dTVQdQfMC", spice_text);
    markdown.push(spice_target.markdown());
    quotes.push(wire::QuotePresentation::Resolved {
        target: spice_target.to_string(),
        text: spice_text.to_owned(),
        author: wire::QuoteAuthor::Directive {
            actor: wire::Actor::Spice,
        },
        disposition: None,
        quotes: Vec::new(),
        packet: None,
        result: None,
        addressee: None,
    });
    let sources = BTreeMap::from([(
        spice_target.to_string(),
        quote_source("message-agent-spice-source", 3_000),
    )]);
    let rendered = render_received_with_quotes(
        &markdown.join(" "),
        &[],
        &[],
        &quotes,
        &BTreeMap::new(),
        &sources,
    );
    let html = rendered.html;

    for (_, key) in cases {
        assert!(
            html.contains(&format!("data-quote-disposition=\"{key}\"")),
            "{key}: {html}"
        );
    }
    assert_eq!(
        html.matches("class=\"message-ack-pill\"").count(),
        2,
        "ACK and NAK reuse the canonical acknowledgement pill"
    );
    let acknowledgement = html
        .find("data-quote-disposition=\"acked\"")
        .expect("acknowledged state renders");
    let acknowledged_prose = html
        .find("direction dTVQdQfLx")
        .expect("acknowledged prose renders");
    assert!(
        acknowledgement < acknowledged_prose,
        "the disposition leads its words: {html}"
    );
    for key in keys {
        assert!(
            html.contains(&format!(
                "direction {key}<span class=\"message-quote-task-tail\" \
                 data-quote-tasks=\"{key}\"></span></p>"
            )),
            "board provenance follows the quote's final word on its last line: {html}"
        );
    }
    assert!(html.contains(
        "<span class=\"message-quote-task-tail\" data-quote-tasks=\"dTVQdQfMC\"></span>"
    ));
    assert_eq!(html.matches("message-quote-age-slot").count(), 1);
    assert!(html.contains(
        "class=\"message-quote-name\" data-quote-author=\"directive\" \
         data-quote-actor=\"spice\" aria-label=\"Spice\">🌶️</span>"
    ));
    assert!(!html.contains(">Spice</span>"));
    assert!(!html.contains(">Operator</span>"));
    assert!(!html.contains("class=\"message-quote-disposition tag"));
}

#[test]
fn quote_levels_name_their_own_task_badges_at_every_occurrence() {
    let inner_text = "nested direction";
    let inner_target = steering_target("dTVQdQfLx", inner_text);
    let outer_text = format!("outer direction {}", inner_target.markdown());
    let outer_target = steering_target("dTVQdQfLw", &outer_text);
    let inner = wire::QuotePresentation::Resolved {
        target: inner_target.to_string(),
        text: inner_text.to_owned(),
        author: wire::QuoteAuthor::Directive {
            actor: wire::Actor::Operator,
        },
        disposition: None,
        quotes: Vec::new(),
        packet: None,
        result: None,
        addressee: None,
    };
    let outer = wire::QuotePresentation::Resolved {
        target: outer_target.to_string(),
        text: outer_text,
        author: wire::QuoteAuthor::Directive {
            actor: wire::Actor::Operator,
        },
        disposition: None,
        quotes: vec![inner],
        packet: None,
        result: None,
        addressee: None,
    };
    let sources = BTreeMap::from([
        (
            outer_target.to_string(),
            quote_source("message-agent-tasked-outer", 1_000),
        ),
        (
            inner_target.to_string(),
            quote_source("message-agent-tasked-inner", 2_000),
        ),
    ]);
    let rendered = render_received_with_quotes(
        &format!("{} {}", outer_target.markdown(), outer_target.markdown()),
        &[],
        &[],
        &[outer.clone(), outer],
        &BTreeMap::new(),
        &sources,
    );

    for (key, level) in [("dTVQdQfLw", "outer"), ("dTVQdQfLx", "nested")] {
        assert_eq!(
            rendered
                .html
                .matches(&format!("data-quote-tasks=\"{key}\""))
                .count(),
            2,
            "each {level} occurrence names its own task badges"
        );
    }
    assert_eq!(
        rendered
            .html
            .matches(
                "outer direction <span class=\"message-quote-task-tail\" \
                 data-quote-tasks=\"dTVQdQfLw\"></span></p>"
            )
            .count(),
        2,
        "a quote ending in a nested quote seats its provenance after its own last word: {}",
        rendered.html
    );
    assert!(!rendered.html.contains("message-quote-task-line"));
}

/// A card holds the Work of every direction it quotes, at any depth or as a
/// directive context, and no other entry of the board's origin index.
#[test]
fn a_cards_quote_tasks_follow_only_the_directions_it_quotes() {
    let inner_text = "nested direction";
    let inner = steering_target("dTVQdQfLx", inner_text);
    let outer_text = format!("outer direction {}", inner.markdown());
    let outer = steering_target("dTVQdQfLw", &outer_text);
    let context = steering_target("dTVQdQfLy", "acknowledged direction");
    let resolved =
        |target: &wire::QuoteTarget, text: &str, quotes| wire::QuotePresentation::Resolved {
            target: target.to_string(),
            text: text.to_owned(),
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Operator,
            },
            disposition: None,
            quotes,
            packet: None,
            result: None,
            addressee: None,
        };
    let quotes = [resolved(
        &outer,
        &outer_text,
        vec![resolved(&inner, inner_text, Vec::new())],
    )];
    let directives = [MessageDirective {
        start: 0,
        end: 1,
        quote_target: context,
        actor: wire::Actor::Operator,
        authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        badge: None,
        sender: None,
        recipient: None,
        packet: None,
        result: None,
    }];
    let work = |handle: &str| vec![handle.parse::<wire::TaskHandle>().expect("fixture task")];
    let index = BTreeMap::from([
        ("dTVQdQfLw".to_owned(), work("OUTER-dTZQjKHhG")),
        ("dTVQdQfLx".to_owned(), work("INNER-dTZRdv3bF")),
        ("dTVQdQfLy".to_owned(), work("CONTEXT-dTZRfCQ43")),
        ("dTVQdQfLz".to_owned(), work("ELSEWHERE-dTZRjCd3s")),
    ]);

    let mut expected = index.clone();
    expected.remove("dTVQdQfLz");
    assert_eq!(
        referenced_quote_tasks(&quotes, &directives, &index),
        expected
    );
}

/// A card keeps the agents its quotes can render and nothing else: rendering
/// from that projection matches rendering from the whole roster, and a seat the
/// quotes never name changing occupancy leaves the card's projection, and so
/// its measurement, untouched.
#[test]
fn a_cards_quote_agents_follow_only_the_lanes_its_quotes_name() {
    let id = |suffix: &str| -> wire::CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131164{suffix}")
            .parse()
            .expect("fixture identity is canonical")
    };
    let (quoted, directing, unrelated, arriving) = (id("0a"), id("0b"), id("0c"), id("0d"));
    let (departed, successor) = (id("1a"), id("1b"));
    let inner_text = "direction from a lane";
    let inner_target = steering_target("dTVQdQfLw", inner_text);
    let outer_text = format!("before {} after", inner_target.markdown());
    let outer_target = tape_target(&outer_text);
    let quotes = vec![wire::QuotePresentation::Resolved {
        target: outer_target.to_string(),
        text: outer_text,
        author: wire::QuoteAuthor::Agent {
            lane: quoted,
            occupant: departed,
        },
        disposition: None,
        quotes: vec![wire::QuotePresentation::Resolved {
            target: inner_target.to_string(),
            text: inner_text.to_owned(),
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Lane(directing),
            },
            disposition: Some(wire::Disposition::Live),
            quotes: Vec::new(),
            packet: None,
            result: None,
            addressee: None,
        }],
        packet: None,
        result: None,
        addressee: None,
    }];
    let agent = |label: &str| QuoteAgent {
        accent: 2,
        label: label.to_owned(),
    };
    let roster = BTreeMap::from([
        ((quoted, Some(successor)), agent("quoted")),
        ((directing, Some(id("2b"))), agent("directing")),
        ((unrelated, Some(id("2c"))), agent("unrelated")),
    ]);
    let render = |agents: &QuoteAgents| {
        render_received_with_quotes(
            &outer_target.markdown(),
            &[],
            &[],
            &quotes,
            agents,
            &BTreeMap::new(),
        )
        .html
    };

    let projection = referenced_quote_agents(&quotes, &[], &[], &roster);
    assert_eq!(render(&projection), render(&roster));

    let mut reseated = roster.clone();
    reseated.remove(&(unrelated, Some(id("2c"))));
    reseated.insert((unrelated, Some(id("3c"))), agent("unrelated"));
    reseated.insert((arriving, Some(id("3d"))), agent("arriving"));
    assert_eq!(
        referenced_quote_agents(&quotes, &[], &[], &reseated),
        projection
    );
}

#[test]
fn unavailable_quote_cards_keep_the_target_structural_without_showing_it_as_prose() {
    let target = steering_target("dTVQdQfLw", "missing");
    let target_text = target.to_string();
    let unavailable = [wire::QuotePresentation::Unavailable {
        target: target_text.clone(),
        reason: wire::QuoteUnavailable::DepthExceeded,
    }];
    let rendered = render_received_with_quotes(
        &target.markdown(),
        &[],
        &[],
        &unavailable,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );

    assert!(!rendered.html.contains("message-quote-signoff"));
    assert!(!rendered.html.contains("<a "));
    assert!(!rendered.html.contains("data-quote-tasks"));
    assert!(
        rendered
            .html
            .contains("data-quote-reason=\"depth-exceeded\"")
    );
    assert!(rendered.html.contains("data-quote-target=\"spice://quote/"));
    assert!(!rendered.html.contains("<code>"));
    assert!(!rendered.html.contains("message-quote-body"));
    let ordinary = render_received("> copied words");
    assert!(ordinary.html.contains("<blockquote>"));
    assert!(ordinary.html.contains("copied words"));
    assert!(!ordinary.html.contains("message-quote-card"));
}

/// Hunk ownership is the renderer's durable layout contract: controls and
/// attribution lead every direct run, each quote owns its lead, and the
/// surrounding speaker restarts after every interruption. The browser mounts
/// the message's semantic badges through each direct hunk's signoff slot.
#[test]
fn attribution_hunks_restart_after_more_than_one_quote() {
    let first_quote = "> first quoted words";
    let second_quote = "> second quoted words";
    let markdown = format!(
        "first answer\n\n![first](a.png)\n\n{first_quote}\n\n\
         second answer\n\n![second](b.png)\n\n{second_quote}\n\nthird answer"
    );
    let directive = |key, quoted: &str, authored: &str| {
        let start = markdown.find(quoted).expect("fixture quote");
        MessageDirective {
            start: u32::try_from(start).expect("fixture offset fits"),
            end: u32::try_from(start + quoted.len()).expect("fixture offset fits"),
            quote_target: steering_target(key, quoted),
            actor: wire::Actor::Operator,
            authored: authored.parse().expect("fixture instant"),
            badge: None,
            sender: None,
            recipient: None,
            packet: None,
            result: None,
        }
    };
    let directives = [
        directive("dTVQdQfLx", first_quote, "1970-01-01T00:00:01.000Z"),
        directive("dTVQdQfLy", second_quote, "1970-01-01T00:00:02.000Z"),
    ];
    let targets = directives
        .iter()
        .map(|directive| directive.quote_target.to_string())
        .collect::<Vec<_>>();
    let sources = directives
        .iter()
        .map(|directive| {
            (
                directive.quote_target.to_string(),
                QuoteSource::directive(directive),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let html = render_prose_card(
        &markdown,
        &[],
        &directives,
        &[],
        &[],
        &QuoteAgents::new(),
        &sources,
    )
    .html;

    assert_eq!(html.matches("class=\"message-hunk\"").count(), 5, "{html}");
    let answer_controls = html
        .match_indices("data-message-controls=\"\"")
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    let answer_attributions = html
        .match_indices(SIGNOFF_SLOT)
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    assert_eq!(answer_controls.len(), 3, "{html}");
    assert_eq!(
        html.matches("data-message-quote-depth=\"0\"").count(),
        3,
        "{html}"
    );
    assert_eq!(
        html.matches("data-message-quote-depth=\"1\"").count(),
        2,
        "{html}"
    );
    assert_eq!(answer_attributions.len(), 3, "{html}");

    let first_words = html.find("first answer").expect("first answer");
    let first_quote_controls = html
        .find(&format!(
            "data-message-controls=\"{}\"",
            escape_html_attribute(&targets[0])
        ))
        .expect("first quote controls");
    let first_quote_words = html.find("first quoted words").expect("first quote words");
    let second_words = html.find("second answer").expect("second answer");
    let second_quote_controls = html
        .find(&format!(
            "data-message-controls=\"{}\"",
            escape_html_attribute(&targets[1])
        ))
        .expect("second quote controls");
    let second_quote_words = html
        .find("second quoted words")
        .expect("second quote words");
    let third_words = html.find("third answer").expect("third answer");
    assert!(
        answer_controls[0] < answer_attributions[0]
            && answer_attributions[0] < first_words
            && first_words < first_quote_controls
            && first_quote_controls < first_quote_words
            && first_quote_words < answer_controls[1]
            && answer_controls[1] < answer_attributions[1]
            && answer_attributions[1] < second_words
            && second_words < second_quote_controls
            && second_quote_controls < second_quote_words
            && second_quote_words < answer_controls[2]
            && answer_controls[2] < answer_attributions[2]
            && answer_attributions[2] < third_words,
        "{html}"
    );
}

/// A quoted resident directive wears its typed kind once, in its own opening
/// attribution where its words begin; resumed hunks after a nested quote and
/// the answering card do not repeat it, and literal markers stay text.
#[test]
fn directive_badge_leads_its_quote_and_ignores_literal_markers() {
    let review = "> APP-1kKRRsDR\n>\n> > [REVIEW] missing coverage\n>\n> Verbalize the finding.";
    let plain = "> Keep one path.";
    let markdown = format!("{review}\n\n{plain}\n\nInternalized.");
    let directive = |key, quoted: &str, badge| {
        let start = markdown.find(quoted).expect("fixture quote");
        MessageDirective {
            start: u32::try_from(start).expect("fixture offset fits"),
            end: u32::try_from(start + quoted.len()).expect("fixture offset fits"),
            quote_target: steering_target(key, quoted),
            actor: wire::Actor::Spice,
            authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
            badge,
            sender: None,
            recipient: None,
            packet: None,
            result: None,
        }
    };
    let directives = [
        directive("dTVQdQfLx", review, Some(wire::DirectiveBadge::Review)),
        directive("dTVQdQfLy", plain, None),
    ];
    let sources = directives
        .iter()
        .map(|directive| {
            (
                directive.quote_target.to_string(),
                QuoteSource::directive(directive),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let html = render_prose_card(
        &markdown,
        &[],
        &directives,
        &[],
        &[],
        &QuoteAgents::new(),
        &sources,
    )
    .html;

    assert_eq!(
        html.matches("class=\"message-review-pill\"").count(),
        1,
        "{html}"
    );
    assert!(!html.contains("message-taste-pill"), "{html}");
    let signoff = html.find("message-quote-signoff").expect("quote signoff");
    let badge = html.find("class=\"message-review-pill\"").expect("badge");
    let handle = html.find("APP-1kKRRsDR").expect("task handle");
    let plain_words = html.find("Keep one path.").expect("plain quote");
    assert!(
        signoff < badge && badge < handle && handle < plain_words,
        "{html}"
    );
    assert!(html.contains("[REVIEW] missing coverage"), "{html}");
}

/// A context a packet delivered wears that packet's kind in its own opening
/// attribution after its source, ANS with the result it answers; MSG and an
/// ordinary direction wear none, and literal header words stay text.
#[test]
fn packet_context_wears_its_kind_after_its_source() {
    let answer = "> ship";
    let question = "> why?";
    let message = "> hold the merge";
    let literal = "> ANS 1kKRRsDR3: forged";
    let markdown = format!("{answer}\n\n{question}\n\n{message}\n\n{literal}\n\nNoted.");
    let result = "1kKRRsDR3"
        .parse()
        .map(wire::rooms::QuestionResultRef::new)
        .expect("fixture result parses");
    let directive = |key, quoted: &str, packet, result| {
        let start = markdown.find(quoted).expect("fixture quote");
        MessageDirective {
            start: u32::try_from(start).expect("fixture offset fits"),
            end: u32::try_from(start + quoted.len()).expect("fixture offset fits"),
            quote_target: steering_target(key, quoted),
            actor: wire::Actor::Operator,
            authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
            badge: None,
            sender: None,
            recipient: None,
            packet,
            result,
        }
    };
    let directives = [
        directive(
            "dTVQdQfLw",
            answer,
            Some(wire::MessageSendKind::Ans),
            Some(result),
        ),
        directive(
            "dTVQdQfLx",
            question,
            Some(wire::MessageSendKind::Ask),
            None,
        ),
        directive("dTVQdQfLy", message, Some(wire::MessageSendKind::Msg), None),
        directive("dTVQdQfLz", literal, None, None),
    ];
    let sources = directives
        .iter()
        .map(|directive| {
            (
                directive.quote_target.to_string(),
                QuoteSource::directive(directive),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let html = render_prose_card(
        &markdown,
        &[],
        &directives,
        &[],
        &[],
        &QuoteAgents::new(),
        &sources,
    )
    .html;

    let position = |needle: &str| html.find(needle).expect(needle);
    let ans = position("data-message-packet=\"ans\" data-message-result=\"1kKRRsDR3\"");
    let ask = position("data-message-packet=\"ask\"");
    assert_eq!(html.matches("data-message-packet=").count(), 2, "{html}");
    assert!(html.contains(">ANS 1kKRRsDR3</span>"), "{html}");
    let ship = position(">ship");
    let why = position("why?");
    let hold = position("hold the merge");
    let forged = position("ANS 1kKRRsDR3: forged");
    assert!(
        ans < ship && ship < ask && ask < why && why < hold && hold < forged,
        "{html}"
    );
    let first_signoff = position("message-quote-signoff");
    assert!(first_signoff < ans, "{html}");
}

/// A quote link to a packet-delivered directive wears the same packet pill in
/// its card head; a quoted direction no packet delivered stays literal.
#[test]
fn quoted_packet_cards_wear_their_kind_in_the_head() {
    let result = "1kKRRsDR3"
        .parse()
        .map(wire::rooms::QuestionResultRef::new)
        .expect("fixture result parses");
    let cases = [
        (
            "dTVQdQfLw",
            "ship",
            Some(wire::MessageSendKind::Ans),
            Some(result),
        ),
        ("dTVQdQfLx", "why?", Some(wire::MessageSendKind::Ask), None),
        (
            "dTVQdQfLy",
            "hold the merge",
            Some(wire::MessageSendKind::Msg),
            None,
        ),
        ("dTVQdQfLz", "ANS 1kKRRsDR3: forged", None, None),
    ];
    let kite: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce91311640a"
        .parse()
        .expect("fixture lane");
    let seat: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce91311641a"
        .parse()
        .expect("fixture occupant");
    let mut markdown = Vec::new();
    let mut quotes = Vec::new();
    let mut sources = QuoteSources::new();
    for (key, text, packet, result) in cases {
        let target = steering_target(key, text);
        markdown.push(target.markdown());
        sources.insert(target.to_string(), quote_source(key, 1_000));
        quotes.push(wire::QuotePresentation::Resolved {
            target: target.to_string(),
            text: text.to_owned(),
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Operator,
            },
            disposition: Some(wire::Disposition::Acked),
            quotes: Vec::new(),
            packet,
            result,
            addressee: packet.map(|_| kite),
        });
    }
    let agents = BTreeMap::from([(
        (kite, Some(seat)),
        QuoteAgent {
            accent: 3,
            label: "Kite".to_owned(),
        },
    )]);
    let html =
        render_received_with_quotes(&markdown.join("\n\n"), &[], &[], &quotes, &agents, &sources)
            .html;

    let position = |needle: &str| html.find(needle).expect(needle);
    assert_eq!(html.matches("data-message-packet=").count(), 2, "{html}");
    assert_eq!(
        html.matches(&format!(
            "data-quote-recipient-lane=\"{kite}\" data-accent=\"3\">Kite<"
        ))
        .count(),
        3,
        "every packet names its addressed lane: {html}"
    );
    let addressed = position("data-quote-recipient-lane=");
    let ans = position("data-message-packet=\"ans\" data-message-result=\"1kKRRsDR3\"");
    let ship = position(">ship");
    let ask = position("data-message-packet=\"ask\"");
    let why = position("why?");
    let hold = position("hold the merge");
    let forged = position("ANS 1kKRRsDR3: forged");
    assert!(
        addressed < ans
            && ans < ship
            && ship < ask
            && ask < why
            && why < hold
            && hold < forged
            && html.rfind("data-quote-recipient-lane=").expect("addressed") < hold,
        "{html}"
    );
}

/// A quoted MSG names the occupant that wrote it while that occupant holds
/// the seat. Once it leaves, whether the lane is vacant or reseated, the
/// lane's own name and accent attribute it historically. A lane gone from
/// the roster reads as unavailable, never as raw identifiers.
#[test]
fn a_quoted_msg_context_keeps_its_lane_identity_across_seats() {
    let id = |suffix: &str| -> wire::CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131164{suffix}")
            .parse()
            .expect("fixture identity is canonical")
    };
    let (lane, author, successor) = (id("0a"), id("1a"), id("1b"));
    let quoted = "> hold the merge";
    let markdown = format!("{quoted}\n\nHeld.");
    let directives = [MessageDirective {
        start: 0,
        end: u32::try_from(quoted.len()).expect("fixture offset fits"),
        quote_target: steering_target("dTVQdQfLx", quoted),
        actor: wire::Actor::Lane(lane),
        authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
        badge: None,
        sender: Some(author),
        recipient: None,
        packet: None,
        result: None,
    }];
    let sources = BTreeMap::from([(
        directives[0].quote_target.to_string(),
        QuoteSource::directive(&directives[0]),
    )]);
    let agent = |label: &str| QuoteAgent {
        accent: 2,
        label: label.to_owned(),
    };
    let render = |agents: &QuoteAgents| {
        render_prose_card(&markdown, &[], &directives, &[], &[], agents, &sources).html
    };
    let name = |state: &str, label: &str| {
        format!(
            "data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{author}\" \
             data-quote-author-state=\"{state}\"{}>{label}</span>",
            if state == "unresolved" {
                ""
            } else {
                " data-accent=\"2\""
            }
        )
    };

    for (agents, state) in [
        (
            BTreeMap::from([((lane, Some(author)), agent("lane-name"))]),
            "current",
        ),
        (
            BTreeMap::from([((lane, Some(successor)), agent("lane-name"))]),
            "historical",
        ),
        (
            BTreeMap::from([((lane, None), agent("lane-name"))]),
            "historical",
        ),
    ] {
        let html = render(&agents);
        assert!(html.contains(&name(state, "lane-name")), "{html}");
    }
    let absent = render(&QuoteAgents::new());
    assert!(
        absent.contains(&name("unresolved", "unavailable")),
        "{absent}"
    );
}

#[test]
fn peer_context_destination_survives_badges_and_reseating() {
    let id = |suffix: &str| -> wire::CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131166{suffix}")
            .parse()
            .unwrap()
    };
    let (sender_lane, sender, recipient_lane, recipient, successor) =
        (id("0a"), id("1a"), id("0b"), id("1b"), id("2b"));
    let body = "> Check the boundary.";
    let markdown = format!("{body}\n\nChecked.");
    let mut context = MessageDirective {
        start: 0,
        end: u32::try_from(body.len()).unwrap(),
        quote_target: steering_target("dTVQdQfLx", body),
        actor: wire::Actor::Lane(sender_lane),
        authored: "1970-01-01T00:00:01.000Z".parse().unwrap(),
        badge: None,
        sender: Some(sender),
        recipient: Some(wire::MessageDestination {
            lane: recipient_lane,
            occupant: recipient,
        }),
        packet: None,
        result: None,
    };
    let sources = BTreeMap::from([(
        context.quote_target.to_string(),
        QuoteSource::directive(&context),
    )]);
    let agent = |label: &str| QuoteAgent {
        accent: 1,
        label: label.to_owned(),
    };
    for badge in [
        None,
        Some(wire::DirectiveBadge::Review),
        Some(wire::DirectiveBadge::Taste),
    ] {
        context.badge = badge;
        for (seat, expected_state) in [
            (Some(recipient), "current"),
            (None, "historical"),
            (Some(successor), "historical"),
        ] {
            let roster = BTreeMap::from([
                ((sender_lane, Some(sender)), agent("sender")),
                ((recipient_lane, seat), agent("recipient-lane")),
            ]);
            let projection =
                referenced_quote_agents(&[], std::slice::from_ref(&context), &[], &roster);
            let html = render_prose_card(
                &markdown,
                &[],
                std::slice::from_ref(&context),
                &[],
                &[],
                &projection,
                &sources,
            )
            .html;
            assert!(html.contains("data-quote-direction=\"incoming\""), "{html}");
            let destination = html.split("message-quote-recipients").nth(1).unwrap();
            assert!(destination.contains(&format!("data-quote-agent-occupant=\"{recipient}\"")));
            assert!(destination.contains(&format!("data-quote-author-state=\"{expected_state}\"")));
            assert!(destination.contains(">recipient-lane</span>"));
            assert!(html.contains("Check the boundary."));
            assert!(html.contains("Checked."));
        }
    }
}

/// An outgoing MSG names its exact sender, every addressed occupant in header
/// order, the operator, and its room context without exposing protocol syntax.
/// A departed recipient and a vacant lane both keep their lane's name, and a
/// lane gone from the roster falls back to its address.
#[test]
fn a_sent_msg_names_every_destination_without_its_protocol_header() {
    let id = |suffix: &str| -> wire::CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131165{suffix}")
            .parse()
            .expect("fixture identity is canonical")
    };
    let (sender_lane, sender) = (id("0a"), id("1a"));
    let (seated_lane, seated) = (id("0b"), id("1b"));
    let (departed_lane, departed, successor) = (id("0c"), id("1c"), id("2c"));
    let (vacant_lane, vacated) = (id("0d"), id("1d"));
    let (absent_lane, absent) = (id("0e"), id("1e"));
    let recipient = |address: &str, lane, occupant| wire::MessageRecipient {
        address: address.parse().expect("fixture address parses"),
        lane,
        occupant,
    };
    let body = "> Please check the parser.";
    let markdown = format!("Asking for review.\n{body}");
    let start = markdown.find(body).expect("fixture body");
    let sends = [wire::MessageSend {
        start: u32::try_from(start).expect("fixture offset fits"),
        end: u32::try_from(markdown.len()).expect("fixture offset fits"),
        from: sender_lane,
        sender,
        recipients: vec![
            recipient("6Jv5", seated_lane, seated),
            recipient("6K26", departed_lane, departed),
            recipient("6KB3", vacant_lane, vacated),
            recipient("6KC4", absent_lane, absent),
        ],
        operator: true,
        contexts: vec![
            "ROOMS-dVrD244Jd"
                .parse()
                .expect("fixture task handle parses"),
        ],
        kind: wire::MessageSendKind::Msg,
        result: None,
    }];
    let agent = |accent: usize, label: &str| QuoteAgent {
        accent,
        label: label.to_owned(),
    };
    let agents = BTreeMap::from([
        ((sender_lane, Some(sender)), agent(1, "sender-agent")),
        ((seated_lane, Some(seated)), agent(2, "seated-agent")),
        ((departed_lane, Some(successor)), agent(3, "departed-lane")),
        ((vacant_lane, None), agent(4, "vacant-lane")),
    ]);
    let html = render_prose_card(
        &markdown,
        &[],
        &[],
        &sends,
        &[],
        &referenced_quote_agents(&[], &[], &sends, &agents),
        &QuoteSources::new(),
    )
    .html;

    assert!(!html.contains("MSG 6Jv5"), "{html}");
    let (lead, recipients) = html
        .split_once("data-quote-direction=\"outgoing\"")
        .and_then(|(_, signoff)| signoff.split_once("message-quote-recipients"))
        .expect("outgoing signoff names its recipients");
    assert!(lead.contains(">sender-agent</span>"), "{html}");
    let words = recipients.find("Please check the parser.").expect("body");
    let named = recipients[..words]
        .split("message-quote-recipient-separator")
        .collect::<Vec<_>>();
    assert_eq!(named.len(), 5, "{html}");
    for (name, (occupant, label, state)) in named[..4].iter().zip([
        (seated, ">seated-agent</span>", "current"),
        (departed, ">departed-lane</span>", "historical"),
        (vacated, ">vacant-lane</span>", "historical"),
        (absent, ">6KC4</span>", "unresolved"),
    ]) {
        assert!(name.contains(&format!("data-quote-agent-occupant=\"{occupant}\"")));
        assert!(name.contains(label), "{name}");
        assert!(name.contains(&format!("data-quote-author-state=\"{state}\"")));
    }
    assert!(
        named[4].contains("data-quote-recipient=\"operator\""),
        "{html}"
    );
    assert!(named[4].contains(">Operator</span>"), "{html}");
    assert!(
        html.contains("data-task-handle=\"ROOMS-dVrD244Jd\""),
        "{html}"
    );
}

#[test]
fn ask_and_ans_quotes_carry_typed_badges_and_the_answered_result() {
    let lane = "018f82d4-6f59-7d06-92e3-bce91311650a"
        .parse()
        .expect("fixture lane parses");
    let sender = "018f82d4-6f59-7d06-92e3-bce91311651a"
        .parse()
        .expect("fixture occupant parses");
    let question = "> Can you review this?";
    let answer = "> Reviewed.";
    let markdown = format!("{question}\n\n{answer}");
    let question_start = markdown.find(question).expect("question exists");
    let answer_start = markdown.find(answer).expect("answer exists");
    let result = "1kKRRsDR3"
        .parse()
        .map(wire::rooms::QuestionResultRef::new)
        .expect("fixture result parses");
    let sends = [
        wire::MessageSend {
            start: u32::try_from(question_start).expect("fixture offset fits"),
            end: u32::try_from(question_start + question.len()).expect("fixture offset fits"),
            from: lane,
            sender,
            recipients: Vec::new(),
            operator: true,
            contexts: Vec::new(),
            kind: wire::MessageSendKind::Ask,
            result: None,
        },
        wire::MessageSend {
            start: u32::try_from(answer_start).expect("fixture offset fits"),
            end: u32::try_from(answer_start + answer.len()).expect("fixture offset fits"),
            from: lane,
            sender,
            recipients: Vec::new(),
            operator: false,
            contexts: Vec::new(),
            kind: wire::MessageSendKind::Ans,
            result: Some(result),
        },
    ];
    let agents = BTreeMap::from([(
        (lane, Some(sender)),
        QuoteAgent {
            accent: 1,
            label: "answering-agent".to_owned(),
        },
    )]);
    let html = render_prose_card(
        &markdown,
        &[],
        &[],
        &sends,
        &[],
        &referenced_quote_agents(&[], &[], &sends, &agents),
        &QuoteSources::new(),
    )
    .html;

    assert!(html.contains("data-message-packet=\"ask\""), "{html}");
    assert!(html.contains("aria-label=\"Question\">"), "{html}");
    assert!(html.contains(">ASK</span>"), "{html}");
    assert!(html.contains("data-message-packet=\"ans\""), "{html}");
    assert!(html.contains("data-message-result=\"1kKRRsDR3\""), "{html}");
    assert!(html.contains(">ANS 1kKRRsDR3</span>"), "{html}");
    assert!(html.contains("data-quote-recipient=\"operator\""), "{html}");
}
