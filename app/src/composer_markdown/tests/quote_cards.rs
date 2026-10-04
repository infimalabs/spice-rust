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

fn directive_quote_source(display_millis: i64) -> QuoteSource {
    QuoteSource {
        card_id: None,
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
    assert!(rendered.html.contains(&format!(
        "<span class=\"message-quote-age-slot\" data-quote-age-authored=\"1000\" \
         data-quote-source=\"message-agent-outer-source\" data-quote-author=\"agent\" \
         data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{source}\" \
         data-quote-author-state=\"current\" data-accent=\"3\"></span>"
    )));
    assert!(rendered.html.contains(
        "<span class=\"message-quote-age-slot\" data-quote-age-authored=\"2000\" \
         data-quote-author=\"directive\" data-quote-actor=\"operator\"></span>"
    ));
    assert!(rendered.html.contains(&format!(
        "<span class=\"message-quote-name\" data-quote-agent-lane=\"{lane}\" \
         data-quote-agent-occupant=\"{source}\" data-quote-author-state=\"current\" \
         data-accent=\"3\">spice-rust-k</span>"
    )));
    assert!(rendered.html.contains(
        "<span class=\"message-quote-name\" data-quote-author=\"directive\" \
         data-quote-actor=\"operator\" role=\"img\" aria-label=\"Operator\"></span>"
    ));
    assert_eq!(rendered.html.matches("message-quote-name").count(), 2);
    assert!(!rendered.html.contains("history unavailable"));
    assert_eq!(rendered.html.matches("data-quote-source=").count(), 1);
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
        "the age, then the name, lead authored prose"
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
        "the acknowledgement precedes its directive prose"
    );
    for key in keys {
        assert!(
            html.contains(&format!(
                "<span class=\"message-quote-signoff\" data-quote-tasks=\"{key}\"></span>"
            )),
            "a sourceless Operator quote's tail holds only its task-badge slot: {html}"
        );
    }
    assert!(html.contains("<span class=\"message-quote-signoff\" data-quote-tasks=\"dTVQdQfMC\">"));
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
            .matches("data-quote-age-authored=\"1000\"")
            .count(),
        2,
        "each repeated outer level keeps its source time"
    );
    assert_eq!(
        rendered
            .html
            .matches("data-quote-age-authored=\"2000\"")
            .count(),
        2,
        "each nested level keeps its source time"
    );
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

/// An acknowledgement signs each directive context it quotes once, with the
/// directive's own author, beside any signoff of a quote nested inside it;
/// the agent's own prose and block quotes stay unsigned.
#[test]
fn acknowledged_directive_contexts_sign_their_author_once() {
    let directing: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131164ab"
        .parse()
        .expect("fixture identity is canonical");
    let nested_text = "earlier direction";
    let nested = steering_target("dTVQdQfLw", nested_text);
    let operator_text = format!("steer from the operator {}", nested.markdown());
    let lane_text = "steer from a lane";
    let operator = format!("> {operator_text}");
    let lane = format!("> {lane_text}");
    let markdown =
        format!("{operator}\n\n{lane}\n\nACK dTVQdQfLx dTVQdQfLy: on it.\n\n> my own aside");
    let context =
        |start: usize, len: usize, key: &str, text: &str, actor, authored: &str| MessageDirective {
            start: u32::try_from(start).expect("fixture offset fits"),
            end: u32::try_from(start + len).expect("fixture offset fits"),
            quote_target: steering_target(key, text),
            actor,
            authored: authored.parse().expect("fixture instant"),
        };
    let directives = [
        context(
            0,
            operator.len(),
            "dTVQdQfLx",
            &operator_text,
            wire::Actor::Operator,
            "1970-01-01T00:00:01.000Z",
        ),
        context(
            operator.len() + 2,
            lane.len(),
            "dTVQdQfLy",
            lane_text,
            wire::Actor::Lane(directing),
            "1970-01-01T00:00:02.000Z",
        ),
    ];
    let quotes = [wire::QuotePresentation::Resolved {
        target: nested.to_string(),
        text: nested_text.to_owned(),
        author: wire::QuoteAuthor::Directive {
            actor: wire::Actor::Operator,
        },
        disposition: None,
        quotes: Vec::new(),
    }];
    let agent = |occupant| {
        let label = "directing".to_owned();
        ((directing, occupant), QuoteAgent { accent: 1, label })
    };
    let sources = BTreeMap::from([
        (
            directives[0].quote_target.to_string(),
            QuoteSource::directive(&directives[0]),
        ),
        (
            directives[1].quote_target.to_string(),
            QuoteSource::directive(&directives[1]),
        ),
        (nested.to_string(), directive_quote_source(3_000)),
    ]);

    let render = |agents: &QuoteAgents| {
        render_received_with_quotes(&markdown, &[], &directives, &quotes, agents, &sources).html
    };
    let html = render(&BTreeMap::from([agent(Some(directing))]));

    let blocks = html.split("<blockquote>").skip(1).collect::<Vec<_>>();
    assert_eq!(blocks.len(), 3, "{html}");
    let [operator, lane, aside] = blocks.as_slice() else {
        unreachable!()
    };
    let attributed = |block: &str, actor: &str| {
        block
            .matches(&format!("data-quote-actor=\"{actor}\""))
            .count()
    };
    assert_eq!(
        (
            attributed(operator, "operator"),
            operator.matches("message-quote-signoff").count()
        ),
        (4, 2),
        "the nested quote and the context each sign once: {operator}"
    );
    assert_eq!(
        (
            attributed(lane, &format!("lane:{directing}")),
            lane.matches("message-quote-signoff").count()
        ),
        (2, 1),
        "{lane}"
    );
    assert_eq!(html.matches("message-quote-name").count(), 3, "{html}");
    let named = format!(
        "class=\"message-quote-name\" data-quote-author=\"directive\" \
         data-quote-actor=\"lane:{directing}\" data-quote-author-state=\"current\" \
         data-accent=\"1\">directing</span>"
    );
    assert!(lane.contains(&named));
    let vacant = render(&BTreeMap::from([agent(None)]));
    assert!(vacant.contains(&named), "{vacant}");
    let unseated = render(&QuoteAgents::new());
    assert!(unseated.contains(&format!(
        "class=\"message-quote-name\" data-quote-author=\"directive\" \
         data-quote-actor=\"lane:{directing}\" data-quote-author-state=\"lane\">lane {}</span>",
        crate::watchband::short(directing),
    )));
    assert!(!operator.contains(">Operator</span>"), "{operator}");
    assert_eq!(html.matches("data-quote-source=").count(), 0, "{html}");
    assert!(html.contains("data-quote-age-authored=\"1000\""));
    assert!(html.contains("data-quote-age-authored=\"2000\""));
    assert!(html.contains("data-quote-age-authored=\"3000\""));
    assert!(!aside.contains("message-quote-signoff"), "{aside}");
}

