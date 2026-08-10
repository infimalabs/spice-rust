//! A merge charged for what it wrote, and not for what it only carried across.
//!
//! The first two cases run one whole battery over the same repository shape: a short
//! Rust source is committed on the base both branches share, the second parent
//! grows it past every file-loc limit and adds a comparison-adjacent literal to
//! it, and `main` is merged with that. The two cases differ in one respect only
//! — whether the merging lane then touched that file — because that is the
//! entire distinction being claimed. A gate that read the first parent alone
//! reports the same two failures either way, since `HEAD` still carries the
//! short literal-free version the merge is not bringing forward.
//!
//! The passing case alone would also pass for a gate that simply stopped
//! measuring during a merge, which is why the refusing case appends one line to
//! that same inherited file and asserts both arms come back. Between them the
//! exemption is proved to be keyed on the file's bytes rather than on the
//! presence of `MERGE_HEAD`.
//!
//! Two adversarial cases close the boundaries around that rule. One restores a
//! path both parents deleted and proves their common base still supplies its
//! delta baseline. The other makes two invalid UTF-8 parent blobs decode to the
//! same replacement text, then proves only exact byte identity can grant the
//! inherited-file exemption.

use super::*;
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricId, MetricKey};

/// The source the second parent commits and the merge inherits verbatim.
const INHERITED: &str = "src/inherited.rs";

/// A second-parent source carrying complexity and environment debt.
const INHERITED_POLICY: &str = "src/inherited_policy.rs";

/// An extensionless second-parent file carrying one workstation-local path.
const INHERITED_LOCAL_PATH: &str = "notes/inherited-local-path";

/// A second-parent prose file carrying one discouraged word.
const INHERITED_TASTE: &str = "notes/inherited.md";

/// The short source the merge itself writes, standing for its resolutions.
const RESOLVED: &str = "src/resolved.rs";

/// A source present only in the parents' common base and restored by a merge.
const REINTRODUCED: &str = "src/reintroduced.rs";

/// A selected source whose exact bytes cannot be represented as UTF-8.
const INVALID: &str = "src/invalid.rs";

/// Lines that put `INHERITED` past `file_loc` however its headroom varies.
///
/// The tracked policy this fixture copies caps `file_loc` at 1000 lines and
/// varies the elastic limit per path and actor, so a file near the cap could
/// land either side of it. Four thousand cannot, which keeps the refusing case
/// below about the merge's own line rather than about a jitter draw.
const INHERITED_LINES: usize = 4000;

/// The comparison-adjacent literal the second parent commits into `INHERITED`.
const INHERITED_LITERAL: u64 = 4242;

/// The comparison-adjacent literal the merging lane adds on top of it.
///
/// Spelled apart from `INHERITED_LITERAL` so the refusing case cannot be
/// satisfied by the inherited one being counted twice.
const MERGED_LITERAL: u64 = 31337;

/// Lines that put the inherited routine beyond the compiled routine limit.
const INHERITED_ROUTINE_LINES: usize = 100;

/// One Rust source of the requested length holding one compared literal.
///
/// The literal is interpolated rather than written out because this file is
/// itself a `.rs` source in this repository's declared magic class: a number
/// spelled next to a comparison operator here would be a new magic number in
/// *this* tree, introduced by the test for the arm that reports them.
fn compared_source(literal: u64, lines: usize) -> String {
    let mut source = format!("pub fn admits(value: u64) -> bool {{\n    value >= {literal}\n}}\n");
    while source.lines().count() < lines {
        source.push_str("pub fn spacer() {}\n");
    }
    source
}

/// One over-limit routine whose environment name is absent from the ledger.
fn inherited_policy_source() -> String {
    let mut source = String::from("pub fn inherited_policy() {\n");
    for _ in 0..INHERITED_ROUTINE_LINES {
        source.push_str("    let _ = std::env::var(\"SPICE_MERGE_PARENT\");\n");
    }
    source.push_str("}\n");
    source
}

/// One workstation-local path authored wholly by the second parent.
fn inherited_local_path_source() -> String {
    format!("source = {}/peer/spice\n", crate::LOCAL_PATH_MARKER)
}

/// One discouraged prose token authored wholly by the second parent.
fn inherited_taste_source() -> &'static str {
    "the orphan fixture belongs to the side parent\n"
}

/// One short source whose final byte is deliberately not UTF-8.
fn invalid_source(byte: u8) -> Vec<u8> {
    let mut source = b"pub fn invalid() {}\n".to_vec();
    source.extend([byte, b'\n']);
    source
}

