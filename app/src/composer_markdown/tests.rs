use super::*;

const OPENABLE_PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

fn steering_target(key: &str, text: &str) -> wire::QuoteTarget {
    wire::QuoteTarget::Steering {
        key: key.to_owned(),
        digest: wire::QuoteTarget::text_digest(text),
    }
}

fn tape_target(text: &str) -> wire::QuoteTarget {
    wire::QuoteTarget::Tape {
        lane: "018f82d4-6f59-7d06-92e3-bce9131163e6"
            .parse()
            .expect("fixture lane is canonical"),
        source: "018f82d4-6f59-7d06-92e3-bce9131163e7"
            .parse()
            .expect("fixture source is canonical"),
        locus: wire::MessageLocus {
            member: 1,
            offset: 24,
            length: 80,
            event: 2,
        },
        digest: wire::QuoteTarget::text_digest(text),
    }
}

fn quoted<'a>(values: &'a [(String, &'a str)]) -> Vec<QuotedReply<'a>> {
    values
        .iter()
        .map(|(target, body)| QuotedReply {
            target,
            body,
            attachments: &[],
        })
        .collect()
}

#[test]
fn composed_markdown_emits_canonical_target_links_in_visible_shard_order() {
    assert_eq!(compose("  body only  ", &[], &[]), "body only");
    let tape = tape_target("agent source text");
    let directive = steering_target("dTVQdQfLw", "directive source text");
    let quotes = [(tape.to_string(), ""), (directive.to_string(), "reply")];
    assert_eq!(
        compose("body", &quoted(&quotes), &[]),
        format!(
            "{}\n\n{}\n\nreply\n\nbody",
            tape.markdown(),
            directive.markdown()
        )
    );
}

#[test]
fn operator_typed_block_quotes_remain_ordinary_composed_markdown() {
    let typed = "> copied words\n>\n> still operator-authored";
    assert_eq!(compose(typed, &[], &[]), typed);
}

#[test]
fn block_quotes_normalize_every_line_ending_before_prefixing() {
    assert_eq!(block_quote("first\r\nsecond"), "> first\n> second");
    assert_eq!(block_quote("first\rsecond"), "> first\n> second");
    assert_eq!(block_quote("first\n\nthird"), "> first\n> \n> third");
}

#[test]
fn a_quote_that_ends_on_a_blank_line_keeps_the_marker_on_every_line() {
    let quote = block_quote("first\n\n");

    assert_eq!(quote, "> first");
    assert!(
        quote.lines().all(|line| line.starts_with("> ")),
        "every line of a block quote carries the marker, got {quote:?}"
    );
}

#[test]
fn whitespace_only_operator_block_quote_text_composes_to_nothing() {
    assert_eq!(block_quote("   \n\t\n  "), "");
    assert_eq!(block_quote(""), "");
}

#[test]
fn every_task_handle_occurrence_renders_in_source_order() {
    let control = "CONTROL-1kKbC3j4"
        .parse::<wire::TaskHandle>()
        .expect("fixture handle parses");
    let mosaic = "MOSAIC-1kRYqQxy"
        .parse::<wire::TaskHandle>()
        .expect("fixture handle parses");
    let identity = "IDENTIT-dRyDYZqJJ"
        .parse::<wire::TaskHandle>()
        .expect("current fixture handle parses");
    let control_chip = task_chip_html(&control);
    let mosaic_chip = task_chip_html(&mosaic);
    let identity_chip = task_chip_html(&identity);

    let rendered = render_received(
        "Plain CONTROL-1kKbC3j4, CONTROL-1kKbC3j4, and `CONTROL-1kKbC3j4`.\n\n\
             Then MOSAIC-1kRYqQxy, IDENTIT-dRyDYZqJJ, and CONTROL-1kKbC3j4!",
    );

    assert_eq!(
        rendered.html,
        format!(
            "<p>Plain {control_chip}, {control_chip}, and {control_chip}.</p>\n\
                 <p>Then {mosaic_chip}, {identity_chip}, and {control_chip}!</p>\n"
        ),
    );
    assert_eq!(
        rendered.html.matches("class=\"message-task-chip\"").count(),
        6
    );
}

#[test]
fn quoted_first_and_nested_mentions_do_not_consume_later_prose_occurrences() {
    let rendered = render_received(
        "> Quoted CONTROL-1kKbC3j4.\n\
             > > Nested `CONTROL-1kKbC3j4`.\n\n\
             Authored CONTROL-1kKbC3j4 and CONTROL-1kKbC3j4.",
    );
    let marker = "data-task-handle=\"CONTROL-1kKbC3j4\"";

    assert_eq!(
        rendered.html.matches(marker).count(),
        4,
        "{}",
        rendered.html
    );
    assert_eq!(rendered.html.matches("<blockquote>").count(), 2);
    assert!(rendered.html.contains("Quoted "));
    assert!(rendered.html.contains("Nested "));
    assert!(rendered.html.contains("Authored "));
    assert!(
        rendered.html.rfind(marker).expect("authored chip renders")
            > rendered
                .html
                .rfind("</blockquote>")
                .expect("quoted context closes"),
        "the authored occurrence follows the quoted context: {}",
        rendered.html,
    );
}

#[test]
fn media_expansion_remaps_the_exact_repeated_marker_classification() {
    let media =
        Media::from_base64("capture.png", "image/png", "iVBORw0KGgo=").expect("fixture media");
    let markdown = format!(
        "{}\n\n> [ACK] remains literal.\n\n> [ACK] is authenticated.",
        media.reference_markdown().expect("media reference")
    );
    let start = markdown.rfind("[ACK]").expect("second marker exists");
    let classification = MessageClassification {
        start: u32::try_from(start).expect("fixture offset fits"),
        end: u32::try_from(start + wire::stream::ACK_MARKER.len()).expect("fixture offset fits"),
        kind: MessageClassificationKind::Acknowledgement,
    };

    let classifications = [classification];
    let ExpandedMedia {
        markdown: expanded,
        classifications: remapped,
        ..
    } = expand_media_payloads(
        &markdown,
        std::slice::from_ref(&media),
        &classifications,
        &[],
    )
    .expect("media expands");
    assert!(wire::valid_message_classifications(&expanded, &remapped));
    let rendered = render_received_with_classifications(&expanded, &remapped);
    assert_eq!(
        rendered.html.matches("class=\"message-ack-pill\"").count(),
        1
    );
    assert!(rendered.html.contains("[ACK] remains literal."));
    assert!(rendered.html.contains("is authenticated."));
}

#[test]
fn authenticated_classification_markers_become_inline_pills() {
    let rendered = render_received_with_fixture_classifications(
        "> [REVIEW] Peer feedback for CANVAS-1kKRRsDR:\n>\n\
             > > missing current coverage\n>\n\
             > Keep working the current task.\n\n\
             [REVIEW] Ordinary prose remains unchanged.\n\n\
             > [REVIEW] A second quoted marker remains unchanged.\n\n\
             > `[REVIEW]` remains inline code.\n\n\
             > ```text\n> [REVIEW] remains fenced code\n> ```\n\n\
             > [TASTE] compatibility: Keep one tested path.\n\n\
             > > [TASTE] fallbacks: Nested guidance keeps its own accent.\n\n\
             > [TASTE] polling: Wait for completion.\n\n\
             > [BROADCAST] Send this to every occupied agent.\n\n\
             ACK: ordinary [TASTE] prose stays text.",
        TestClassifications {
            acknowledgements: 0,
            tastes: 3,
            broadcasts: 1,
            reviews: 1,
            pending: 0,
            declines: 0,
        },
    );
    assert!(!rendered.html.contains("compatibility:"));
    assert!(!rendered.html.contains("fallbacks:"));
    assert!(!rendered.html.contains("polling:"));
    assert!(rendered.html.contains("Keep one tested path."));
    assert!(
        rendered
            .html
            .contains("Nested guidance keeps its own accent.")
    );
    assert!(rendered.html.contains("Wait for completion."));
    assert!(rendered.html.contains("Peer feedback for "));
    assert!(
        rendered
            .html
            .contains("data-task-handle=\"CANVAS-1kKRRsDR\"")
    );
    assert!(rendered.html.contains("missing current coverage"));
    assert!(rendered.html.contains("Keep working the current task."));
    assert!(!rendered.html.contains("[REVIEW] Peer feedback"));
    assert!(
        rendered
            .html
            .contains("[REVIEW] Ordinary prose remains unchanged.")
    );
    assert!(
        rendered
            .html
            .contains("[REVIEW] A second quoted marker remains unchanged.")
    );
    assert!(rendered.html.contains("<code>[REVIEW]</code>"));
    assert!(rendered.html.contains("[REVIEW] remains fenced code"));
    assert!(rendered.html.contains("class=\"message-review-pill\""));
    assert!(
        rendered
            .html
            .contains("aria-label=\"Review classification\">REVIEW</span>")
    );
    assert_eq!(rendered.html.matches("message-review-pill").count(), 1);

    assert_eq!(
        rendered
            .html
            .matches("class=\"message-broadcast-pill\"")
            .count(),
        1,
    );

    assert_eq!(
        rendered
            .html
            .matches("class=\"message-taste-pill\"")
            .count(),
        3,
        "{}",
        rendered.html,
    );
    assert_eq!(
        rendered
            .html
            .matches("aria-label=\"Taste classification\">TASTE</span>")
            .count(),
        3,
    );
    assert!(
        rendered
            .html
            .contains("ACK: ordinary [TASTE] prose stays text.")
    );

    let unauthenticated = render_received("> [REVIEW] Quoted prose is not metadata.");
    assert!(!unauthenticated.html.contains("message-review-pill"));
    assert!(
        unauthenticated
            .html
            .contains("[REVIEW] Quoted prose is not metadata.")
    );
}

#[test]
fn broadcast_marker_is_idempotent_and_requires_authenticated_quote_metadata() {
    let marked = mark_broadcast("Send this to every occupied agent.");
    assert_eq!(marked, "[BROADCAST] Send this to every occupied agent.");
    assert_eq!(mark_broadcast(&marked), marked);

    let classified = render_received_with_fixture_classifications(
        "> [BROADCAST] Send this to every occupied agent.\n\n\
             > [BROADCAST] A second marker remains literal.\n\n\
             `[BROADCAST]` remains code.",
        TestClassifications {
            broadcasts: 1,
            ..TestClassifications::default()
        },
    );
    assert_eq!(
        classified
            .html
            .matches("class=\"message-broadcast-pill\"")
            .count(),
        1,
    );
    assert!(
        classified
            .html
            .contains("Send this to every occupied agent.")
    );
    assert!(
        classified
            .html
            .contains("[BROADCAST] A second marker remains literal.")
    );
    assert!(classified.html.contains("<code>[BROADCAST]</code>"));

    let unclassified = render_received("> [BROADCAST] Remains ordinary quoted prose.");
    assert!(!unclassified.html.contains("message-broadcast-pill"));
    assert!(
        unclassified
            .html
            .contains("[BROADCAST] Remains ordinary quoted prose.")
    );
}

#[test]
fn acknowledged_context_markers_render_static_pills_inside_the_quote() {
    let rendered = render_received_with_fixture_classifications(
        "> [ACK] Keep the current path.\n\n\
             Applied.\n\n\
             > [ACK] A second marker remains literal.\n\n\
             [ACK] Ordinary prose remains literal.\n\n\
             > `[ACK]` remains inline code.\n\n\
             > ```text\n> [ACK] remains fenced code\n> ```",
        TestClassifications {
            acknowledgements: 1,
            ..TestClassifications::default()
        },
    );
    assert_eq!(
        rendered.html.matches("class=\"message-ack-pill\"").count(),
        1
    );
    assert!(rendered.html.contains("data-acknowledgement=\"acked\""));
    assert!(!rendered.html.contains("message-classifiers"));
    assert!(!rendered.html.contains("message-badge-count"));
    let pill = rendered
        .html
        .find("class=\"message-ack-pill\"")
        .expect("acknowledgement pill renders synchronously");
    let quote = rendered
        .html
        .find("<blockquote>")
        .expect("quoted context renders");
    assert!(
        quote < pill,
        "the pill nests inside the quote: {}",
        rendered.html
    );
    assert!(
        rendered
            .html
            .contains("<span class=\"message-ack-label\">ACK</span></span> Keep the current path.")
    );
    assert!(
        rendered
            .html
            .contains("[ACK] A second marker remains literal.")
    );
    assert!(
        rendered
            .html
            .contains("[ACK] Ordinary prose remains literal.")
    );
    assert!(rendered.html.contains("<code>[ACK]</code>"));
    assert!(rendered.html.contains("[ACK] remains fenced code"));

    let unclassified = render_received("> [ACK] Remains ordinary quoted prose.");
    assert!(!unclassified.html.contains("message-ack-pill"));
    assert!(
        unclassified
            .html
            .contains("[ACK] Remains ordinary quoted prose.")
    );
}

#[test]
fn bare_acknowledged_context_marker_heads_a_nested_operator_quote() {
    let rendered = render_received_with_fixture_classifications(
        "> [ACK]\n>\n> > The original assertion remains visible.\n>\n> approved\n\nApplied.",
        TestClassifications {
            acknowledgements: 1,
            ..TestClassifications::default()
        },
    );
    assert_eq!(
        rendered.html.matches("class=\"message-ack-pill\"").count(),
        1
    );
    assert!(
        rendered
            .html
            .contains("<span class=\"message-ack-label\">ACK</span></span></p>")
    );
    assert!(
        rendered.html.contains(
            "<blockquote>\n<p>The original assertion remains visible.</p>\n</blockquote>"
        )
    );
    assert!(rendered.html.contains("<p>approved</p>"));
}

#[test]
fn a_marker_quoted_back_from_another_card_never_takes_this_classification() {
    let rendered = render_received_with_fixture_classifications(
        "> > > [ACK] Tell me what the task is even about\n\
             > > \n\
             > > GIT-dSfJfCXfp is the task-git admission-depth task.\n\
             >\n\
             > [ACK] Delete it\n\n\
             Deleting GIT-dSfJfCXfp as directed.",
        TestClassifications {
            acknowledgements: 1,
            ..TestClassifications::default()
        },
    );
    assert_eq!(
        rendered.html.matches("class=\"message-ack-pill\"").count(),
        1
    );
    assert!(
        rendered
            .html
            .contains("<span class=\"message-ack-label\">ACK</span></span> Delete it</p>"),
        "the pill lands on the operator's own words: {}",
        rendered.html
    );
    assert!(
        rendered
            .html
            .contains("[ACK] Tell me what the task is even about"),
        "the deeper marker stays literal: {}",
        rendered.html
    );
}

#[test]
fn flavored_acknowledgements_keep_their_own_pill_as_the_mark() {
    let rendered = render_received_with_fixture_classifications(
        "> [TASTE] polling: Wait for completion.\n\n\
             > [BROADCAST] Send this to every occupied agent.\n\n\
             > [ACK] Stray marker stays literal.",
        TestClassifications {
            acknowledgements: 2,
            tastes: 1,
            broadcasts: 1,
            reviews: 0,
            pending: 0,
            declines: 0,
        },
    );
    assert_eq!(
        rendered
            .html
            .matches("class=\"message-taste-pill\"")
            .count(),
        1
    );
    assert_eq!(
        rendered
            .html
            .matches("class=\"message-broadcast-pill\"")
            .count(),
        1
    );
    assert!(!rendered.html.contains("message-ack-pill"));
    assert!(rendered.html.contains("[ACK] Stray marker stays literal."));

    let mixed = render_received_with_fixture_classifications(
        "> [TASTE] polling: Wait for completion.\n\n> [ACK] Keep the current path.",
        TestClassifications {
            acknowledgements: 2,
            tastes: 1,
            ..TestClassifications::default()
        },
    );
    assert_eq!(
        mixed.html.matches("class=\"message-taste-pill\"").count(),
        1
    );
    assert_eq!(mixed.html.matches("class=\"message-ack-pill\"").count(), 1);
}

#[test]
fn acknowledgement_receipts_keep_one_stable_measured_shape() {
    let render = |marker, requested| {
        render_received_with_fixture_classifications(
            &format!(
                "> {marker} Keep the current path.\n\n\
                     ```mermaid\n\
                     graph LR\n\
                       A[Receipt] --> B[Stable card]\n\
                     ```"
            ),
            requested,
        )
    };
    let pending = render(
        "[ACK PENDING]",
        TestClassifications {
            pending: 1,
            ..TestClassifications::default()
        },
    );
    let retired = render(
        "[ACK]",
        TestClassifications {
            acknowledgements: 1,
            ..TestClassifications::default()
        },
    );
    let declined = render(
        "[NACK]",
        TestClassifications {
            declines: 1,
            ..TestClassifications::default()
        },
    );

    assert!(pending.html.contains("data-acknowledgement=\"pending\""));
    assert!(retired.html.contains("data-acknowledgement=\"acked\""));
    assert!(declined.html.contains("data-acknowledgement=\"declined\""));
    assert!(same_measured_shape(&pending, &retired));
    assert!(
        !same_measured_shape(&pending, &declined),
        "NACK carries distinct measured text"
    );
    assert_eq!(
        pending.html.matches("class=\"message-ack-pill\"").count(),
        1
    );
    assert_eq!(
        pending.diagrams,
        vec!["graph LR\nA[Receipt] --> B[Stable card]\n"]
    );
    assert_eq!(pending.diagrams, retired.diagrams);
    assert_eq!(pending.diagrams, declined.diagrams);
    assert_eq!(pending.image_only, retired.image_only);
    assert_eq!(pending.image_only, declined.image_only);
}

#[test]
fn taste_metadata_neither_overreaches_nor_decorates_code() {
    let one = render_received_with_tastes(
        "> `[TASTE]` remains code.\n\n\
             > [TASTE] first.\n\n\
             > [TASTE] second remains literal.",
        1,
    );
    assert_eq!(one.html.matches("class=\"message-taste-pill\"").count(), 1);
    assert!(one.html.contains("<code>[TASTE]</code>"));
    assert!(one.html.contains("[TASTE] second remains literal."));

    let none = render_received_with_tastes("> [TASTE] untouched.", 0);
    assert!(!none.html.contains("message-taste-pill"));
    assert!(none.html.contains("[TASTE] untouched."));

    let token = render_received_with_tastes("> [TASTE]ful is not a marker.", 1);
    assert!(!token.html.contains("message-taste-pill"));
    assert!(token.html.contains("[TASTE]ful is not a marker."));

    let prefixed =
        render_received_with_tastes("> prefix[TASTE] stays text.\n\n> [TASTE] actual.", 1);
    assert_eq!(
        prefixed
            .html
            .matches("class=\"message-taste-pill\"")
            .count(),
        1,
    );
    assert!(prefixed.html.contains("prefix[TASTE] stays text."));
}

#[test]
fn noncanonical_and_block_code_handles_remain_ordinary_markdown() {
    let rendered = render_received("`APP-a1` and APP-a1.\n\n```text\nCONTROL-1kKbC3j4\n```");

    assert!(!rendered.html.contains("message-task-chip"));
    assert!(
        rendered.diagrams.is_empty(),
        "ordinary code must not request diagram rendering"
    );
    assert!(rendered.html.contains("<code>APP-a1</code> and APP-a1."));
    assert!(rendered.html.contains("CONTROL-1kKbC3j4"));
}

#[test]
fn every_image_only_markdown_shape_renders_one_card_wide_stack() {
    let image = |name: &str| format!("![{name}]({OPENABLE_PNG})");
    let shapes = [
        [image("one"), image("two"), image("three")].join(" "),
        [image("one"), image("two"), image("three")].join("\n"),
        [image("one"), image("two"), image("three")].join("  \n"),
        [image("one"), image("two"), image("three")].join("\n\n"),
    ];

    for markdown in shapes {
        let rendered = render_received(&markdown);
        assert!(
            rendered.image_only,
            "shape must receive the large tier: {markdown:?}"
        );
        assert_eq!(
            rendered
                .html
                .matches("class=\"message-image-stack\"")
                .count(),
            1,
            "all image blocks must share one stack: {}",
            rendered.html,
        );
        assert_eq!(
            rendered
                .html
                .matches("class=\"message-image message-image--openable\"")
                .count(),
            3,
            "every preview must remain present: {}",
            rendered.html,
        );
        assert!(!rendered.html.contains("<a "));
    }
}

#[test]
fn image_actions_preserve_the_stack_without_admitting_script_urls() {
    let rendered = render_received(
        "![remote](https://example.test/image.png)\n\n\
             ![vector](data:image/svg+xml;base64,PHN2Zz4=)\n\n\
             ![refused](javascript:alert(1))",
    );

    assert!(rendered.image_only);
    assert_eq!(
        rendered
            .html
            .matches("class=\"message-image-stack\"")
            .count(),
        1
    );
    assert_eq!(rendered.html.matches("<button ").count(), 2);
    assert_eq!(
        rendered
            .html
            .matches("<span class=\"message-image\"")
            .count(),
        1
    );
    assert!(!rendered.html.contains("javascript:"));
}

#[test]
fn linked_images_keep_one_image_action_without_navigation() {
    let image_only = render_received(
        "[![one](https://images.example.test/one.png)](https://links.example.test/one)\n\n\
             [![two](https://images.example.test/two.png)](https://links.example.test/two)",
    );

    assert!(image_only.image_only);
    assert_eq!(
        image_only
            .html
            .matches("class=\"message-image-stack\"")
            .count(),
        1
    );
    assert_eq!(image_only.html.matches("<button ").count(), 2);
    assert!(!image_only.html.contains("<a "));
    assert!(
        image_only
            .html
            .contains("src=\"https://images.example.test/one.png\"")
    );
    assert!(
        image_only
            .html
            .contains("src=\"https://images.example.test/two.png\"")
    );
    assert!(!image_only.html.contains("links.example.test"));

    let mixed = render_received(
        "See [![preview](https://images.example.test/mixed.png)](https://links.example.test/mixed) now.",
    );
    assert!(!mixed.image_only);
    assert_eq!(mixed.html.matches("<button ").count(), 1);
    assert!(!mixed.html.contains("<a "));
    assert!(
        mixed
            .html
            .contains("src=\"https://images.example.test/mixed.png\"")
    );
    assert!(!mixed.html.contains("links.example.test"));
}

#[test]
fn prose_and_images_keep_the_ordinary_rendering_path() {
    let rendered = render_received("ordinary prose\n\n![preview](https://example.test/image.png)");

    assert!(!rendered.image_only);
    assert!(rendered.html.starts_with("<p>ordinary prose</p>\n"));
    assert_eq!(
        rendered
            .html
            .matches("class=\"message-image-stack\"")
            .count(),
        1
    );
    assert!(!rendered.html.contains("<a "));
}

#[test]
fn every_image_preview_owns_an_explicit_initial_lifecycle() {
    let remote = render_received("![remote](https://example.test/image.png)");
    assert!(remote.html.contains("data-mosaic-image-state=\"pending\""));

    let cached = render_received(&format!("![cached]({OPENABLE_PNG})"));
    assert!(cached.html.contains("data-mosaic-image-state=\"pending\""));
    assert_eq!(cached.html.matches("data-mosaic-image-status=").count(), 1);

    let malformed = render_received("![broken](data:image/png;base64,%%%)");
    assert!(malformed.html.contains("data-mosaic-image-state=\"error\""));
}

fn quote_fixture_id(tail: &str) -> wire::CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail}")
        .parse()
        .expect("fixture identity is canonical")
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
                card_id: "message-agent-outer-source".to_owned(),
                agent: Some(agent),
            },
        ),
        (
            inner.to_string(),
            QuoteSource {
                card_id: "message-agent-inner-source".to_owned(),
                agent: None,
            },
        ),
    ])
}