/// A prose card's attribution leads the card and each answer after a chain of
/// quoted directives, ahead of the answer's pill; a chain answered by a block
/// other than prose, or by nothing, still gets its attribution line.
#[test]
fn prose_card_attribution_leads_each_answer_after_a_quote_chain() {
    let slot = super::SIGNOFF_SLOT;
    let line = format!("<p class=\"message-signoff-line\">{slot}</p>");
    let card = |markdown: &str, contexts: &[&str]| {
        let mut directives = Vec::new();
        let mut classifications = Vec::new();
        for (index, context) in contexts.iter().enumerate() {
            let start = markdown.find(context).expect("fixture context");
            directives.push(MessageDirective {
                start: u32::try_from(start).expect("fixture offset fits"),
                end: u32::try_from(start + context.len()).expect("fixture offset fits"),
                quote_target: steering_target(&format!("dTVQdQfL{index}"), context),
                actor: wire::Actor::Operator,
                authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
            });
        }
        for (start, _) in markdown.match_indices(wire::stream::ACK_MARKER) {
            classifications.push(MessageClassification {
                start: u32::try_from(start).expect("fixture offset fits"),
                end: u32::try_from(start + wire::stream::ACK_MARKER.len())
                    .expect("fixture offset fits"),
                kind: wire::MessageClassificationKind::Acknowledgement,
            });
        }
        render_prose_card(
            markdown,
            &classifications,
            &directives,
            &[],
            &QuoteAgents::new(),
            &QuoteSources::new(),
        )
        .html
    };

    let html = card(
        "> one\n\n> two\n\n[ACK] first answer\n\n> three\n\n- listed",
        &["> one", "> two", "> three"],
    );
    assert_eq!(html.matches(slot).count(), 2, "{html}");
    let first = html.find(slot).expect("first slot");
    let second_quote = html.find("two").expect("second quote");
    let pill = html.find("message-ack-pill").expect("answer pill");
    let answer = html.find("first answer").expect("answer");
    assert!(
        second_quote < first && first < pill && pill < answer,
        "one attribution leads the answer after both quotes: {html}"
    );
    let list = html.find("<ul>").expect("list");
    assert!(
        html.find(&line).is_some_and(|line| line < list),
        "a list answering the last quote gets a leading attribution line: {html}"
    );

    let plain = card("Plain words", &[]);
    assert!(
        plain.starts_with(&format!("<p>{slot}Plain words")),
        "{plain}"
    );
    assert_eq!(plain.matches(slot).count(), 1, "{plain}");

    let unanswered = card("> only", &["> only"]);
    assert!(unanswered.ends_with(&line), "{unanswered}");
    assert_eq!(unanswered.matches(slot).count(), 1, "{unanswered}");
}

