//! Hermetic repository enforcement.
//!
//! The gate reads one selected repository and its Git-common quality-state
//! ledger. A report remains a function of the selected tree, one current
//! [`Contract`], and the exact run-start ledger; proposed state is published
//! separately and atomically only after the enclosing run accepts.
//!
//! [`jurisdiction::Malady`] and [`ratchet::Malady`] are two closed sets under
//! one noun, and they stay module-qualified rather than being flattened here.
//! An ignore line and a ratchet line fail in ways that have nothing to say to
//! each other, and the qualifier is what keeps a reader from believing
//! otherwise.

pub mod battery;
pub mod camp;
mod change_selection;
pub mod config;
pub mod core;
#[cfg(not(target_arch = "wasm32"))]
mod held_root;
pub mod hygiene;
pub mod input;
pub mod jurisdiction;
pub mod kill;
mod long_lines;
pub mod manifest;
pub mod measure;
pub mod message;
pub mod metrics;
pub mod names;
pub mod policy;
pub mod ratchet;
pub mod report;
#[cfg(not(target_arch = "wasm32"))]
mod semantic_closure;
pub mod state;
#[cfg(not(target_arch = "wasm32"))]
mod structural;
pub mod test_evidence;
pub mod toolchain;
#[cfg(not(target_arch = "wasm32"))]
pub mod tree;
pub mod verdict;
#[cfg(not(target_arch = "wasm32"))]
mod workspace_tests;

/// Hard ceiling on canonical workspace-test shard workers.
pub const WORKSPACE_TEST_CONCURRENCY_CAP: usize = 4;
/// Higher shard ceiling reserved for a completed shared-output prebuild.
///
/// Six lanes parallelize already-built test processes without admitting six
/// cold Rust compilers or consuming the outer scheduler slot deliberately kept
/// free of Cargo work during compiler setup. The workspace-test coordinator
/// accepts this ceiling only when its private shared-output authority is
/// present.
pub const PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP: usize = 6;