/// Valid UTF-8 whose decoded text matches either invalid parent after loss.
fn replacement_source() -> Vec<u8> {
    let mut source = b"pub fn invalid() {}\n".to_vec();
    source.extend("\u{fffd}\n".as_bytes());
    source
}

/// One arm's result within a complete battery report.
fn arm(report: &BatteryReport, id: GateId) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == id)
        .unwrap_or_else(|| panic!("the battery ran the {id:?} arm"))
}

/// One path's row for one metric within one arm.
fn row<'report>(
    report: &'report BatteryReport,
    id: GateId,
    path: &str,
    metric: MetricId,
) -> &'report MetricEvidence {
    let ArmEvidence::Metrics { report } = arm(report, id).evidence() else {
        panic!("the {id:?} arm projects structural metric evidence")
    };
    let key = MetricKey::path(path).expect("a selected path key");
    report
        .metrics()
        .iter()
        .find(|candidate| candidate.key() == &key && candidate.metric() == metric)
        .unwrap_or_else(|| {
            panic!(
                "the {id:?} arm carries a {} row for {path:?}, but its keys are {:?}",
                metric.as_str(),
                report
                    .metrics()
                    .iter()
                    .map(MetricEvidence::key)
                    .collect::<Vec<_>>()
            )
        })
}

/// The value one row measured, or a panic naming the state it carried instead.
fn measured(row: &MetricEvidence, path: &str) -> u64 {
    let MetricState::Measured { measurement } = row.state() else {
        panic!(
            "{path:?} was expected to measure {}, but carried {:?}",
            row.metric().as_str(),
            row.state()
        )
    };
    measurement.value()
}

/// Repository path one structural key names, absent for whole-arm evidence.
fn key_path(key: &MetricKey) -> Option<&str> {
    match key {
        MetricKey::Arm => None,
        MetricKey::Path(path) => Some(path),
        MetricKey::Function(function) => Some(function.path()),
    }
}

/// Toolchain findings retained by one battery arm.
fn findings(report: &BatteryReport, id: GateId) -> &[crate::ArmFinding] {
    let ArmEvidence::Toolchain { report } = arm(report, id).evidence() else {
        panic!("the {id:?} arm projects toolchain evidence")
    };
    &report.findings
}

/// Commit `INHERITED` on a side branch and merge it into `main`, uncommitted.
///
/// The merge is left with `MERGE_HEAD` in place and the merged content staged,
/// which is exactly the state `spice gate pre-commit` observes when Git invokes
/// it for the merge commit.
fn merged_repository(root: &Path) -> PathBuf {
    let repository = production_repository(root);
    let mut tracked: toml::Value = toml::from_str(
        &fs::read_to_string(repository.join("spice-rust.toml")).expect("fixture tracked policy"),
    )
    .expect("fixture tracked policy parses");
    tracked["policy"]["env_names"] = toml::Value::Array(Vec::new());
    fs::write(
        repository.join("spice-rust.toml"),
        toml::to_string(&tracked).expect("fixture tracked policy encodes"),
    )
    .expect("fixture tracked policy without unrelated ledger names");
    // Taken out of the index before either parent exists, so neither branch and
    // neither case below carries it. Left committed, the magic arm folds to its
    // refusal to read the class and stops answering the question asked here.
    for path in PREDECESSOR_CLASS_SOURCES {
        fixture_git(&repository, &["rm", "-q", "--cached", path]);
    }
    // Committed short and literal-free on the base both branches share, so the
    // side branch grows a file `HEAD` already carries. A file merely absent from
    // `HEAD` would reach the magic arm as having no baseline at all, which is
    // inapplicability rather than the regression this is about.
    fs::write(repository.join(INHERITED), "pub fn inherited() {}\n").expect("fixture base source");
    fs::write(repository.join(REINTRODUCED), "pub fn reintroduced() {}\n")
        .expect("fixture merge-base source");
    fixture_git(
        &repository,
        &["add", "spice-rust.toml", INHERITED, REINTRODUCED],
    );
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "drop the unreadable class"],
    );

    fixture_git(&repository, &["checkout", "-q", "-b", "side"]);
    fs::create_dir(repository.join("notes")).expect("fixture prose directory");
    fs::write(
        repository.join(INHERITED),
        compared_source(INHERITED_LITERAL, INHERITED_LINES),
    )
    .expect("fixture inherited source");
    fs::write(repository.join(INHERITED_POLICY), inherited_policy_source())
        .expect("fixture inherited policy source");
    fs::write(
        repository.join(INHERITED_LOCAL_PATH),
        inherited_local_path_source(),
    )
    .expect("fixture inherited local path");
    fs::write(repository.join(INHERITED_TASTE), inherited_taste_source())
        .expect("fixture inherited taste");
    fixture_git(&repository, &["rm", "-q", REINTRODUCED]);
    fixture_git(
        &repository,
        &[
            "add",
            INHERITED,
            INHERITED_POLICY,
            INHERITED_LOCAL_PATH,
            INHERITED_TASTE,
        ],
    );
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "side authors the file"],
    );

    fixture_git(&repository, &["checkout", "-q", "main"]);
    fixture_git(&repository, &["rm", "-q", REINTRODUCED]);
    fs::write(repository.join("src/lib.rs"), "pub fn fixture_moved() {}\n")
        .expect("fixture main-side edit");
    fixture_git(&repository, &["add", "src/lib.rs"]);
    fixture_git(&repository, &["commit", "-q", "-m", "main moves elsewhere"]);

    fixture_git(&repository, &["merge", "--no-commit", "--no-ff", "side"]);
    assert!(
        repository.join(".git/MERGE_HEAD").exists(),
        "the fixture merge must leave MERGE_HEAD in place"
    );
    // The merge writes one short source of its own, the way a real merge carries
    // its resolutions. Without it every selected path in this tree is some
    // parent's, the arm has nothing left to measure, and it would report that it
    // never applied — which passes the case below for the wrong reason and
    // cannot tell a per-file exemption from a merge-wide amnesty.
    fs::write(repository.join(RESOLVED), "pub fn resolved() {}\n").expect("the merge's resolution");
    fixture_git(&repository, &["add", RESOLVED]);
    repository
}