#[test]
fn directive_context_ending_in_images_leads_its_flow_with_every_image_once() {
    let text = "phase colors please";
    let render = |quoted: &str| {
        let directives = [MessageDirective {
            start: 0,
            end: u32::try_from(quoted.len()).expect("fixture offset fits"),
            quote_target: steering_target("dTVQdQfLx", text),
            actor: wire::Actor::Operator,
            authored: "1970-01-01T00:00:01.000Z".parse().expect("fixture instant"),
        }];
        render_received_with_quotes(
            &format!("{quoted}\n\nACK dTVQdQfLx: on it."),
            &[],
            &directives,
            &[],
            &QuoteAgents::new(),
            &QuoteSources::new(),
        )
        .html
    };

    let html = render(&format!(
        "> {text}\n>\n> more words\n>\n> ![first](a.png)\n>\n> ![second](b.png)"
    ));
    let quote = html
        .split("<blockquote>")
        .nth(1)
        .and_then(|tail| tail.split("</blockquote>").next())
        .expect("the directive quote renders");
    assert_eq!(
        quote.matches("class=\"message-image-flow\"").count(),
        1,
        "{html}"
    );
    assert_eq!(
        quote.matches("class=\"message-image-stack\"").count(),
        1,
        "{html}"
    );
    assert_eq!(quote.matches("src=\"a.png\"").count(), 1, "{html}");
    assert_eq!(quote.matches("src=\"b.png\"").count(), 1, "{html}");
    assert_eq!(
        quote.matches("class=\"message-quote-signoff\"").count(),
        1,
        "{html}"
    );
    let stack = quote.find("message-image-stack").expect("stack");
    let signoff = quote.find("message-quote-signoff").expect("signoff");
    let first = quote.find(text).expect("first paragraph");
    let last = quote.find("more words").expect("last paragraph");
    assert!(stack < signoff && signoff < first && first < last, "{html}");
    assert!(
        !quote.contains("message-quote-signoff-line"),
        "the signoff leads the first prose paragraph: {html}"
    );

    let images_only = render("> ![first](a.png)\n>\n> ![second](b.png)");
    assert!(!images_only.contains("message-image-flow"), "{images_only}");
    assert_eq!(
        images_only.matches("class=\"message-image-stack\"").count(),
        2
    );
    let line = images_only
        .find("message-quote-signoff-line")
        .expect("attribution line");
    let stack = images_only.find("message-image-stack").expect("stack");
    assert!(
        line < stack,
        "an image-only context leads with its own attribution line: {images_only}"
    );
}

#[test]
fn quote_cards_ending_in_images_lead_with_their_attribution_once() {
    let render = |text: &str| {
        let target = steering_target("dTVQdQfLw", text);
        let quotes = [wire::QuotePresentation::Resolved {
            target: target.to_string(),
            text: text.to_owned(),
            author: wire::QuoteAuthor::Directive {
                actor: wire::Actor::Operator,
            },
            disposition: Some(wire::Disposition::Acked),
            quotes: Vec::new(),
        }];
        render_received_with_quotes(
            &target.markdown(),
            &[],
            &[],
            &quotes,
            &QuoteAgents::new(),
            &QuoteSources::new(),
        )
        .html
    };

    let html = render("see these\n\n![first](a.png)\n\n![second](b.png)");
    assert_eq!(
        html.matches("class=\"message-image-flow\"").count(),
        1,
        "{html}"
    );
    assert_eq!(
        html.matches("class=\"message-quote-signoff\"").count(),
        1,
        "{html}"
    );
    assert_eq!(
        html.matches("data-quote-disposition=\"acked\"").count(),
        1,
        "{html}"
    );
    let flow = html.find("message-image-flow").expect("flow");
    let stack = html.find("message-image-stack").expect("stack");
    let signoff = html.find("message-quote-signoff").expect("signoff");
    let pill = html.find("data-quote-disposition").expect("pill");
    let prose = html.find("see these").expect("prose");
    assert!(
        flow < stack && stack < signoff && signoff < pill && pill < prose,
        "{html}"
    );

    let images_only = render("![first](a.png)\n\n![second](b.png)");
    assert!(!images_only.contains("message-image-flow"), "{images_only}");
    let line = images_only
        .find("class=\"message-quote-signoff-line\"")
        .expect("attribution line");
    let signoff = images_only
        .find("class=\"message-quote-signoff\"")
        .expect("signoff");
    let pill = images_only.find("data-quote-disposition").expect("pill");
    let image = images_only.find("src=\"a.png\"").expect("image");
    assert!(
        line < signoff && signoff < pill && pill < image,
        "{images_only}"
    );
}