fn assert_source_author_survives_reseating(
    outer: &wire::QuoteTarget,
    quotes: &[wire::QuotePresentation],
    sources: &QuoteSources,
    lane: wire::CanonicalUuid,
    source: wire::CanonicalUuid,
    team: wire::CanonicalUuid,
) {
    let successor = quote_fixture_id("e9");
    let agents = BTreeMap::from([(
        (lane, successor),
        QuoteAgent {
            team,
            accent: 4,
            label: "spice-rust-k".to_owned(),
        },
    )]);
    let rendered = render_received_with_quotes(
        &outer.markdown(),
        &[],
        &[],
        quotes,
        &agents,
        sources,
        &BTreeMap::new(),
    );
    assert!(rendered.html.contains(&format!(
        "data-quote-agent-occupant=\"{source}\" \
         data-quote-author-state=\"source\" data-accent=\"3\""
    )));
    assert!(rendered.html.contains(">spice-rust-k</span>"));
    assert!(!rendered.html.contains("history unavailable"));
    assert!(!rendered.html.contains("data-quote-agent-team="));
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

    let team: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163e8".parse().unwrap();
    let agents = BTreeMap::from([(
        (lane, source),
        QuoteAgent {
            team,
            accent: 3,
            label: "spice-rust-k".to_owned(),
        },
    )]);
    let sources = nested_quote_sources(
        &outer_target,
        &inner_target,
        agents
            .get(&(lane, source))
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
        &BTreeMap::new(),
    );

    assert_eq!(
        rendered
            .html
            .matches("data-quote-state=\"resolved\"")
            .count(),
        2
    );
    assert!(rendered.html.contains("data-quote-author=\"agent\""));
    assert!(rendered.html.contains(&format!(
        "data-quote-agent-team=\"{team}\" \
         data-quote-agent-lane=\"{lane}\" data-quote-agent-occupant=\"{source}\" \
         data-quote-author-state=\"current\" data-accent=\"3\" \
         aria-label=\"Open information for spice-rust-k\">spice-rust-k</button>"
    )));
    assert!(
        rendered.html.contains(
            "data-quote-author=\"directive\" data-quote-actor=\"operator\">Operator</span>"
        )
    );
    assert!(!rendered.html.contains("history unavailable"));
    assert_eq!(rendered.html.matches("message-quote-source").count(), 2);
    assert!(rendered.html.contains(
        "href=\"#/card/message-agent-outer-source\" \
         data-quote-source=\"message-agent-outer-source\""
    ));
    assert!(rendered.html.contains(
        "href=\"#/card/message-agent-inner-source\" \
         data-quote-source=\"message-agent-inner-source\""
    ));
    assert!(rendered.html.contains(inner_text));
    assert!(rendered.html.contains("data-quote-disposition=\"live\""));
    assert!(!rendered.html.contains("Agent "));
    assert!(!rendered.html.contains("Directive by"));
    let outer_body = rendered
        .html
        .find("before ")
        .expect("outer authenticated prose renders");
    let outer_signoff = rendered
        .html
        .rfind("spice-rust-k</button>")
        .expect("outer signoff renders");
    assert!(
        outer_body < outer_signoff,
        "the signoff trails authored prose"
    );

    assert_source_author_survives_reseating(&outer_target, &quotes, &sources, lane, source, team);
}

