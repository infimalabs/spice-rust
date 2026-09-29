//! The tracked document is the only place a taste is named.
//!
//! Every assertion about the installed set is made against tracked bytes, and
//! every assertion about decoder behavior is made against a synthetic document
//! written here. No test spells a seed name, trigger word, or message: an
//! oracle that repeated those values would agree with the configuration only
//! until an operator commit moved one of them, which is precisely the drift
//! sourcing the set from configuration exists to prevent.

use gate::PolicyContext;
use listen::{Event, Locus, Prose};
use taste::{Bag, ConfigError, bag_identity, configured};

/// The tracked curation document, which owns the seeds and their rationale.
const TASTES: &str = include_str!("../../TASTES.md");

/// The tracked repository configuration, in full and unedited.
const TRACKED: &str = include_str!("../../spice-rust.toml");

/// The seed block exactly as `TASTES.md` publishes it.
///
/// The block is lifted rather than retyped, so this file states where the
/// seeds live without stating what they are. Exactly one block must be found:
/// a second one added later is an ambiguity about which is authoritative, and
/// silently taking the first would resolve it by accident.
fn documented_seeds() -> &'static str {
    let opens = TASTES.match_indices("```toml").collect::<Vec<_>>();
    assert_eq!(
        opens.len(),
        1,
        "TASTES.md must publish exactly one toml seed block",
    );
    let body = &TASTES[opens[0].0 + "```toml".len()..];
    let end = body.find("```").expect("the seed block must be closed");
    &body[..end]
}

fn prose(text: &str) -> Event {
    Event::Prose(Prose {
        role: listen::ProseRole::Assistant,
        text: text.to_owned(),
        phase: Some("final".to_owned()),
        media: Vec::new(),
        external_media: Vec::new(),
        locus: Locus::new(7, 31, text.len() as u64),
    })
}

fn refusal(document: &str) -> ConfigError {
    configured(document).expect_err("this document must be refused")
}

fn matching_hits(bag: &Bag, event: &Event) -> Vec<taste::Hit> {
    bag.hits(event, &PolicyContext::new())
        .expect("unconstrained taste context")
}

#[test]
fn the_documented_seed_block_decodes_as_a_complete_tracked_document() {
    let bag = configured(documented_seeds()).expect("the published seeds decode");

    assert_eq!(
        bag.tastes().len(),
        6,
        "TASTES.md publishes a seed set of six; a seed was added or dropped",
    );
}

#[test]
fn every_documented_seed_selects_prose_containing_its_own_tracked_words() {
    let bag = configured(documented_seeds()).expect("the published seeds decode");
    let mut proven = 0;

    for taste in bag.tastes() {
        // Selection alone would ratify a decoder that altered a word, because
        // the probe below is built from whatever the decoder returned and
        // would match its own corruption. Each decoded value is therefore
        // first required to appear in the tracked block as TOML spells it:
        // quoted in full, so a truncated selector no longer matches.
        let block = documented_seeds();
        assert!(
            block.contains(&format!("[taste.{}]", taste.name())),
            "taste {} is not a table in the tracked block",
            taste.name(),
        );
        assert!(
            block.contains(&format!("\"{}\"", taste.message())),
            "the message decoded for {} is not the one tracked",
            taste.name(),
        );

        for trigger in taste.triggers() {
            assert!(
                block.contains(&format!("\"{}\"", trigger.as_str())),
                "taste {} carries a selector the tracked block does not spell: {:?}",
                taste.name(),
                trigger.as_str(),
            );

            // The probe is built from the tracked selector itself, so this
            // file still names no word while exercising every literal entry.
            let probe = trigger.as_str();
            let hits = matching_hits(&bag, &prose(&format!("we should {probe} instead")));
            let hit = hits
                .iter()
                .find(|hit| hit.taste() == taste.name() && hit.trigger() == trigger.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "taste {} selector {:?} selected nothing it describes",
                        taste.name(),
                        trigger.as_str(),
                    )
                });
            assert_eq!(
                hit.message(),
                taste.message(),
                "a candidate must carry the message tracked beside its words",
            );
            proven += 1;
        }
    }

    assert_eq!(
        proven,
        bag.tastes()
            .iter()
            .map(|taste| taste.triggers().len())
            .sum::<usize>(),
        "every tracked selector must be proven, not merely iterated",
    );
    assert!(proven > 0, "the seed set must carry selectors");
}

#[test]
fn the_tracked_root_configuration_installs_the_documented_seed_set() {
    // Decoding the complete document also proves unrelated root sections are
    // ignored. Equality covers identities, selectors, messages, and order
    // without repeating any of those configured values in Rust.
    assert_eq!(
        configured(TRACKED).expect("the tracked taste set decodes"),
        configured(documented_seeds()).expect("the published seeds decode"),
        "spice-rust.toml must install exactly the seed behavior TASTES.md publishes",
    );
}