pub use battery::{
    ActorEvidence, ArmDisposition, ArmEvidence, ArmExecution, ArmFamily,
    BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryArmResult, BatteryContext, BatteryError,
    BatteryExecutor, BatteryInputs, BatteryManifest, BatteryManifestArm, BatteryReceipt,
    BatteryRefusalFold, BatteryReport, BatteryScope, BatteryState, BatteryToolEvidence,
    BoundActorInputsEvidence, BoundEvidence, CANONICAL_BATTERY_MANIFEST, CompiledArm,
    ConfigurationEvidence, FocusedArmReport, GATE_BATTERY_RECEIPT_VERSION, GATE_BATTERY_VERSION,
    GateFinding, LatchEvidence, ReceiptRefusal, ResultProvenance, UnsupportedReceipt, run_battery,
    run_battery_arm, run_focused_arm, supported_pairs,
};
#[cfg(not(target_arch = "wasm32"))]
pub use battery::{BatteryCapture, BatteryStateRun, RepositoryExecutionEvent, StateRunScope};
pub use camp::{
    BoundRelation, CAMP_REPORT_VERSION, Camp, CampError, CampFinding, CampReport, CampRunError,
    CampStanding, DistributionReport, LocusDistance, SharedBound,
};
pub use change_selection::{
    CHANGE_SELECTION_REPORT_VERSION, ChangeArmReach, ChangeReachAuthority, ChangeReachEdge,
    ChangeReachMode, ChangeReachObject, ChangeReachOrigin, ChangeSelectionEdge,
    ChangeSelectionError, ChangeSelectionMode, ChangeSelectionPolicy, ChangeSelectionPolicyInputs,
    ChangeSelectionReport, select_change_gates, select_current_source_change_gates,
    select_current_source_change_gates_with_full_verifier,
};
pub use config::{Config, WalkConfig};
#[cfg(not(target_arch = "wasm32"))]
pub use core::{
    ActorCapture, CONFIG_KEY_DECLARATIONS, ClaimCapture, ConfigKeyDeclaration, CoreSource,
    acquire_core,
};
pub use core::{
    CORE_GATE_IDS, CORE_REPORT_VERSION, ConfigInventoryEvidence, ConfigKeyEvidence,
    CoreArmDeclaration, CoreArmInput, CoreArmReport, CoreEvidence, CoreFinding, CoreInputError,
    CoreInputs, CoreObservation, CoreProvenance, CoreReport, CoreReportDecodeError, CoreStanding,
    CoreTimeoutContract, CoreToolContract, DocMeasurement, GitCommandContract,
    MergeIntegrityEvidence, MergeTreeCompletion, PlanPhaseEvidence,
    PredecessorObservation as CorePredecessorObservation, RepoDocLimit, RepoDocsEvidence,
    RepoShapeEvidence, StagingEvidence, core_declarations, evaluate_core,
};
pub use hygiene::{
    AnalysisEvidence, AnalyzerEvidence, EvidenceState, ExclusionEvidence, HYGIENE_ARMS,
    HYGIENE_REPORT_VERSION, HygieneAnalyzer, HygieneConfigurationSource, HygieneDeclaration,
    HygieneError, HygieneFinding, HygieneFindingDetail, HygieneJurisdiction, HygieneMeasurement,
    HygieneOutcome, HygieneProvenance, HygieneReport, HygieneTraversal,
    MigrationRefusal as HygieneMigrationRefusal, ModuleReach,
    PredecessorComparison as HygienePredecessorComparison,
    PredecessorComparisonError as HygienePredecessorComparisonError,
    PredecessorObservation as HygienePredecessorObservation,
    PredecessorResult as HygienePredecessorResult, PythonUnusedStatus, SuppressedPath,
    compare_predecessor as compare_hygiene_predecessor,
};
#[cfg(not(target_arch = "wasm32"))]
pub use hygiene::{
    HygieneAnalysisError, HygieneConfiguration, HygieneExclusion, HygieneSource,
    PrivateCouplingException, acquire_hygiene,
};
pub use input::{
    BASELINE_PATH, BASELINE_VERSION, Baseline, BaselineError, Contract, ContractError,
    ExecutableDeclaration, FieldError, Floor, FloorDeclaration, FloorKind, GateSet, GateSetError,
    KillProfile, LONG_LINE_LIMIT, POLICY_PATH, PolicyError, ToolError, ToolIdentity,
    declared_mutation_tools, declared_tools,
};
pub use jurisdiction::{
    Admission, Entry, Ignore, IgnoreError, Jurisdiction, Rule, Shape, WalkError,
};
#[cfg(not(target_arch = "wasm32"))]
pub use semantic_closure::{
    GATE_SEMANTIC_CLOSURE_VERSION, GateArmClosure, GateClosureComparison, GateClosureSource,
    GateDependencyMode, GateSemanticClosure, GateSourceClass, GateSourceDependency,
};
// Projection is consumed only by native hygiene acquisition, but the shared
// jurisdiction module is deliberately type-checked on the browser target too.
#[cfg(target_arch = "wasm32")]
const _: fn(&Jurisdiction, &[String]) -> Jurisdiction = Jurisdiction::select_paths;
pub use kill::{
    AttemptEvidence, AttemptKind, Kill, KillBounds, KillError, KillRun, MutantDisposition,
    MutantEvidence, MutationBattery, MutationInventory, MutationMovement, MutationTool,
    MutationToolIdentity, OutcomeEvidence, PINNED_CARGO_MUTANTS, ProcessCompletion,
    ProcessEvidence, RateError, StreamEvidence, SurvivorRate,
};
pub use long_lines::LongLineFinding;
pub use manifest::{
    GATE_MANIFEST_VERSION, GateDeclaration, GateId, GateManifest, GateManifestError,
    MigrationError, MigrationErrorKind, OriginLayer, PREDECESSOR_GATE_RELEASE, PredecessorDocument,
    PredecessorInputs, Provenance, migrate_predecessor,
};
pub use measure::{
    Bounds, JurisdictionMode, MeasureError, Measurement, Metric, MetricError, Parser, Policy,
    measurement_digest, render_measurements,
};
pub use message::{
    CommitMessageError, CommitMessagePolicy, OriginTrailer, OriginTrailerError, TrailerRejection,
    validate_commit_message,
};
pub use names::{
    RETIRED_NAMES, RetiredMatch, RetiredName, retired_command_matches, retired_matches,
};
pub use policy::{
    Applicability, BoundLimits, BoundName, CompileError as PolicyCompileError, FileClasses,
    FileShapePolicy, GlobalRatchet, LanguagesPolicy, MagicRule,
    MigrationError as PolicyMigrationError, NARROWEST_FILE_CLASSES, OneLetterBehavior,
    PackagePolicy, Policy as RulePolicy, PolicyContext, PolicyDocument, RegexDialect,
    RepoTruthPolicy, ResolveError as PolicyResolveError, ResolvedBound, ResolvedMagic,
    RuleDeclaration, RuleId, RuleSettings, RuleSource, RuleSourceKind, ScopeAxis, ScopeSelector,
    SelectionProvenance, SelectorSpecificity, migrate_predecessor_policy,
    tracked_path_selector_matches, validate_tracked_path_selector,
};
pub use ratchet::{Finding, Movement, Ratchet, RatchetError};
pub use report::{
    GATE_REPORT_VERSION, GateReport, KILL_REPORT_VERSION, KillReport, ReportAttempt,
    ReportAttemptKind, ReportCompletion, ReportDecodeError, ReportFloor, ReportMeasurement,
    ReportMutant, ReportMutantDisposition, ReportMutationInventory, ReportMutationMovement,
    ReportOutcome, ReportProcess, ReportRate, ReportStream,
};
pub use state::{
    ActorInputs, ActorResolution, ActorSource, BoundDecision, BoundError, BoundRule, ClaimDecision,
    ClaimStatus, CurrentState, KeyError, LatchEntry, LatchKind, LatchObservation, LatchTransition,
    PeerClaim, PendingState, RenameMap, StateDecision, StateError, StateErrorKind, StateKey,
    StateLedger, StateReceipt, current_state_path, varied_elastic_limit,
};
pub use test_evidence::{
    CanonicalTestExecution, CoverageCompleteness, CoveredSource, MutationCoverageFinding,
    MutationCoverageReport, MutationExecution, MutationObservation, StandingMutationWaiver,
    SubsumptionRelation, SuiteSeamFinding, SuiteSeamReport, TEST_EVIDENCE_REPORT_VERSION,
    TEST_SELECTION_REPORT_VERSION, TestCoverage, TestEvidenceError, TestEvidenceGraph,
    TestEvidenceReport, TestEvidenceStanding, TestSelectionEdge, TestSelectionReport,
    TestSubsumptionFinding, TestSubsumptionReport,
};
/// Private `cargo xtask app` argument used by the native full verifier to
/// publish a selected-source app build into its live worktree.
pub const VERIFIER_APP_PUBLISH_FLAG: &str = "--spice-verifier-publish-root";
/// Complete stdout record emitted after Trunk's Cargo process succeeds but
/// before selected-source validation and atomic app publication finish.
pub const VERIFIER_APP_COMPILER_READY_RECORD: &str = "[app] compiler-ready";
#[cfg(not(target_arch = "wasm32"))]
pub use structural::{
    StructuralCapture, StructuralMatch, StructuralPattern, StructuralScanError, scan_structural,
};
pub use toolchain::{
    ArmFinding, ArmMeasurement, ArmObservation, ArmOutcome, ArmTiming, ConfigurationSource,
    CsharpUnusedStatus, ENV_NAME_LITERAL_MARKER, EnvironmentLedgerKind, FindingDetail,
    JavascriptUnusedStatus, LOCAL_PATH_MARKER, MeasurementEngine,
    MigrationRefusal as ToolchainMigrationRefusal, PACKAGED_ENV_INCLUDES, PACKAGED_ENV_PROFILE,
    PACKAGED_JAVASCRIPT_EXEMPTIONS, PACKAGED_SCANNER_EXCLUDES, PACKAGED_TASTE_WORDS,
    PACKAGED_TEST_PATHS, PackagedEnvironmentProfile, PredecessorComparison,
    PredecessorComparisonError, PredecessorConfiguration, PredecessorFinding,
    PredecessorObservation, PredecessorResult, ProbeFailure, SourceLocus, TOOLCHAIN_ARMS,
    TOOLCHAIN_REPORT_VERSION, ToolEvidence, ToolRequirement, ToolchainDeclaration, ToolchainError,
    ToolchainReport, TreeInput, TreeJurisdiction, compare_predecessor, measure_toolchain,
};
#[cfg(not(target_arch = "wasm32"))]
pub use toolchain::{
    PreparedVerifierCompiler, RimCapture, RimDirectoryError, RustAnalysisWorkspace,
    ToolchainSource, VerifierCompilerContract, VerifierCompilerEvidence,
    VerifierCompilerProcessEvidence, VerifierCompilerRun, VerifierCompilerStage, acquire_toolchain,
    admit_verifier_compiler_receipt, encode_verifier_compiler_receipt, formatter_edition,
    formatter_sources, semantic_verifier_compiler_authorities,
};
#[cfg(not(target_arch = "wasm32"))]
pub use tree::{SelectedTree, Selector, TreeError};
pub use verdict::{
    CurrentRun, GATE_RECEIPT_VERSION, PublishError, ReceiptDecodeError, ReceiptTool, Run, RunError,
    Standing, Verdict,
};
#[cfg(not(target_arch = "wasm32"))]
pub use workspace_tests::{
    WorkspaceTestError, WorkspaceTestExecution, WorkspaceTestRoots,
    cargo_workspace_tests_applicable, prebuild_shared_at, run as run_workspace_tests,
    run_at as run_workspace_tests_at,
    run_exhaustive_selected_at as run_exhaustive_selected_workspace_tests_at,
    run_prebuilt_shared_at, run_selected_at as run_selected_workspace_tests_at,
};
