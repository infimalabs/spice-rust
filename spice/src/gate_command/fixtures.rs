use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gate::{
    Bounds, GateReport, Ignore, Kill, KillBounds, KillReport, Metric, MutationBattery,
    MutationTool, PINNED_CARGO_MUTANTS, Parser, Policy, Ratchet, ReportAttemptKind,
    ReportCompletion, ReportMutantDisposition, ReportMutationMovement, ReportOutcome, Run,
    SurvivorRate,
};
use sill::{Deadlines, Environment};
use wire::Outcome;

use super::{kill_report, run_report};
use crate::render::{self, Form, Stream};

const SOURCE: &[u8] = b"pub fn original() {}\n";
const FLOOR: &[u8] = b"rust.mutation-survivors 1\n";
const JOURNAL: &[u8] = b"caller journal sentinel\0\xff";

const FAKE_CARGO: &str = r#"#!/bin/sh
if [ "$1" = "test" ]; then
    exit 0
fi
exit 71
"#;

const FAKE_MUTANTS: &str = r#"#!/bin/sh
[ "$1" = "mutants" ] || exit 73
shift

if [ "$1" = "--version" ]; then
    printf 'cargo-mutants 27.1.0\n'
    exit 0
fi

for argument in "$@"; do
    if [ "$argument" = "--list" ]; then
        /bin/cat inventory.json
        exit 0
    fi
done

identity=
output_dir=
previous=
for argument in "$@"; do
    if [ "$previous" = "--re" ]; then
        identity=$argument
    fi
    if [ "$previous" = "--output" ]; then
        output_dir=$argument
    fi
    previous=$argument
done

write_outcome() {
    /bin/mkdir -p "$output_dir/mutants.out"
    printf '%s\n' "$1" > "$output_dir/mutants.out/outcomes.json"
}

caught='{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":0,"caught":1,"timeout":0,"unviable":0,"outcomes":[{"summary":"CaughtMutant"}]}'
survived='{"cargo_mutants_version":"27.1.0","total_mutants":1,"missed":1,"caught":0,"timeout":0,"unviable":0,"outcomes":[{"summary":"MissedMutant"}]}'
timed_out='{
    "cargo_mutants_version":"27.1.0",
    "total_mutants":1,
    "missed":0,
    "caught":0,
    "timeout":1,
    "unviable":0,
    "outcomes":[{"summary":"Timeout"}]
}'

printf 'mutated\n' >> src/lib.rs
case "$identity" in
    *Killed*)
        write_outcome "$caught"
        exit 0
        ;;
    *Survived*)
        write_outcome "$survived"
        exit 2
        ;;
    *Flaky*)
        if [ "$SPICE_MUTATION_ATTEMPT" = "1" ]; then
            write_outcome "$caught"
            exit 0
        fi
        write_outcome "$survived"
        exit 2
        ;;
    *Timeout*)
        write_outcome "$timed_out"
        exit 3
        ;;
    *) exit 70 ;;
esac
"#;

#[test]
fn bounded_parser_failure_crosses_the_ordinary_production_mapper() {
    let source = Scratch::new("parser");
    source.write("src/lib.rs", SOURCE);
    source.write("baseline.toml", FLOOR);
    source.write("journal.sqlite3", JOURNAL);

    let parser = Parser::new("/bin/sh", ["-c", "while :; do /bin/sleep 1; done"])
        .expect("declare nonterminating parser");
    let metric = Metric::new(
        "rust.lines",
        "rust",
        "lines in admitted rust sources",
        parser,
        "*.rs\n",
        "",
    )
    .expect("declare parser metric");
    let policy =
        Policy::new(Ignore::parse("").expect("parse ignore"), [metric]).expect("declare policy");
    let error = Run::adjudicate(
        &policy,
        source.path(),
        &Ratchet::default(),
        &short_bounds("gate-rim-parser"),
    )
    .expect_err("the real parser must exceed its process bound");
    let observed = error.to_string();

    let outcome = run_report(Err(error));
    let refusal = expect_refusal(&outcome, "gate::run-refused");
    assert_eq!(refusal.observed.as_deref(), Some(observed.as_str()));
    assert!(observed.contains("rust.lines"));
    assert!(observed.contains("TimedOut"));
    assert_rim_semantics(&outcome, "gate::run-refused");
    assert_caller_unchanged(&source);
}