#[test]
fn rows_arrive_in_tracked_order_rather_than_sorted() {
    let bag = configured(
        r#"
[taste.zulu]
words = ["aardvark"]
message = "Prefer the last letter."

[taste.alpha]
words = ["buffalo"]
message = "Prefer the first letter."
"#,
    )
    .expect("two rows decode");

    assert_eq!(
        bag.tastes()
            .iter()
            .map(|taste| taste.name().as_str())
            .collect::<Vec<_>>(),
        vec!["zulu", "alpha"],
        "the configured set must follow the tracked document, not an imposed sort",
    );
}

#[test]
fn adding_editing_and_removing_a_row_moves_the_configured_set_with_no_rust_edit() {
    let one = configured(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
"#,
    )
    .expect("one row decodes");
    let added = configured(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."

[taste.beta]
words = ["buffalo"]
message = "Prefer the second letter."
"#,
    )
    .expect("two rows decode");
    let edited = configured(
        r#"
[taste.alpha]
words = ["chinchilla"]
message = "Prefer the third letter."
"#,
    )
    .expect("an edited row decodes");

    let names = |bag: &Bag| {
        bag.tastes()
            .iter()
            .map(|taste| taste.name().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&one), vec!["alpha"]);
    assert_eq!(names(&added), vec!["alpha", "beta"]);
    assert_eq!(names(&edited), vec!["alpha"]);

    // Identity is not the whole claim: matching must move too.
    assert!(matching_hits(&one, &prose("one buffalo here")).is_empty());
    assert_eq!(matching_hits(&added, &prose("one buffalo here")).len(), 1);
    assert_eq!(matching_hits(&added, &prose("one aardvark here")).len(), 1);

    // Removing the added row retires exactly the matching it brought.
    assert_eq!(matching_hits(&one, &prose("one aardvark here")).len(), 1);

    // Editing one row replaces both its words and its message under a name
    // that did not change, which no identity check alone would notice.
    assert!(matching_hits(&edited, &prose("one aardvark here")).is_empty());
    let hit = matching_hits(&edited, &prose("one chinchilla here"))
        .into_iter()
        .next()
        .expect("the edited words select");
    assert_eq!(hit.message(), "Prefer the third letter.");
}

#[test]
fn a_vendor_and_model_scoped_bag_only_fires_in_its_shared_applicability_context() {
    let bag = configured(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
scopes = { vendors = ["omp"], models = ["gpt-5"] }
"#,
    )
    .expect("scoped row decodes");
    let scoped = PolicyContext::new().with_vendor("omp").with_model("gpt-5");
    let other = PolicyContext::new()
        .with_vendor("other")
        .with_model("gpt-5");

    assert_eq!(
        bag.hits(&prose("one aardvark here"), &scoped)
            .expect("scoped model context resolves")
            .len(),
        1,
    );
    assert!(
        bag.hits(&prose("one aardvark here"), &other)
            .expect("other vendor resolves")
            .is_empty(),
        "spice-shape case 438: a vendor-scoped bag cannot fire elsewhere",
    );
    assert!(
        bag.hits(
            &prose("one aardvark here"),
            &PolicyContext::new().with_vendor("omp").with_model("o3"),
        )
        .expect("other model resolves")
        .is_empty(),
        "the same shared grammar constrains another admitted context axis",
    );
}

#[test]
fn a_disabled_named_row_remains_tracked_enumerable_clone_stable_and_inert() {
    let document = r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
enabled = false
scopes = { vendors = ["omp"] }
"#;
    let bag = configured(document).expect("disabled row decodes");
    let enabled = configured(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
scopes = { vendors = ["omp"] }
"#,
    )
    .expect("enabled row decodes");
    let other_clone = bag.clone();
    let taste = &bag.tastes()[0];

    assert_eq!(taste.name().as_str(), "alpha");
    assert!(!taste.enabled());
    assert_eq!(taste.scopes().vendors(), ["omp"]);
    assert_eq!(other_clone, bag);
    assert_eq!(bag_identity(&other_clone), bag_identity(&bag));
    assert_ne!(
        bag_identity(&enabled),
        bag_identity(&bag),
        "tracked disablement changes the effective bag identity",
    );
    assert!(
        bag.hits(&prose("one aardvark here"), &PolicyContext::new())
            .expect("disabled row needs no applicability evaluation")
            .is_empty(),
        "spice-shape case 439: the named tracked disablement is inert",
    );
}

