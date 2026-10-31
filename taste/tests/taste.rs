use gate::PolicyContext;
use listen::{Event, Locus, Prose, ProseRole, Usage};
use taste::{Bag, Taste, Trigger, TriggerError, bag_identity, normalize_prose, reminder_identity};
use wire::FactKind;

const BEHAVIOR: &str = include_str!("fixtures/behavior.txt");
const MENTION: &str = include_str!("fixtures/mention.txt");

fn locus(text: &str) -> Locus {
    Locus::new(7, 31, text.len() as u64)
}

fn prose(text: &str) -> Event {
    Event::Prose(Prose {
        role: ProseRole::Assistant,
        text: text.to_owned(),
        phase: Some("final".to_owned()),
        media: Vec::new(),
        external_media: Vec::new(),
        locus: locus(text),
    })
}

fn fallback_taste() -> Taste {
    Taste::new(
        "fallbacks".parse().expect("taste name"),
        ["fall back", "fallback", "polling"],
        "Keep one deterministic path.",
    )
    .expect("taste fixture")
}

fn matching_hits(bag: &Bag, event: &Event) -> Vec<taste::Hit> {
    bag.hits(event, &PolicyContext::new())
        .expect("unconstrained taste context")
}

#[test]
fn exact_alphabetic_words_and_phrases_follow_one_matching_rule() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hits = matching_hits(&bag, &prose(BEHAVIOR));

    assert_eq!(
        hits.iter().map(|hit| hit.trigger()).collect::<Vec<_>>(),
        vec!["fall back", "polling"],
    );
    assert!(hits.iter().all(|hit| hit.context() == BEHAVIOR));
    assert!(hits.iter().all(|hit| hit.locus() == locus(BEHAVIOR)));
    assert_eq!(hits[0].span().end - hits[0].span().first, 2);
    assert_eq!(hits[1].span().end - hits[1].span().first, 1);

    let exact = Taste::new(
        "shims".parse().expect("taste name"),
        ["shim"],
        "Replace the old shape.",
    )
    .expect("taste fixture");
    let exact = Bag::new(vec![exact]).expect("bag");
    let exact_hits = matching_hits(&exact, &prose("A shimmer is not a shim."));
    assert_eq!(exact_hits.len(), 1, "a bare word matches a whole token");
}

#[test]
fn a_hit_names_the_exact_line_and_byte_column_of_its_first_token() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hits = matching_hits(&bag, &prose("prefix\n  Polling now"));

    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].line(), hits[0].column()), (2, 3));

    let unicode = matching_hits(&bag, &prose("é: polling"));
    assert_eq!(unicode.len(), 1);
    assert_eq!(unicode[0].column(), 5, "the column counts UTF-8 bytes");
}

#[test]
fn reminder_identity_frames_taste_and_whole_normalized_prose() {
    let fallback: wire::Name = "fallbacks".parse().expect("taste name");
    let polling: wire::Name = "polling".parse().expect("taste name");
    let first = reminder_identity(&fallback, "  Keep\nONE path. ");

    assert_eq!(normalize_prose("  Keep\nONE path. "), "keep one path.");
    assert_eq!(first, reminder_identity(&fallback, "keep one   PATH."));
    assert_ne!(first, reminder_identity(&fallback, "keep one path!"));
    assert_ne!(first, reminder_identity(&polling, "keep one path."));
}

#[test]
fn bag_identity_covers_ordered_names_triggers_and_vendors() {
    let original = Bag::new(vec![fallback_taste()]).expect("bag");
    let same = Bag::new(vec![fallback_taste()]).expect("same bag");
    let changed = Bag::new(vec![
        Taste::new(
            "fallbacks".parse().expect("taste name"),
            ["fall back", "fallback", "polling"],
            "Use a different vendor.",
        )
        .expect("changed taste"),
    ])
    .expect("changed bag");

    assert_eq!(bag_identity(&original), bag_identity(&same));
    assert_ne!(bag_identity(&original), bag_identity(&changed));
    assert_ne!(
        bag_identity(&original),
        reminder_identity(&"fallbacks".parse().expect("taste name"), BEHAVIOR),
        "bag and prose identities occupy distinct domains"
    );
}

#[test]
fn non_alphabetic_and_empty_triggers_are_refused() {
    for invalid in ["fall*", "fall* back", "f*llback", "fall-back", "word2"] {
        assert!(matches!(
            Trigger::parse(invalid),
            Err(TriggerError::NotWord(_))
        ));
    }
    assert_eq!(Trigger::parse("  "), Err(TriggerError::Empty));
}

#[test]
fn matching_is_literal_case_insensitive_and_alphabetic() {
    let taste = Taste::new(
        "literal".parse().expect("taste name"),
        ["Polling", "fall back", "élan vital"],
        "Use literal selectors.",
    )
    .expect("literal taste");
    let bag = Bag::new(vec![taste]).expect("bag");

    let hits = matching_hits(&bag, &prose("POLLING, then fall\nback with ÉLAN—VITAL."));
    assert_eq!(
        hits.iter().map(|hit| hit.trigger()).collect::<Vec<_>>(),
        ["Polling", "fall back", "élan vital"]
    );

    for adjacent in [
        "poll",
        "polls",
        "polled",
        "pollster",
        "repolling",
        "pollings",
        "polling2",
        "2polling",
    ] {
        assert!(
            matching_hits(&bag, &prose(adjacent)).is_empty(),
            "unlisted variation {adjacent:?} must remain inert"
        );
    }
    assert!(
        matching_hits(&bag, &prose("fall 2 back")).is_empty(),
        "a digit-bearing run must remain a phrase boundary"
    );
}

#[test]
fn only_typed_assistant_prose_is_a_candidate_surface() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let event = Event::Usage(Usage {
        turns: 1,
        input_tokens: 3,
        cached_input_tokens: 0,
        cache_write_input_tokens: 0,
        output_tokens: 2,
        reasoning_output_tokens: None,
        locus: Locus::new(1, 0, 10),
    });

    assert!(matching_hits(&bag, &event).is_empty());
}

#[test]
fn a_harmless_mention_still_selects_a_candidate_the_runtime_must_weigh() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");

    let hit = matching_hits(&bag, &prose(MENTION))
        .into_iter()
        .next()
        .expect("mention still selects a candidate");

    // The bag is a selector, not a verdict. Runtime policy decides whether the
    // candidate reaches direct steering or the optional resident adjudicator.
    assert_eq!(hit.taste().as_str(), "fallbacks");
    assert_eq!(hit.context(), MENTION);
}

#[test]
fn direct_reminders_and_judgments_have_distinct_producer_contracts() {
    let expected = [FactKind::TasteJudged, FactKind::TasteReminded];
    assert!(
        expected
            .iter()
            .all(|kind| kind.as_str().starts_with("taste::"))
    );
    assert_eq!(
        FactKind::TasteReminded.producer(),
        wire::Producer::ResidentRecognition,
        "the disabled path remains resident recognition"
    );
    assert_eq!(
        FactKind::TasteJudged.producer(),
        wire::Producer::Judge,
        "every durable verdict belongs to the judge producer"
    );
}
