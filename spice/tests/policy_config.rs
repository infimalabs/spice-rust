use consent::Answer;
use gate::Shape;
use lane::life::Life;
use spice::config::Root;

const COMPLETE: &str = r#"
[taste.zeta]
words = ["delay*", "poll*", "slept"]
message = "Wait on the real event."

[taste.alpha]
words = ["finish*"]
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
review = -75.5
band = 3.0

[lane]
life = "Steer"
lease = "90s"
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
        ["delay*", "poll*", "slept"]
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

    assert_eq!(decoded.board().next().review(), -75.5);
    assert_eq!(decoded.board().next().band(), 3.0);
    assert_eq!(decoded.lane().life(), Life::Steer);
    assert_eq!(decoded.lane().lease().to_string(), "90s");
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
    assert_eq!(root.board().next().review(), -100.0);
    assert_eq!(root.board().next().band(), 2.0);
    assert_eq!(root.lane().life(), Life::Drive);
    assert_eq!(root.lane().lease().to_string(), "15m");
    assert_eq!(root.lane().vendor().as_str(), "claude");
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
        ["5m", "15m", "1h"]
    );
    assert_eq!(root.vendor().claude().binary().as_str(), "claude");
    assert_eq!(root.vendor().claude().model(), vendor::DEFAULT_CLAUDE_MODEL);
    assert_eq!(root.vendor().claude().effort(), vendor::Effort::High);
    assert_eq!(root.vendor().codex().binary().as_str(), "codex");
    assert_eq!(root.vendor().codex().model(), vendor::DEFAULT_CODEX_MODEL);
    assert_eq!(root.vendor().codex().effort(), vendor::Effort::High);
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
fn malformed_policy_values_are_refused_under_their_domain_paths() {
    for (document, path) in [
        (
            "[taste.Bad]\nwords = [\"poll*\"]\nmessage = \"wait\"\n",
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
        ("[lane]\nvendor = \"Claude\"\n", "lane.vendor"),
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
}

#[test]
fn unknown_fields_name_their_domain_and_key() {
    for (document, domain) in [
        (
            "[taste.polling]\nwords = [\"poll*\"]\nmessage = \"wait\"\nextra = 1\n",
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