#[test]
fn directive_quote_cards_cover_every_disposition_and_board_backed_navigation() {
    let cases = [
        (wire::Disposition::Live, "live"),
        (wire::Disposition::Acked, "acked"),
        (wire::Disposition::Declined, "nacked"),
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
    let handle: wire::TaskHandle = "QUOTE-dTVQdQfLw"
        .parse()
        .expect("fixture task is canonical");
    let tasks = BTreeMap::from([("dTVQdQfLw".to_owned(), vec![handle])]);
    let rendered = render_received_with_quotes(
        &markdown.join(" "),
        &[],
        &[],
        &quotes,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &tasks,
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
        "ACK and NACK reuse the canonical acknowledgement pill"
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
    assert_eq!(html.matches("class=\"message-quote-signoff\"").count(), 7);
    assert_eq!(
        html.matches("data-quote-actor=\"operator\">Operator</span>")
            .count(),
        6
    );
    assert!(
        html.contains("data-quote-actor=\"spice\" role=\"img\" aria-label=\"Spice\">🌶️</span>")
    );
    assert!(!html.contains("history unavailable"));
    assert!(!html.contains("Directive by"));
    assert!(!html.contains("class=\"message-quote-disposition tag"));
    assert_eq!(html.matches("class=\"message-quote-task\"").count(), 1);
    assert!(html.contains("data-task-handle=\"QUOTE-dTVQdQfLw\""));
    assert!(html.contains("aria-label=\"Open task QUOTE-dTVQdQfLw in Work\""));
}

#[test]
fn tasked_quote_levels_keep_every_exact_task_at_every_occurrence() {
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
    let outer_tasks = [
        "FIRST-dTZQjKHhG"
            .parse()
            .expect("fixture task is canonical"),
        "SECOND-dTZRfCQ43"
            .parse()
            .expect("fixture task is canonical"),
    ];
    let inner_task: wire::TaskHandle = "INNER-dTZRdv3bF"
        .parse()
        .expect("fixture task is canonical");
    let tasks = BTreeMap::from([
        ("dTVQdQfLw".to_owned(), outer_tasks.to_vec()),
        ("dTVQdQfLx".to_owned(), vec![inner_task.clone()]),
    ]);
    let rendered = render_received_with_quotes(
        &format!("{} {}", outer_target.markdown(), outer_target.markdown()),
        &[],
        &[],
        &[outer.clone(), outer],
        &BTreeMap::new(),
        &BTreeMap::new(),
        &tasks,
    );

    assert_eq!(
        rendered
            .html
            .matches("class=\"message-quote-task\"")
            .count(),
        6,
        "two outer occurrences each keep both tasks and their nested task"
    );
    for handle in outer_tasks {
        assert_eq!(
            rendered
                .html
                .matches(&format!("data-task-handle=\"{handle}\""))
                .count(),
            2,
            "{handle} renders once for each occurrence of its own level"
        );
    }
    assert_eq!(
        rendered
            .html
            .matches(&format!("data-task-handle=\"{inner_task}\""))
            .count(),
        2,
        "the nested task renders once within each repeated outer quote"
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
        team: id("ff"),
        accent: 2,
        label: label.to_owned(),
    };
    let roster = BTreeMap::from([
        ((quoted, successor), agent("quoted")),
        ((directing, id("2b")), agent("directing")),
        ((unrelated, id("2c")), agent("unrelated")),
    ]);
    let render = |agents: &QuoteAgents| {
        render_received_with_quotes(
            &outer_target.markdown(),
            &[],
            &[],
            &quotes,
            agents,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .html
    };

    let projection = referenced_quote_agents(&quotes, &[], &roster);
    assert_eq!(render(&projection), render(&roster));

    let mut reseated = roster.clone();
    reseated.remove(&(unrelated, id("2c")));
    reseated.insert((unrelated, id("3c")), agent("unrelated"));
    reseated.insert((arriving, id("3d")), agent("arriving"));
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
    let tasks = BTreeMap::from([(
        "dTVQdQfLw".to_owned(),
        vec![
            "QUOTE-dTVQdQfLw"
                .parse()
                .expect("fixture task is canonical"),
        ],
    )]);
    let rendered = render_received_with_quotes(
        &target.markdown(),
        &[],
        &[],
        &unavailable,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &tasks,
    );

    assert!(rendered.html.contains("Quote unavailable: depth-exceeded"));
    assert!(rendered.html.contains("authorship unavailable"));
    assert!(!rendered.html.contains("message-quote-signoff"));
    assert!(!rendered.html.contains("<a "));
    assert!(!rendered.html.contains("message-quote-task"));
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
    let context = |start: usize, len: usize, key: &str, text: &str, actor| MessageDirective {
        start: u32::try_from(start).expect("fixture offset fits"),
        end: u32::try_from(start + len).expect("fixture offset fits"),
        quote_target: steering_target(key, text),
        actor,
    };
    let directives = [
        context(
            0,
            operator.len(),
            "dTVQdQfLx",
            &operator_text,
            wire::Actor::Operator,
        ),
        context(
            operator.len() + 2,
            lane.len(),
            "dTVQdQfLy",
            lane_text,
            wire::Actor::Lane(directing),
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
    let agents = BTreeMap::from([(
        (directing, directing),
        QuoteAgent {
            team: directing,
            accent: 1,
            label: "directing".to_owned(),
        },
    )]);
    let sources = BTreeMap::from([
        (
            directives[0].quote_target.to_string(),
            QuoteSource {
                card_id: "message-agent-operator-source".to_owned(),
                agent: None,
            },
        ),
        (
            directives[1].quote_target.to_string(),
            QuoteSource {
                card_id: "message-agent-lane-source".to_owned(),
                agent: None,
            },
        ),
        (
            nested.to_string(),
            QuoteSource {
                card_id: "message-agent-nested-source".to_owned(),
                agent: None,
            },
        ),
    ]);

    let html = render_received_with_quotes(
        &markdown,
        &[],
        &directives,
        &quotes,
        &agents,
        &sources,
        &BTreeMap::new(),
    )
    .html;

    let blocks = html.split("<blockquote>").skip(1).collect::<Vec<_>>();
    assert_eq!(blocks.len(), 3, "{html}");
    let [operator, lane, aside] = blocks.as_slice() else {
        unreachable!()
    };
    let signed = |block: &str, actor: &str| {
        block
            .matches(&format!("data-quote-actor=\"{actor}\""))
            .count()
    };
    assert_eq!(
        (
            signed(operator, "operator"),
            operator.matches("message-quote-signoff").count()
        ),
        (2, 2),
        "the nested quote and the context each sign once: {operator}"
    );
    assert_eq!(
        (
            signed(lane, &format!("lane:{directing}")),
            lane.matches("message-quote-signoff").count()
        ),
        (1, 1),
        "{lane}"
    );
    assert!(lane.contains(">directing</span>"), "{lane}");
    assert!(lane.contains("data-accent=\"1\""), "{lane}");
    assert_eq!(html.matches("message-quote-source").count(), 3, "{html}");
    assert!(html.contains("href=\"#/card/message-agent-operator-source\""));
    assert!(html.contains("href=\"#/card/message-agent-lane-source\""));
    assert!(html.contains("href=\"#/card/message-agent-nested-source\""));
    assert!(!aside.contains("message-quote-signoff"), "{aside}");
}