/// Merge a side-parent source whose bytes are not valid UTF-8.
fn invalid_merged_repository(root: &Path) -> PathBuf {
    let repository = production_repository(root);
    for path in PREDECESSOR_CLASS_SOURCES {
        fixture_git(&repository, &["rm", "-q", "--cached", path]);
    }
    fs::write(repository.join(INVALID), invalid_source(0x80)).expect("fixture base bytes");
    fixture_git(&repository, &["add", INVALID]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "commit the invalid-byte base"],
    );

    fixture_git(&repository, &["checkout", "-q", "-b", "side"]);
    fs::write(repository.join(INVALID), invalid_source(0x81)).expect("fixture side bytes");
    fixture_git(&repository, &["add", INVALID]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "side changes the exact byte"],
    );

    fixture_git(&repository, &["checkout", "-q", "main"]);
    fs::write(repository.join("src/lib.rs"), "pub fn main_moved() {}\n")
        .expect("fixture main edit");
    fixture_git(&repository, &["add", "src/lib.rs"]);
    fixture_git(&repository, &["commit", "-q", "-m", "main moves elsewhere"]);
    fixture_git(&repository, &["merge", "--no-commit", "--no-ff", "side"]);
    assert!(
        repository.join(".git/MERGE_HEAD").exists(),
        "the invalid-byte fixture must remain a pending merge"
    );
    repository
}