#[test]
fn confirmed_mutation_results_cross_the_production_mapper() {
    let killed = MutationFixture::new(r#"[{"name":"Killed"}]"#);
    let killed_report = killed.report(Some(rate(1, 1)));
    assert!(matches!(
        killed_report.movement(),
        ReportMutationMovement::Shrank { .. }
    ));
    assert_eq!(killed_report.proposed_floor(), Some(rate(0, 1).into()));
    let admitted = kill_report(killed_report.clone());
    let Outcome::Reply(admitted) = admitted else {
        panic!("confirmed killed report must be admitted")
    };
    assert_eq!(admitted.body, killed_report);
    killed.assert_unchanged();

    let survived = MutationFixture::new(r#"[{"name":"Survived"}]"#);
    let survived_report = survived.report(Some(rate(0, 1)));
    assert!(matches!(
        survived_report.movement(),
        ReportMutationMovement::Regressed { .. }
    ));
    assert_eq!(survived_report.proposed_floor(), None);
    let rendered = survived_report.render();
    let refusal = kill_report(survived_report);
    assert_eq!(
        expect_refusal(&refusal, "gate::kill-regressed")
            .observed
            .as_deref(),
        Some(rendered.as_str())
    );
    survived.assert_unchanged();
}

#[test]
fn flaky_and_timed_out_mutants_retain_ordered_unconfirmed_evidence() {
    let fixture = MutationFixture::new(r#"[{"name":"Timeout"},{"name":"Flaky"}]"#);
    let report = fixture.report(Some(rate(0, 1)));
    assert_eq!(
        report.movement(),
        ReportMutationMovement::Unconfirmed {
            confirmed: 0,
            total: 2,
        }
    );
    assert_eq!(report.rate(), None);
    assert_eq!(report.proposed_floor(), None);
    assert_eq!(
        report
            .mutants()
            .iter()
            .map(|mutant| mutant.identity())
            .collect::<Vec<_>>(),
        ["Flaky", "Timeout"]
    );
    assert_flaky_evidence(&report);
    assert_timeout_evidence(&report);

    let rendered = report.render();
    let refusal = kill_report(report);
    assert_eq!(
        expect_refusal(&refusal, "gate::kill-unconfirmed")
            .observed
            .as_deref(),
        Some(rendered.as_str())
    );
    fixture.assert_unchanged();
}

fn short_bounds(owner: &str) -> Bounds {
    Bounds::new(owner, parser_deadlines(), Duration::from_millis(20), 4096)
        .expect("declare short parser bounds")
}

fn parser_deadlines() -> Deadlines {
    Deadlines::new(
        Duration::from_millis(150),
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .expect("declare process deadlines")
}

fn kill_deadlines() -> Deadlines {
    // All fixture processes except the explicitly nonterminating parser are
    // deterministic and immediate, so this outer budget exists only to keep
    // scheduler contention from deciding a preflight. cargo-mutants' own typed
    // timeout artifact is what exercises mutant-timeout mapping, so nothing here
    // needs an artificially tiny wall-clock deadline either. It deliberately
    // does not track the gate's own runtime deadline, which is sized for a cold
    // `trunk build --release` none of these fixtures run, and which here would
    // only make a genuinely hung fixture take four times as long to report.
    Deadlines::new(
        Duration::from_secs(30),
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("declare mutation process deadlines")
}

fn rate(numerator: u64, denominator: u64) -> SurvivorRate {
    SurvivorRate::new(numerator, denominator).expect("declare survivor rate")
}

fn expect_refusal<'a, B>(outcome: &'a Outcome<B>, reason: &str) -> &'a wire::Refusal {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("expected {reason} refusal")
    };
    assert_eq!(refusal.reason, reason);
    refusal
}

fn assert_rim_semantics(outcome: &Outcome<GateReport>, reason: &str) {
    let text = render::render(outcome, Form::Text).expect("render text refusal");
    let json = render::render(outcome, Form::Json).expect("render JSON refusal");
    assert_eq!((text.stream(), json.stream()), (Stream::Err, Stream::Err));
    assert_eq!(crate::exit_status(outcome), crate::REFUSAL_EXIT);
    assert_eq!(
        text.document(),
        format!(
            "spice: {reason}; restore the declared inputs and tools, then run the gate again\n"
        )
        .as_bytes()
    );
    let decoded: Outcome<GateReport> =
        wire::decode(json.document()).expect("decode JSON refusal rendering");
    assert_eq!(decoded, *outcome);
}

fn assert_flaky_evidence(report: &KillReport) {
    let flaky = &report.mutants()[0];
    assert_eq!(flaky.disposition(), ReportMutantDisposition::Unconfirmed);
    assert_eq!(
        flaky
            .attempts()
            .iter()
            .map(|attempt| (attempt.number(), attempt.seed(), attempt.kind()))
            .collect::<Vec<_>>(),
        [
            (1, 41, ReportAttemptKind::Killed),
            (2, 41, ReportAttemptKind::Survived),
        ]
    );
    assert!(matches!(
        flaky.attempts()[0].process().completion(),
        ReportCompletion::Exited { code: Some(0) }
    ));
    assert!(matches!(
        flaky.attempts()[1].process().completion(),
        ReportCompletion::Exited { code: Some(2) }
    ));
    assert!(
        flaky
            .attempts()
            .iter()
            .all(|attempt| matches!(attempt.outcome(), ReportOutcome::Present { .. }))
    );
}

fn assert_timeout_evidence(report: &KillReport) {
    let timeout = &report.mutants()[1];
    assert_eq!(timeout.disposition(), ReportMutantDisposition::Unconfirmed);
    assert_eq!(
        timeout
            .attempts()
            .iter()
            .map(|attempt| (attempt.number(), attempt.seed(), attempt.kind()))
            .collect::<Vec<_>>(),
        [
            (1, 41, ReportAttemptKind::TimedOut),
            (2, 41, ReportAttemptKind::TimedOut),
        ]
    );
    assert!(timeout.attempts().iter().all(|attempt| matches!(
        attempt.process().completion(),
        ReportCompletion::Exited { code: Some(3) }
    )));
    assert!(
        timeout
            .attempts()
            .iter()
            .all(|attempt| matches!(attempt.outcome(), ReportOutcome::Present { .. }))
    );
}

struct MutationFixture {
    source: Scratch,
    tools: Scratch,
    inventory: Vec<u8>,
}

impl MutationFixture {
    fn new(inventory: &str) -> Self {
        let inventory = inventory.as_bytes().to_vec();
        let source = Scratch::new("mutation-source");
        source.write("Cargo.toml", b"[workspace]\nmembers = []\n");
        source.write("src/lib.rs", SOURCE);
        source.write("baseline.toml", FLOOR);
        source.write("journal.sqlite3", JOURNAL);
        source.write("inventory.json", &inventory);

        let tools = Scratch::new("mutation-tools");
        tools.executable("cargo", FAKE_CARGO);
        tools.executable("cargo-mutants", FAKE_MUTANTS);
        Self {
            source,
            tools,
            inventory,
        }
    }

    fn report(&self, floor: Option<SurvivorRate>) -> KillReport {
        let tool = MutationTool::new(
            self.tools.path().join("cargo"),
            self.tools.path().join("cargo-mutants"),
            PINNED_CARGO_MUTANTS,
            Environment::empty(),
        )
        .expect("declare fixture mutation tool");
        let battery =
            MutationBattery::new(["src/lib.rs"], 2, 41).expect("declare fixture mutation battery");
        let bounds = KillBounds::new(
            "gate-rim-kill",
            kill_deadlines(),
            Duration::from_millis(20),
            4096,
        )
        .expect("declare fixture kill bounds");
        let run = Kill::new(tool, battery, bounds)
            .run(
                self.source.path(),
                &Ignore::parse("").expect("parse empty ignore"),
                floor,
            )
            .expect("complete fixture mutation run");
        let report = run.report();
        let encoded = wire::encode(&report).expect("encode coherent mutation report");
        let decoded: KillReport = wire::decode(&encoded).expect("decode coherent mutation report");
        assert_eq!(decoded, report);
        report
    }

    fn assert_unchanged(&self) {
        assert_caller_unchanged(&self.source);
        assert_eq!(self.source.read("inventory.json"), self.inventory);
        assert_eq!(self.tools.read("cargo"), FAKE_CARGO.as_bytes());
        assert_eq!(self.tools.read("cargo-mutants"), FAKE_MUTANTS.as_bytes());
    }
}

fn assert_caller_unchanged(source: &Scratch) {
    assert_eq!(source.read("src/lib.rs"), SOURCE);
    assert_eq!(source.read("baseline.toml"), FLOOR);
    assert_eq!(source.read("journal.sqlite3"), JOURNAL);
    assert!(!source.path().join(".spice-gate-mutants").exists());
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spice-gate-rim-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create unique rim fixture root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: &[u8]) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    fn executable(&self, relative: &str, contents: &str) {
        self.write(relative, contents.as_bytes());
        fs::set_permissions(
            self.path().join(relative),
            fs::Permissions::from_mode(0o755),
        )
        .expect("make fixture executable");
    }

    fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.path().join(relative)).expect("read fixture file")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove rim fixture root");
    }
}
