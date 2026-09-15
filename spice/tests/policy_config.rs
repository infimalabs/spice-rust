use consent::Answer;
use gate::Shape;
use lane::life::Life;
use spice::config::{Document, Inputs, Root};

const COMPLETE: &str = r#"
[taste.zeta]
words = ["delay", "poll", "slept"]
message = "Wait on the real event."

[taste.alpha]
words = ["finish"]
message = "Observe completion."

[consent]
posture = "queue"

[[consent.rule]]
name = "reads"
tool = "read*"
answer = "allow"
budget = 25

[[consent.rule]]
name = "fallback"
tool = "*"
answer = "deny"

[gate.walk]
skip = ["src/**", "fixtures/"]

[board.next]
band = 3

[lane]
life = "Steer"
lease = "90s"
runtime = "6h"
vendor = "codex"
deaths = 4
young = "45s"
hold = "1h"

[steer]
expiry = "48h"
resend = ["10m", "30m", "2h"]

[vendor.claude]
binary = "/opt/claude"
model = "claude-opus"
effort = "max"
auto_compact_window_tokens = 123456

[vendor.codex]
binary = "/opt/codex"
model = "gpt-codex"
effort = "max"
model_context_window = 1234567
personality = "pragmatic"
fast_mode = true

[proof]
image = "registry.example/proof@sha256:4444444444444444444444444444444444444444444444444444444444444444"
"#;

fn decode(document: &str) -> Result<Root, String> {
    let deserializer = toml::Deserializer::parse(document).map_err(|error| error.to_string())?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| error.to_string())
}

#[test]
fn every_policy_shape_round_trips_without_losing_order_or_templates() {
    let decoded = decode(COMPLETE).expect("the complete policy fixture decodes");

    assert_eq!(
        decoded
            .taste()
            .bag()
            .tastes()
            .iter()
            .map(|taste| taste.name().as_str())
            .collect::<Vec<_>>(),
        ["zeta", "alpha"]
    );
    assert_eq!(
        decoded.taste().bag().tastes()[0]
            .triggers()
            .iter()
            .map(taste::Trigger::as_str)
            .collect::<Vec<_>>(),
        ["delay", "poll", "slept"]
    );
    assert_eq!(
        decoded.consent().policy().decide("read file").answer(),
        Answer::Allow
    );
    assert_eq!(
        decoded.consent().policy().decide("write file").answer(),
        Answer::Deny
    );
    assert_eq!(decoded.consent().policy().rules()[0].name(), "reads");
    assert_eq!(decoded.consent().policy().rules()[1].name(), "fallback");

    let skips = decoded.gate().walk().skip().rules();
    assert!(skips[0].matches("src/lib.rs", Shape::File));
    assert!(skips[0].matches("src/deep/module.rs", Shape::File));
    assert!(!skips[0].matches("src", Shape::Directory));
    assert!(!skips[0].matches("docs/src/lib.rs", Shape::File));

    assert_eq!(decoded.board().next().band(), 3);
    assert_eq!(decoded.lane().life(), Life::Steer);
    assert_eq!(decoded.lane().lease().to_string(), "90s");
    assert_eq!(decoded.lane().runtime().to_string(), "6h");
    assert_eq!(decoded.lane().vendor().as_str(), "codex");
    assert_eq!(decoded.lane().deaths(), 4);
    assert_eq!(decoded.lane().young().to_string(), "45s");
    assert_eq!(decoded.lane().hold().to_string(), "1h");
    assert_eq!(decoded.steer().expiry().to_string(), "48h");
    assert_eq!(
        decoded
            .steer()
            .resend()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["10m", "30m", "2h"]
    );
    assert_eq!(decoded.vendor().claude().binary().as_str(), "/opt/claude");
    assert_eq!(decoded.vendor().claude().model(), "claude-opus");
    assert_eq!(decoded.vendor().claude().effort(), vendor::Effort::Max);
    assert_eq!(
        decoded.vendor().claude().auto_compact_window_tokens(),
        123_456
    );
    assert_eq!(decoded.vendor().codex().binary().as_str(), "/opt/codex");
    assert_eq!(decoded.vendor().codex().model(), "gpt-codex");
    assert_eq!(decoded.vendor().codex().effort(), vendor::Effort::Max);
    assert_eq!(decoded.vendor().codex().model_context_window(), 1_234_567);
    assert_eq!(
        decoded.vendor().codex().model_auto_compact_token_limit(),
        1_172_838
    );
    assert_eq!(decoded.vendor().codex().personality(), Some("pragmatic"));
    assert!(decoded.vendor().codex().fast_mode());
    assert_eq!(
        decoded.proof().image().as_str(),
        "registry.example/proof@sha256:4444444444444444444444444444444444444444444444444444444444444444"
    );

    let rendered = toml::to_string_pretty(&decoded).expect("the typed policy renders as TOML");
    let round_trip = decode(&rendered).expect("the rendered policy decodes");
    assert_eq!(round_trip, decoded);
}

