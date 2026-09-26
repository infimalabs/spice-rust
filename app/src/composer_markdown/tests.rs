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
    let (expanded, remapped) =
        expand_media_payloads(&markdown, std::slice::from_ref(&media), &classifications)
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

#[test]
fn resolved_quote_links_render_complete_nested_cards_synchronously() {
    let inner_text = "authenticated inner direction";
    let inner_target = steering_target("dTVQdQfLw", inner_text);
    let outer_text = format!("before {} after", inner_target.markdown());
    let lane = "018f82d4-6f59-7d06-92e3-bce9131163e6"
        .parse()
        .expect("fixture lane is canonical");
    let source = "018f82d4-6f59-7d06-92e3-bce9131163e7"
        .parse()
        .expect("fixture source is canonical");
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

    let rendered =
        render_received_with_quotes(&outer_target.markdown(), &[], &quotes, &BTreeMap::new());

    assert_eq!(
        rendered
            .html
            .matches("data-quote-state=\"resolved\"")
            .count(),
        2
    );
    assert!(rendered.html.contains("data-quote-author=\"agent\""));
    assert!(rendered.html.contains("data-quote-author=\"directive\""));
    assert!(rendered.html.contains(inner_text));
    assert!(rendered.html.contains("data-quote-disposition=\"live\""));
    assert!(!rendered.html.contains("<a "));
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
    let handle: wire::TaskHandle = "QUOTE-dTVQdQfLw"
        .parse()
        .expect("fixture task is canonical");
    let tasks = BTreeMap::from([("dTVQdQfLw".to_owned(), handle)]);
    let rendered = render_received_with_quotes(&markdown.join(" "), &[], &quotes, &tasks);

    for (_, key) in cases {
        assert!(
            rendered
                .html
                .contains(&format!("data-quote-disposition=\"{key}\"")),
            "{key}: {}",
            rendered.html
        );
    }
    assert_eq!(
        rendered
            .html
            .matches("class=\"message-quote-task\"")
            .count(),
        1
    );
    assert!(
        rendered
            .html
            .contains("data-task-handle=\"QUOTE-dTVQdQfLw\"")
    );
    assert!(
        rendered
            .html
            .contains("aria-label=\"Open task QUOTE-dTVQdQfLw in Work\"")
    );
}

#[test]
fn unavailable_quote_cards_name_reason_and_target_without_changing_plain_quotes() {
    let target = steering_target("dTVQdQfLw", "missing");
    let target_text = target.to_string();
    let unavailable = [wire::QuotePresentation::Unavailable {
        target: target_text.clone(),
        reason: wire::QuoteUnavailable::DepthExceeded,
    }];
    let rendered =
        render_received_with_quotes(&target.markdown(), &[], &unavailable, &BTreeMap::new());

    assert!(rendered.html.contains("Quote unavailable: depth-exceeded"));
    assert!(
        rendered
            .html
            .contains("data-quote-reason=\"depth-exceeded\"")
    );
    assert!(rendered.html.contains(&target_text));

    let ordinary = render_received("> copied words");
    assert!(ordinary.html.contains("<blockquote>"));
    assert!(ordinary.html.contains("copied words"));
    assert!(!ordinary.html.contains("message-quote-card"));
}