#[test]
fn malformed_or_incomplete_applicability_refuses_loudly() {
    let malformed = refusal(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
scopes = { vendors = [""] }
"#,
    );
    assert!(matches!(&malformed, ConfigError::Taste { name, .. } if name == "alpha"));
    assert!(
        malformed.to_string().contains("scopes.vendors"),
        "the shared compiler must name the malformed selector axis: {malformed}",
    );

    let bag = configured(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
scopes = { vendors = ["omp"] }
"#,
    )
    .expect("valid constrained row decodes");
    let missing = bag
        .hits(&prose("one aardvark here"), &PolicyContext::new())
        .expect_err("missing constrained vendor must refuse");
    assert_eq!(missing.code(), "policy::context-missing");
    assert_eq!(missing.dimension(), Some(gate::ScopeAxis::Vendors));
}

#[test]
fn malformed_toml_is_refused_with_the_parser_position() {
    let error = refusal("[taste.alpha\nwords = [\"aardvark\"]\n");

    assert!(matches!(error, ConfigError::Parse(_)));
    assert!(
        error.to_string().contains("not valid TOML"),
        "refusal must name the parse failure: {error}",
    );
}

#[test]
fn a_missing_taste_section_and_an_empty_one_are_refused_apart() {
    assert!(matches!(
        refusal("[tasks]\nstems = [\"taste\"]\n"),
        ConfigError::Absent,
    ));
    assert!(matches!(refusal("[taste]\n"), ConfigError::Empty));
}

#[test]
fn a_repeated_table_or_key_is_refused_by_the_parser() {
    let table = refusal(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."

[taste.alpha]
words = ["buffalo"]
message = "Prefer the second letter."
"#,
    );
    let key = refusal(
        r#"
[taste.alpha]
words = ["aardvark"]
words = ["buffalo"]
message = "Prefer the first letter."
"#,
    );

    // Both are refused before this decoder sees them, so each carries the
    // parser's own context rather than a taste identity this crate supplied.
    // A repeated table is reported at the table, and a repeated key at the
    // key, which is why they are asserted apart instead of through one loop
    // that would pass on whichever context happened to be present.
    assert!(matches!(table, ConfigError::Parse(_)));
    assert!(
        table.to_string().contains("alpha"),
        "a repeated table must name the table: {table}",
    );

    assert!(matches!(key, ConfigError::Parse(_)));
    let reported = key.to_string();
    assert!(
        reported.contains("words") && reported.contains("line 4"),
        "a repeated key must name the key and where it sits: {key}",
    );
}

#[test]
fn a_table_name_that_is_not_a_taste_name_is_refused_with_that_name() {
    let error = refusal(
        r#"
[taste.Alpha]
words = ["aardvark"]
message = "Prefer the first letter."
"#,
    );

    assert!(matches!(&error, ConfigError::Name { name, .. } if name == "Alpha"));
    assert!(
        error.to_string().contains("[taste.Alpha]"),
        "refusal must name the offending table: {error}",
    );
}

#[test]
fn a_row_that_is_not_shaped_like_a_taste_is_refused_with_its_taste_name() {
    let misspelled = refusal(
        r#"
[taste.alpha]
word = ["aardvark"]
message = "Prefer the first letter."
"#,
    );
    let mistyped = refusal(
        r#"
[taste.alpha]
words = "aardvark"
message = "Prefer the first letter."
"#,
    );
    // A key beside a complete and valid row. Nothing else refuses this one:
    // the words and message are both present and both admissible, so only
    // closing the row's shape catches a stray key that reads as configuration
    // but steers nothing.
    let extra = refusal(
        r#"
[taste.alpha]
words = ["aardvark"]
message = "Prefer the first letter."
severity = "high"
"#,
    );

    for error in [misspelled, mistyped, extra] {
        assert!(matches!(&error, ConfigError::Row { name, .. } if name == "alpha"));
        assert!(
            error.to_string().contains("[taste.alpha]"),
            "refusal must name the offending row: {error}",
        );
    }
}

#[test]
fn an_invalid_trigger_empty_bag_or_empty_message_is_refused_with_its_taste_name() {
    let documents = [
        r#"
[taste.alpha]
words = ["aard*vark"]
message = "Prefer the first letter."
"#,
        r#"
[taste.alpha]
words = []
message = "Prefer the first letter."
"#,
        r#"
[taste.alpha]
words = ["aardvark"]
message = "   "
"#,
    ];

    for document in documents {
        let error = refusal(document);
        assert!(
            matches!(&error, ConfigError::Taste { name, .. } if name == "alpha"),
            "expected a taste refusal, got {error}",
        );
        assert!(
            error.to_string().contains("[taste.alpha]"),
            "refusal must name the offending taste: {error}",
        );
    }
}
