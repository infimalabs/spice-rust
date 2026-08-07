use listen::{Event, Locus, Prose, Usage};
use taste::{
    Bag, Decision, JudgmentFact, REMINDER_MARKER, Taste, Trigger, TriggerError, is_reminder_echo,
    judge, normalize_prose, reminder_identity, reminder_text,
};
use wire::FactKind;

const BEHAVIOR: &str = include_str!("fixtures/behavior.txt");
const MENTION: &str = include_str!("fixtures/mention.txt");

fn locus(text: &str) -> Locus {
    Locus::new(7, 31, text.len() as u64)
}

fn prose(text: &str) -> Event {
    Event::Prose(Prose {
        text: text.to_owned(),
        phase: Some("final".to_owned()),
        locus: locus(text),
    })
}

fn fallback_taste() -> Taste {
    Taste::new(
        "fallbacks".parse().expect("taste name"),
        ["fall back", "fallback*", "poll*"],
        "Keep one deterministic path.",
    )
    .expect("taste fixture")
}

#[test]
fn exact_words_suffix_globs_and_phrases_follow_one_matching_rule() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hits = bag.hits(&prose(BEHAVIOR));

    assert_eq!(
        hits.iter().map(|hit| hit.trigger()).collect::<Vec<_>>(),
        vec!["fall back", "poll*"],
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
    let exact_hits = Bag::new(vec![exact])
        .expect("bag")
        .hits(&prose("A shimmer is not a shim."));
    assert_eq!(exact_hits.len(), 1, "a bare word matches a whole token");
}

#[test]
fn a_hit_names_the_exact_line_and_byte_column_of_its_first_token() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hits = bag.hits(&prose("prefix\n  Polling now"));

    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].line(), hits[0].column()), (2, 3));

    let unicode = bag.hits(&prose("é: polling"));
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
fn the_reserved_marker_classifies_echo_before_matching_and_leads_rendering() {
    let taste: wire::Name = "fallbacks".parse().expect("taste name");
    let message = "Keep one deterministic path.";
    let rendered = reminder_text(&taste, message);

    assert!(rendered.starts_with(REMINDER_MARKER));
    assert!(rendered.contains(taste.as_str()));
    assert!(
        rendered.ends_with(message),
        "configured content is unchanged"
    );
    assert!(is_reminder_echo(&rendered));
    assert!(is_reminder_echo(&format!("> quoted {rendered}")));
    assert!(!is_reminder_echo("ordinary prose about taste"));
}

#[test]
fn invalid_glob_positions_and_non_words_are_refused() {
    assert!(matches!(
        Trigger::parse("fall* back"),
        Err(TriggerError::GlobPosition(_))
    ));
    assert!(matches!(
        Trigger::parse("f*llback"),
        Err(TriggerError::GlobPosition(_))
    ));
    assert!(matches!(
        Trigger::parse("fall-back"),
        Err(TriggerError::NotWord(_))
    ));
    assert_eq!(Trigger::parse("  "), Err(TriggerError::Empty));
}

#[test]
fn only_typed_assistant_prose_is_a_candidate_surface() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let event = Event::Usage(Usage {
        input_tokens: 3,
        cached_input_tokens: 0,
        cache_write_input_tokens: 0,
        output_tokens: 2,
        reasoning_output_tokens: None,
        locus: Locus::new(1, 0, 10),
    });

    assert!(bag.hits(&event).is_empty());
}

#[test]
fn judgment_is_the_only_door_from_a_hit_to_a_historical_curation_fact() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hit = bag
        .hits(&prose(BEHAVIOR))
        .into_iter()
        .next()
        .expect("behavior fixture hits");

    let cleared = judge(hit.clone(), Decision::Clear).into_fact();
    assert_eq!(cleared.kind(), FactKind::TasteCleared);
    assert!(fold::FactPlane::Taste.contains(cleared.kind()));
    let JudgmentFact::Cleared(cleared) = cleared else {
        panic!("a clearance minted the wrong typed body");
    };
    assert_eq!(cleared.body.taste, "fallbacks");
    assert_eq!(cleared.body.locus, locus(BEHAVIOR).to_string());

    let confirmed = judge(hit, Decision::Confirm).into_fact();
    assert_eq!(confirmed.kind(), FactKind::TasteConfirmed);
    assert!(fold::FactPlane::Taste.contains(confirmed.kind()));
    let JudgmentFact::Confirmed(confirmed) = confirmed else {
        panic!("a confirmation minted the wrong typed body");
    };
    assert_eq!(confirmed.body.taste, "fallbacks");
    assert_eq!(confirmed.body.locus, locus(BEHAVIOR).to_string());
}

#[test]
fn a_harmless_mention_reaches_the_clearance_fact_path() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hit = bag
        .hits(&prose(MENTION))
        .into_iter()
        .next()
        .expect("mention still selects a candidate");

    assert_eq!(
        judge(hit, Decision::Clear).into_fact().kind(),
        FactKind::TasteCleared
    );
}

#[test]
fn judgment_and_reminder_origins_cover_the_taste_plane_without_overlap() {
    let bag = Bag::new(vec![fallback_taste()]).expect("bag");
    let hit = bag
        .hits(&prose(BEHAVIOR))
        .into_iter()
        .next()
        .expect("behavior fixture hits");
    let mut judgment_produced = [
        judge(hit.clone(), Decision::Confirm).into_fact().kind(),
        judge(hit, Decision::Clear).into_fact().kind(),
    ];
    judgment_produced.sort_unstable();

    assert_eq!(
        judgment_produced,
        [FactKind::TasteConfirmed, FactKind::TasteCleared],
        "the judge owns historical curation evidence only"
    );

    let reminder_originated = [FactKind::TasteReminded];
    assert_eq!(
        FactKind::TasteReminded.producer(),
        wire::Producer::ResidentRecognition,
        "the live judge-free path belongs to resident recognition"
    );
    assert!(
        reminder_originated
            .iter()
            .all(|kind| !judgment_produced.contains(kind))
    );

    let mut produced = judgment_produced
        .into_iter()
        .chain(reminder_originated)
        .collect::<Vec<_>>();
    produced.sort_unstable();

    let mut taste_plane = FactKind::ALL
        .iter()
        .copied()
        .filter(|kind| fold::FactPlane::Taste.contains(*kind))
        .collect::<Vec<_>>();
    taste_plane.sort_unstable();

    assert_eq!(produced, taste_plane);
}