/// A merge is charged by no per-file arm for content it only carried.
///
/// The second parent's file is past `file_loc` and holds a literal `HEAD` has
/// never seen, so a gate comparing the staged tree against the first parent
/// alone fails both arms here. Both are asserted, and each is asserted at its
/// own row rather than only at the arm, so an arm that passed for some unrelated
/// reason cannot stand in for the exemption.
///
/// The file's measured length is *not* asserted, because the claim is that this
/// run does not measure it at all: it takes no bound decision, so it can neither
/// breach an elastic limit nor be handed the flex slice that a breach latches.
#[test]
fn a_merge_is_not_charged_for_a_file_its_second_parent_already_committed() {
    let fixture = Fixture::new();
    let repository = merged_repository(&fixture.root);

    let report = battery_report_with_changes(
        &repository,
        &[INHERITED_POLICY, INHERITED_LOCAL_PATH, INHERITED_TASTE],
    );

    let shape = row(&report, GateId::FileShape, INHERITED, MetricId::FileLines);
    assert!(
        matches!(shape.state(), MetricState::NotApplicable { .. }),
        "the inherited file takes no file-loc decision, but carried {:?}",
        shape.state()
    );
    assert!(
        measured(
            row(&report, GateId::FileShape, RESOLVED, MetricId::FileLines),
            RESOLVED,
        ) > 0,
        "the file the merge did write is measured beside the one it inherited"
    );
    assert_eq!(
        arm(&report, GateId::FileShape).state(),
        BatteryState::Passed,
        "the file-shape arm admits a merge that authored no over-limit shape: {}",
        arm(&report, GateId::FileShape).reason()
    );
    assert_eq!(
        measured(
            row(
                &report,
                GateId::MagicNumbers,
                INHERITED,
                MetricId::MagicNumberRegressions,
            ),
            INHERITED,
        ),
        0,
        "a literal committed by the second parent is not introduced by the merge"
    );
    assert_eq!(
        arm(&report, GateId::MagicNumbers).state(),
        BatteryState::Passed,
        "the magic-number arm admits a merge that introduced no literal: {}",
        arm(&report, GateId::MagicNumbers).reason()
    );
    let ArmEvidence::Metrics { report: complexity } = arm(&report, GateId::Complexity).evidence()
    else {
        panic!("the complexity arm projects structural metric evidence")
    };
    assert!(
        complexity
            .metrics()
            .iter()
            .all(|metric| key_path(metric.key()) != Some(INHERITED_POLICY)),
        "the inherited routine takes no complexity decision"
    );
    assert_eq!(
        arm(&report, GateId::Complexity).state(),
        BatteryState::Passed,
        "the complexity arm admits a routine authored by the second parent: {}",
        arm(&report, GateId::Complexity).reason()
    );
    assert_eq!(
        arm(&report, GateId::EnvPolicy).state(),
        BatteryState::NotApplicable,
        "the environment policy scans no inherited staged path: {}",
        arm(&report, GateId::EnvPolicy).reason()
    );
    assert!(
        findings(&report, GateId::EnvPolicy).is_empty(),
        "the inherited environment access produces no policy finding"
    );
    assert_eq!(
        arm(&report, GateId::EnvNameLedger).state(),
        BatteryState::Passed,
        concat!(
            "the environment ledger admits an unaccounted name referenced only by inherited ",
            "files: {}; findings={:?}"
        ),
        arm(&report, GateId::EnvNameLedger).reason(),
        findings(&report, GateId::EnvNameLedger),
    );
    assert!(
        findings(&report, GateId::EnvNameLedger).is_empty(),
        "the inherited environment name produces no ledger finding"
    );
    for id in [GateId::LocalPaths, GateId::Taste] {
        assert_eq!(
            arm(&report, id).state(),
            BatteryState::NotApplicable,
            "{} scans no path inherited byte-identically from a merge parent: {}",
            id.as_str(),
            arm(&report, id).reason(),
        );
        assert!(
            findings(&report, id).is_empty(),
            "{} produces no finding for a path the merge did not author",
            id.as_str(),
        );
    }
}