#[test]
fn an_empty_tree_is_the_complete_compiled_policy() {
    let root = decode("").expect("an empty policy tree is valid");

    assert!(root.taste().bag().tastes().is_empty());
    assert_eq!(root.consent().policy().posture(), Answer::Queue);
    assert!(root.consent().policy().rules().is_empty());
    assert!(root.gate().walk().skip().rules().is_empty());
    assert_eq!(root.board().next().band(), 2);
    assert_eq!(root.lane().life(), Life::Drive);
    assert_eq!(root.lane().lease().to_string(), "15m");
    assert_eq!(root.lane().runtime().to_string(), "24h");
    assert_eq!(root.lane().vendor().as_str(), "codex");
    assert_eq!(root.lane().deaths(), 3);
    assert_eq!(root.lane().young().to_string(), "30s");
    assert_eq!(root.lane().hold().to_string(), "30m");
    assert_eq!(root.steer().expiry().to_string(), "24h");
    assert_eq!(
        root.steer()
            .resend()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["15s", "60s", "300s"]
    );
    assert_eq!(root.vendor().claude().binary().as_str(), "claude");
    assert_eq!(vendor::DEFAULT_CLAUDE_MODEL, "claude-opus-5");
    assert_eq!(root.vendor().claude().model(), vendor::DEFAULT_CLAUDE_MODEL);
    assert_eq!(root.vendor().claude().effort(), vendor::Effort::XHigh);
    assert_eq!(root.vendor().codex().binary().as_str(), "codex");
    assert_eq!(root.vendor().codex().model(), vendor::DEFAULT_CODEX_MODEL);
    assert_eq!(root.vendor().codex().effort(), vendor::Effort::High);
    assert_eq!(
        root.vendor().codex().model_context_window(),
        vendor::DEFAULT_CODEX_MODEL_CONTEXT_WINDOW
    );
    assert_eq!(
        root.vendor().codex().model_auto_compact_token_limit(),
        997_500
    );
    assert_eq!(root.vendor().codex().personality(), None);
    assert!(!root.vendor().codex().fast_mode());
    assert_eq!(root.proof().image().as_str(), proof::DEFAULT_IMAGE);
    assert!(root.proof().container().is_none());
    assert!(root.proof().runner().is_none());

    let rendered = toml::to_string_pretty(&root).expect("compiled policy renders as TOML");
    assert_eq!(
        decode(&rendered).expect("rendered compiled policy decodes"),
        root
    );
}

#[test]
fn rust_codex_default_is_current_and_explicit_selection_still_wins() {
    assert_eq!(vendor::DEFAULT_CODEX_MODEL, "gpt-5.6-sol");
    assert_eq!(
        decode("")
            .expect("empty policy uses compiled defaults")
            .vendor()
            .codex()
            .model(),
        "gpt-5.6-sol"
    );
    assert_eq!(
        decode("[vendor.codex]\nmodel = \"operator-selected\"\n")
            .expect("explicit Codex model is valid")
            .vendor()
            .codex()
            .model(),
        "operator-selected"
    );
}

