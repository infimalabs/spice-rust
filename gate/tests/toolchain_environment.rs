use gate::{
    ArmFinding, ArmOutcome, EnvironmentLedgerKind, FindingDetail, GateId, SourceLocus,
    TOOLCHAIN_ARMS, measure_toolchain,
};

mod support;

use support::toolchain::{
    READ, SETTINGS, SHARED_NAME, WAIVED_NAME, WORKER, acquired, arm, environment_repository,
    python_idiom, toolchain_directory,
};
use support::{fixture_column, git_commit, set_environment_names};

/// Every unwaived access is one finding, and the waived one is none.
///
/// The waiver is the whole point of the arm: an access is admitted by being
/// written down, so an arm that reported the annotated read too would make the
/// inventory unreadable and an arm that reported neither would make it empty.
/// Past the waiver this holds the arm to one finding per access rather than per
/// line, over a line whose two reads only one extractor each can name and a
/// line whose two reads share a name and differ by column alone. Admission
/// refuses findings that agree in every field, so the second of those two is
/// retained only because the column tells it apart.
#[test]
fn the_environment_policy_arm_reports_only_the_unwaived_access() {
    let repository = environment_repository("toolchain-env-policy");
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[SETTINGS],
    );

    let observation = arm(&observations, GateId::EnvPolicy);
    assert_eq!(
        observation.outcome,
        ArmOutcome::Findings,
        "the policy arm read a tree with one unwaived access: {}",
        observation.reason
    );
    let idiom = python_idiom();
    let access = |line: u32, source: &str, needle: &str, name: &str| ArmFinding {
        path: Some(SETTINGS.to_owned()),
        line: Some(line),
        column: Some(fixture_column(source, needle)),
        detail: FindingDetail::EnvironmentAccess {
            name: name.to_owned(),
            source: source.to_owned(),
        },
    };
    assert_eq!(
        observation.findings,
        [
            access(4, READ, SHARED_NAME, SHARED_NAME),
            access(5, support::toolchain::MIXED, SHARED_NAME, SHARED_NAME),
            access(5, support::toolchain::MIXED, "os.environ", &idiom),
            access(6, support::toolchain::DYNAMIC, "os.environ", &idiom),
            access(6, support::toolchain::DYNAMIC, "os.getenv", &idiom),
        ],
        "the policy arm named something other than the five unwaived accesses"
    );

    measure_toolchain(observations).expect("environment observations are admitted");
}

/// A name the profile never declared is unaccounted at every locus reading it.
///
/// The ledger is repository-wide, so it carries loci instead of a locus, and
/// dropping either one would understate where the environment shapes behavior.
/// Nothing was selected as changed here, which is also what proves the policy
/// arm reaches `NotApplicable` only through its own declared predicate.
#[test]
fn the_environment_ledger_arm_accounts_an_unaccounted_name_at_every_locus() {
    let repository = environment_repository("toolchain-env-ledger");
    set_environment_names(&repository, &[WAIVED_NAME]);
    git_commit(&repository, "remove the shared environment declaration");
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );

    let observation = arm(&observations, GateId::EnvNameLedger);
    assert_eq!(
        observation.outcome,
        ArmOutcome::Findings,
        "the ledger arm read a tree declaring none of the names it uses: {}",
        observation.reason
    );
    let accounted = observation
        .findings
        .iter()
        .find(|finding| match &finding.detail {
            FindingDetail::EnvironmentLedger { name, .. } => name == SHARED_NAME,
            _ => false,
        })
        .unwrap_or_else(|| panic!("the ledger never accounted {SHARED_NAME}"));
    assert_eq!(
        (accounted.path.as_deref(), accounted.line, accounted.column),
        (None, None, None),
        "a ledger finding is repository-wide and carries no locus of its own"
    );
    let FindingDetail::EnvironmentLedger {
        mismatch,
        references,
        ..
    } = &accounted.detail
    else {
        panic!("the accounted finding stopped being a ledger finding");
    };
    assert_eq!(*mismatch, EnvironmentLedgerKind::Unaccounted);
    assert_eq!(
        references,
        &[
            SourceLocus {
                path: SETTINGS.to_owned(),
                line: 4,
            },
            SourceLocus {
                path: SETTINGS.to_owned(),
                line: 5,
            },
            SourceLocus {
                path: WORKER.to_owned(),
                line: 3,
            },
        ]
    );

    let policy = arm(&observations, GateId::EnvPolicy);
    assert_eq!(
        policy.outcome,
        ArmOutcome::NotApplicable,
        "the policy arm skipped a tree for a reason other than its predicate: {}",
        policy.reason
    );
    let applicability = TOOLCHAIN_ARMS
        .into_iter()
        .find(|declaration| declaration.id() == GateId::EnvPolicy)
        .expect("env-policy is declared")
        .jurisdiction()
        .applicability();
    assert!(
        policy.reason.contains(applicability),
        "the skip named something other than the declared predicate: {}",
        policy.reason
    );

    measure_toolchain(observations).expect("environment observations are admitted");
}