/// The same merge is charged in full once it writes to that same file.
///
/// One line is appended and one literal added to the file the case above let
/// through. Both arms refuse, which is what separates a per-file authorship
/// question from a merge-wide amnesty: the exemption is keyed on the bytes being
/// a parent's, and these bytes are no parent's.
///
/// An edited file is then measured against the nearest parent that carries it,
/// so the count below is both literals rather than the one this lane typed. That
/// is the stated rule and not an oversight: the guarantee a merge is given is
/// for the files it left alone, and a lane that edits an inherited file is
/// presenting the result as its own. Narrowing it to what the merge typed would
/// mean asking each parent separately, which is a different contract than the
/// one this fixture pins.
#[test]
fn a_merge_is_charged_for_the_line_it_adds_to_an_inherited_file() {
    let fixture = Fixture::new();
    let repository = merged_repository(&fixture.root);
    let mut resolved = compared_source(INHERITED_LITERAL, INHERITED_LINES);
    resolved.push_str(&format!(
        "pub fn merged(value: u64) -> bool {{\n    value >= {MERGED_LITERAL}\n}}\n"
    ));
    fs::write(repository.join(INHERITED), &resolved).expect("the merge's own edit");
    let mut policy = inherited_policy_source();
    policy.push_str("// resolved by the merge\n");
    fs::write(repository.join(INHERITED_POLICY), policy).expect("the merge's policy edit");
    let mut local_path = inherited_local_path_source();
    local_path.push_str("merge touched this file\n");
    fs::write(repository.join(INHERITED_LOCAL_PATH), local_path)
        .expect("the merge's local-path edit");
    let mut taste = inherited_taste_source().to_owned();
    taste.push_str("merge touched this prose\n");
    fs::write(repository.join(INHERITED_TASTE), taste).expect("the merge's taste edit");
    fixture_git(
        &repository,
        &[
            "add",
            INHERITED,
            INHERITED_POLICY,
            INHERITED_LOCAL_PATH,
            INHERITED_TASTE,
        ],
    );

    let report = battery_report_with_changes(
        &repository,
        &[INHERITED_POLICY, INHERITED_LOCAL_PATH, INHERITED_TASTE],
    );

    let lines = measured(
        row(&report, GateId::FileShape, INHERITED, MetricId::FileLines),
        INHERITED,
    );
    assert_eq!(
        lines,
        u64::try_from(resolved.lines().count()).expect("a fixture source fits u64"),
        "a file the merge edited is measured rather than passed over"
    );
    assert_eq!(
        arm(&report, GateId::FileShape).state(),
        BatteryState::Failed,
        "the file-shape arm still refuses the shape this merge now owns"
    );
    assert_eq!(
        measured(
            row(
                &report,
                GateId::MagicNumbers,
                INHERITED,
                MetricId::MagicNumberRegressions,
            ),
            INHERITED,
        ),
        [INHERITED_LITERAL, MERGED_LITERAL].len() as u64,
        "a file the merge edited is measured against the nearest parent carrying \
         it, which is HEAD, where neither literal is present yet"
    );
    assert_eq!(
        arm(&report, GateId::MagicNumbers).state(),
        BatteryState::Failed,
        "the magic-number arm still refuses the literal this merge introduced"
    );
    for id in [
        GateId::Complexity,
        GateId::LocalPaths,
        GateId::Taste,
        GateId::EnvPolicy,
        GateId::EnvNameLedger,
    ] {
        assert_eq!(
            arm(&report, id).state(),
            BatteryState::Failed,
            "{} still charges a merge that edited the inherited file: {}",
            id.as_str(),
            arm(&report, id).reason(),
        );
    }
    assert!(
        !findings(&report, GateId::EnvPolicy).is_empty(),
        "the edited file's environment accesses are charged"
    );
    assert!(
        !findings(&report, GateId::EnvNameLedger).is_empty(),
        "the edited file's unaccounted environment name is charged"
    );
    assert!(
        findings(&report, GateId::LocalPaths)
            .iter()
            .any(|finding| finding.path.as_deref() == Some(INHERITED_LOCAL_PATH)),
        "the edited extensionless file's inherited local path is charged"
    );
    assert!(
        findings(&report, GateId::Taste)
            .iter()
            .any(|finding| finding.path.as_deref() == Some(INHERITED_TASTE)),
        "the edited prose file's inherited discouraged word is charged"
    );
}

/// A merge-base file remains a delta baseline after both parents delete it.
#[test]
fn a_merge_is_charged_when_it_reintroduces_a_path_only_the_merge_base_carries() {
    let fixture = Fixture::new();
    let repository = merged_repository(&fixture.root);
    fs::write(
        repository.join(REINTRODUCED),
        compared_source(MERGED_LITERAL, 4),
    )
    .expect("the merge reintroduces the base path");
    fixture_git(&repository, &["add", REINTRODUCED]);

    let report = battery_report(&repository);

    assert_eq!(
        measured(
            row(
                &report,
                GateId::MagicNumbers,
                REINTRODUCED,
                MetricId::MagicNumberRegressions,
            ),
            REINTRODUCED,
        ),
        1,
        "a path absent from both parents still regresses from their common base"
    );
    assert_eq!(
        arm(&report, GateId::MagicNumbers).state(),
        BatteryState::Failed,
        "the merge is charged for the literal it reintroduced over the merge base"
    );
}

/// Lossy text equality cannot grant an exact-byte merge exemption.
#[test]
fn a_byte_distinct_invalid_source_is_measured_instead_of_marked_inherited() {
    let fixture = Fixture::new();
    let repository = invalid_merged_repository(&fixture.root);
    let parent = invalid_source(0x81);
    let current = replacement_source();
    assert_ne!(parent, current, "the fixture needs byte-distinct content");
    assert_eq!(
        String::from_utf8_lossy(&parent),
        String::from_utf8_lossy(&current),
        "the fixture needs text that only exact comparison can distinguish"
    );
    fs::write(repository.join(INVALID), current).expect("the merge's exact-byte edit");
    fixture_git(&repository, &["add", INVALID]);

    let report = battery_report(&repository);

    assert!(
        measured(
            row(&report, GateId::FileShape, INVALID, MetricId::FileBytes),
            INVALID,
        ) > 0,
        "byte-distinct content was mistaken for an inherited parent blob"
    );
}