#[test]
fn malformed_policy_values_are_refused_under_their_domain_paths() {
    for (document, path) in [
        (
            "[taste.Bad]\nwords = [\"poll\"]\nmessage = \"wait\"\n",
            "taste.Bad",
        ),
        (
            "[taste.polling]\nwords = [\"poll.*\"]\nmessage = \"wait\"\n",
            "taste.polling.words",
        ),
        (
            "[consent]\n[[consent.rule]]\nname = \"Read Rule\"\ntool = \"read\"\nanswer = \"allow\"\n",
            "consent.rule[0]",
        ),
        ("[lane]\nlease = \"15minutes\"\n", "lane.lease"),
        ("[lane]\nruntime = \"eventually\"\n", "lane.runtime"),
        ("[lane]\nvendor = \"Claude\"\n", "lane.vendor"),
        ("[steer]\nresend = [\"15s\", \"60s\"]\n", "steer"),
        ("[steer]\nresend = [\"15s\", \"15s\", \"300s\"]\n", "steer"),
        (
            "[steer]\nexpiry = \"300s\"\nresend = [\"15s\", \"60s\", \"300s\"]\n",
            "steer",
        ),
        ("[gate.walk]\nskip = [\"\"]\n", "gate.walk.skip[0]"),
        ("[proof]\nimage = \"proof:latest\"\n", "proof.image"),
        ("[proof]\ncontainer = \"docker\"\n", "proof.container"),
        ("[proof]\nrunner = \"\"\n", "proof.runner"),
        ("[vendor.Claude]\nbinary = \"claude\"\n", "vendor.Claude"),
        (
            "[vendor.claude]\npersonality = \"friendly\"\n",
            "vendor.claude.personality",
        ),
        (
            "[vendor.claude]\nfast_mode = false\n",
            "vendor.claude.fast_mode",
        ),
        (
            "[vendor.claude]\nauto_compact_window_tokens = 0\n",
            "vendor.claude.auto_compact_window_tokens",
        ),
        (
            "[vendor.codex]\nauto_compact_window_tokens = 100\n",
            "vendor.codex.auto_compact_window_tokens",
        ),
        (
            "[vendor.codex]\nmodel_context_window = 'large'\n",
            "vendor.codex.model_context_window",
        ),
        (
            "[vendor.codex]\nmodel_context_window = 0\n",
            "vendor.codex.model_context_window",
        ),
        (
            "[vendor.codex]\nmodel_context_window = 1\n",
            "vendor.codex.model_context_window",
        ),
        (
            "[vendor.codex]\nmodel_context_window = 9223372036854775808\n",
            "vendor.codex.model_context_window",
        ),
        (
            "[vendor.codex]\nmodel_context_window = 18446744073709551616\n",
            "vendor.codex.model_context_window",
        ),
        (
            "[policy]\ntest_engines = [\"unknown-engine\"]\n",
            "policy.test_engines[0]",
        ),
        ("[vendor.codex]\nmodel = \"\"\n", "vendor.codex.model"),
        ("[vendor.gemini]\nbinary = \"gemini\"\n", "vendor.gemini"),
    ] {
        let error = decode(document).expect_err("malformed policy must refuse");
        assert!(
            error.contains(path),
            "refusal did not name {path:?}: {error}"
        );
    }

    let duplicate = decode(
        "[consent]\nposture = \"queue\"\n\
         [[consent.rule]]\nname = \"reads\"\ntool = \"read*\"\nanswer = \"allow\"\n\
         [[consent.rule]]\nname = \"reads\"\ntool = \"cat*\"\nanswer = \"deny\"\n",
    )
    .expect_err("duplicate consent identities must refuse");
    assert!(duplicate.contains("consent.rule"), "{duplicate}");

    let duplicate = decode("[policy]\ntest_engines = [\"cargo-workspace\", \"cargo-workspace\"]\n")
        .expect_err("duplicate test engines must refuse");
    assert!(duplicate.contains("policy.test_engines"), "{duplicate}");
}

/// Every tracked structural-policy key Gate admits is one this schema accepts.
///
/// Gate's registry decides that a tracked key is legitimate; this schema
/// decides whether the document carrying it loads at all. When the two
/// disagree the registry advertises a setting that makes the whole file
/// refuse. Each declaration below is therefore checked against Gate's sole
/// registry before the complete document is decoded.
#[test]
fn every_tracked_structural_policy_key_is_one_this_schema_admits() {
    for (key, _) in TRACKED_STRUCTURAL_DECLARATIONS {
        assert!(
            gate::CONFIG_KEY_DECLARATIONS
                .iter()
                .any(|entry| entry.template() == key),
            "Gate does not admit {key} through its current registry"
        );
    }

    let policy = decode(&structural_policy_document())
        .expect("every tracked structural-policy key declared together loads")
        .policy()
        .clone();
    assert_structural_policy_values(&policy);
}

/// Every tracked structural-policy key reaches its accessor through the resolver.
///
/// The schema test above proves serde accepts these spellings, but a tracked
/// document reaches serde only after `resolve` sanitizes it against the
/// descriptor registry. A key missing from that registry is refused as
/// `IssueKind::Unknown`, leaving the accessor unreachable while Gate still
/// advertises the setting.
#[test]
fn every_tracked_structural_policy_key_reaches_its_accessor() {
    let document = structural_policy_document();
    let resolved = spice::config::resolve(Inputs::new(
        Some(Document::new("spice-rust.toml", &document)),
        None,
        &[],
    ))
    .unwrap_or_else(|error| panic!("current resolution refused a tracked key: {error}"));
    assert_structural_policy_values(resolved.root().policy());
}

