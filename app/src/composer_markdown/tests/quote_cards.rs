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
    assert!(!rendered.html.contains("history unavailable"));
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
        }],
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

    assert_eq!(
        rendered
            .html
            .matches("data-quote-state=\"resolved\"")
            .count(),
        2
    );
    // Only the agent's quote wears its lane accent; the operator's nested quote stays neutral.
    let shell = " data-accent=\"3\"><div class=\"message-quote-body\">";
    assert_eq!(rendered.html.matches(shell).count(), 1);
    assert_eq!(
        rendered.html.matches("message-quote-age-slot").count(),
        3,
        "the outer source leads both hunks around the nested quote"
    );
    assert_eq!(rendered.html.matches("message-quote-name").count(), 3);
    assert!(!rendered.html.contains("history unavailable"));
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
    assert!(!html.contains("history unavailable"));
    assert!(!html.contains("Directive by"));
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
    };
    let outer = wire::QuotePresentation::Resolved {
        target: outer_target.to_string(),
        text: outer_text,
        author: wire::QuoteAuthor::Directive {
            actor: wire::Actor::Operator,
        },
        disposition: None,
        quotes: vec![inner],
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
        }],
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

    let projection = referenced_quote_agents(&quotes, &[], &roster);
    assert_eq!(render(&projection), render(&roster));

    let mut reseated = roster.clone();
    reseated.remove(&(unrelated, Some(id("2c"))));
    reseated.insert((unrelated, Some(id("3c"))), agent("unrelated"));
    reseated.insert((arriving, Some(id("3d"))), agent("arriving"));
    assert_eq!(referenced_quote_agents(&quotes, &[], &reseated), projection);
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

    assert!(rendered.html.contains("Quote unavailable: depth-exceeded"));
    assert!(rendered.html.contains("authorship unavailable"));
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
/// attribution lead every direct run, images precede its prose, a nested quote
/// owns its own lead, and the surrounding speaker resumes afterward.
#[test]
fn attribution_hunks_restart_around_quotes_and_lead_with_their_images() {
    let quoted = "> quoted words";
    let markdown =
        format!("first answer\n\n![first](a.png)\n\n{quoted}\n\nsecond answer\n\n![second](b.png)");
    let quote_start = markdown.find(quoted).expect("fixture quote");
    let directive = MessageDirective {
        start: u32::try_from(quote_start).expect("fixture offset fits"),
        end: u32::try_from(quote_start + quoted.len()).expect("fixture offset fits"),
        quote_target: steering_target("dTVQdQfLx", quoted),
        actor: wire::Actor::Operator,
        authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
    };
    let target = directive.quote_target.to_string();
    let sources = BTreeMap::from([(target.clone(), QuoteSource::directive(&directive))]);
    let html = render_prose_card(
        &markdown,
        &[],
        &[directive],
        &[],
        &QuoteAgents::new(),
        &sources,
    )
    .html;

    assert_eq!(html.matches("class=\"message-hunk\"").count(), 3, "{html}");
    let answer_controls = html
        .match_indices("data-message-controls=\"\"")
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    let answer_attributions = html
        .match_indices(SIGNOFF_SLOT)
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    assert_eq!(answer_controls.len(), 2, "{html}");
    assert_eq!(answer_attributions.len(), 2, "{html}");

    let first_image = html.find("src=\"a.png\"").expect("first image");
    let first_words = html.find("first answer").expect("first answer");
    let quote_controls = html
        .find(&format!(
            "data-message-controls=\"{}\"",
            escape_html_attribute(&target)
        ))
        .expect("quote controls");
    let quote_age = html
        .find("data-quote-age-authored=\"1000\"")
        .expect("quote age");
    let quote_words = html.find("quoted words").expect("quote words");
    let quote_task = html
        .find("data-quote-tasks=\"dTVQdQfLx\"")
        .expect("quote task provenance");
    let second_image = html.find("src=\"b.png\"").expect("second image");
    let second_words = html.find("second answer").expect("second answer");
    assert!(
        answer_controls[0] < answer_attributions[0]
            && answer_attributions[0] < first_image
            && first_image < first_words
            && first_words < quote_controls
            && quote_controls < quote_age
            && quote_age < quote_words
            && quote_words < quote_task
            && quote_task < answer_controls[1]
            && answer_controls[1] < answer_attributions[1]
            && answer_attributions[1] < second_image
            && second_image < second_words,
        "{html}"
    );
}