/// Each value is distinct so no reading can be satisfied by another's bytes.
const TRACKED_STRUCTURAL_DECLARATIONS: [(&str, &str); 11] = [
    ("policy.repo_truth.docs", "[\"DOCTRINE.md\"]"),
    ("policy.file_shape.source_suffixes", "[\".rs\"]"),
    ("policy.languages.complexity", "[\".rs\", \".py\"]"),
    ("policy.languages.magic", "[\".rs\"]"),
    ("policy.package_roots", "[\"src/sample\"]"),
    ("policy.generated_paths", "[\"build/derived\"]"),
    ("policy.name_cluster_threshold", "3"),
    ("policy.taste_mining_threshold", "4"),
    ("policy.test_engines", "[\"cargo-workspace\"]"),
    ("policy.test_paths", "[\"suite\"]"),
    ("policy.package.boundary_underscore_pattern", "\"^[a-z]+$\""),
];

fn structural_policy_document() -> String {
    TRACKED_STRUCTURAL_DECLARATIONS
        .iter()
        .map(|(key, value)| format!("{key} = {value}\n"))
        .collect()
}

fn assert_structural_policy_values(policy: &spice::config::WorkspacePolicy) {
    assert_eq!(
        policy.repo_truth().docs(),
        Some(["DOCTRINE.md".to_owned()].as_slice())
    );
    assert_eq!(
        policy.file_shape().source_suffixes(),
        Some([".rs".to_owned()].as_slice())
    );
    assert_eq!(
        policy.languages().complexity(),
        Some([".rs".to_owned(), ".py".to_owned()].as_slice())
    );
    assert_eq!(
        policy.languages().magic(),
        Some([".rs".to_owned()].as_slice())
    );
    assert_eq!(policy.package_roots(), ["src/sample".to_owned()]);
    assert_eq!(policy.generated_paths(), ["build/derived".to_owned()]);
    assert_eq!(policy.name_cluster_threshold(), Some(3));
    assert_eq!(policy.taste_mining_threshold(), Some(4));
    assert_eq!(
        policy.test_engines(),
        [spice::config::TestEngine::CargoWorkspace]
    );
    assert_eq!(policy.test_paths(), ["suite".to_owned()]);
    assert_eq!(
        policy.package().boundary_underscore_pattern(),
        Some("^[a-z]+$")
    );
}

#[test]
fn unknown_fields_name_their_domain_and_key() {
    for (document, domain) in [
        (
            "[taste.polling]\nwords = [\"poll\"]\nmessage = \"wait\"\nextra = 1\n",
            "taste.polling",
        ),
        ("[consent]\nextra = 1\n", "consent"),
        ("[gate.walk]\nextra = 1\n", "gate.walk"),
        ("[board.next]\nextra = 1\n", "board.next"),
        ("[lane]\nextra = 1\n", "lane"),
        ("[steer]\nextra = 1\n", "steer"),
        (
            "[vendor.claude]\nbinary = \"claude\"\nextra = 1\n",
            "vendor.claude",
        ),
        ("[proof]\nextra = 1\n", "proof"),
    ] {
        let error = decode(document).expect_err("unknown policy key must refuse");
        assert!(
            error.contains(domain),
            "refusal omitted {domain:?}: {error}"
        );
        assert!(error.contains("extra"), "refusal omitted the key: {error}");
    }
}

#[test]
fn retired_review_coefficient_and_fractional_band_refuse_loudly() {
    let review = decode("[board.next]\nreview = -100.0\n")
        .expect_err("the removed review coefficient must not remain inert");
    assert!(review.contains("board.next"), "{review}");
    assert!(review.contains("review"), "{review}");

    let band = decode("[board.next]\nband = 2.5\n")
        .expect_err("an integer urgency scalar needs an integer tie band");
    assert!(band.contains("board.next.band"), "{band}");
}

#[test]
fn taste_mining_threshold_requires_real_recurrence() {
    let error = decode("[policy]\ntaste_mining_threshold = 1\n")
        .expect_err("one correction is not recurrence");
    assert!(error.contains("at least two independent loci"), "{error}");
}

#[test]
fn regex_punctuation_is_only_ever_an_exact_consent_word_pattern() {
    let root = decode(
        "[consent]\nposture = \"queue\"\n\
         [[consent.rule]]\nname = \"literal\"\ntool = \"^read$\"\nanswer = \"deny\"\n",
    )
    .expect("regex-looking punctuation is admitted as literal text");

    assert_eq!(
        root.consent().policy().decide("read").answer(),
        Answer::Queue
    );
    assert_eq!(
        root.consent().policy().decide("^read$").answer(),
        Answer::Deny
    );
}
