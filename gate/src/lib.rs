//! Hermetic repository enforcement.
//!
//! The gate reads a foreign repository and nothing else. It holds no journal
//! and asks none: a receipt is a function of the selected tree and one current
//! [`Contract`], including the exact compiled set, bounds, baseline, and probed
//! tool identities. The same complete identities produce the same receipt on
//! any machine and at any hour.
//!
//! [`jurisdiction::Malady`] and [`ratchet::Malady`] are two closed sets under
//! one noun, and they stay module-qualified rather than being flattened here.
//! An ignore line and a ratchet line fail in ways that have nothing to say to
//! each other, and the qualifier is what keeps a reader from believing
//! otherwise.

pub mod battery;
pub mod camp;
pub mod config;
pub mod core;
pub mod hygiene;
pub mod input;
pub mod jurisdiction;
pub mod kill;
pub mod manifest;
pub mod measure;
pub mod message;
pub mod metrics;
pub mod names;
pub mod policy;
pub mod ratchet;
pub mod report;
pub mod state;
pub mod toolchain;
#[cfg(not(target_arch = "wasm32"))]
pub mod tree;
pub mod verdict;

pub use battery::{
    ActorEvidence, ArmDisposition, ArmEvidence, ArmExecution, ArmFamily, BatteryArmResult,
    BatteryContext, BatteryError, BatteryExecutor, BatteryInputs, BatteryManifest,
    BatteryManifestArm, BatteryReceipt, BatteryReport, BatteryScope, BatteryState,
    BatteryToolEvidence, BoundActorInputsEvidence, BoundEvidence, CANONICAL_BATTERY_MANIFEST,
    CompiledArm, ConfigurationEvidence, GATE_BATTERY_RECEIPT_VERSION, GATE_BATTERY_VERSION,
    LatchEvidence, ReceiptRefusal, ResultProvenance, UnsupportedReceipt, run_battery,
    supported_pairs,
};
pub use camp::{
    BoundRelation, CAMP_REPORT_VERSION, Camp, CampError, CampFinding, CampReport, CampRunError,
    CampStanding, DistributionReport, LocusDistance, SharedBound,
};
pub use config::{Config, WalkConfig};
pub use core::{
    CORE_GATE_IDS, CORE_REPORT_VERSION, ConfigInventoryEvidence, ConfigKeyEvidence,
    CoreArmDeclaration, CoreArmInput, CoreArmReport, CoreEvidence, CoreFinding, CoreInputError,
    CoreInputs, CoreObservation, CoreProvenance, CoreReport, CoreReportDecodeError, CoreStanding,
    CoreTimeoutContract, CoreToolContract, DocMeasurement, GitCommandContract,
    MergeIntegrityEvidence, MergeTreeCompletion, PlanPhaseEvidence,
    PredecessorObservation as CorePredecessorObservation, RepoDocLimit, RepoDocsEvidence,
    RepoShapeEvidence, StagingEvidence, core_declarations, evaluate_core,
};
#[cfg(not(target_arch = "wasm32"))]
pub use core::{ClaimCapture, CoreSource, acquire_core};
pub use hygiene::{
    AnalysisEvidence, AnalyzerEvidence, EvidenceState, ExclusionEvidence, HYGIENE_ARMS,
    HYGIENE_REPORT_VERSION, HygieneAnalysisError, HygieneAnalyzer, HygieneConfiguration,
    HygieneConfigurationSource, HygieneDeclaration, HygieneError, HygieneExclusion, HygieneFinding,
    HygieneFindingDetail, HygieneJurisdiction, HygieneMeasurement, HygieneOutcome,
    HygieneProvenance, HygieneReport, HygieneTraversal,
    MigrationRefusal as HygieneMigrationRefusal,
    PredecessorComparison as HygienePredecessorComparison,
    PredecessorComparisonError as HygienePredecessorComparisonError,
    PredecessorObservation as HygienePredecessorObservation,
    PredecessorResult as HygienePredecessorResult, PrivateCouplingException, PythonUnusedStatus,
    analyze_hygiene, compare_predecessor as compare_hygiene_predecessor,
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
pub use kill::{
    AttemptEvidence, AttemptKind, Kill, KillBounds, KillError, KillRun, MutantDisposition,
    MutantEvidence, MutationBattery, MutationInventory, MutationMovement, MutationTool,
    MutationToolIdentity, OutcomeEvidence, PINNED_CARGO_MUTANTS, ProcessCompletion,
    ProcessEvidence, RateError, StreamEvidence, SurvivorRate,
};
pub use manifest::{
    GATE_MANIFEST_VERSION, GateDeclaration, GateId, GateManifest, GateManifestError,
    MigrationError, MigrationErrorKind, OriginLayer, PREDECESSOR_GATE_RELEASE, PredecessorDocument,
    PredecessorInputs, Provenance, migrate_predecessor,
};
pub use measure::{
    Bounds, MeasureError, Measurement, Metric, MetricError, Parser, Policy, measurement_digest,
    render_measurements,
};
pub use message::{
    CommitMessageError, CommitMessagePolicy, OriginTrailer, OriginTrailerError, TrailerRejection,
    validate_commit_message,
};
pub use names::{RETIRED_NAMES, RetiredMatch, RetiredName, retired_matches};
pub use policy::{
    BoundLimits, BoundName, CompileError as PolicyCompileError, GlobalRatchet, MagicRule,
    MigrationError as PolicyMigrationError, OneLetterBehavior, Policy as RulePolicy, PolicyContext,
    PolicyDocument, RegexDialect, ResolveError as PolicyResolveError, ResolvedBound, ResolvedMagic,
    RuleDeclaration, RuleId, RuleSettings, RuleSource, RuleSourceKind, ScopeAxis, ScopeSelector,
    SelectionProvenance, SelectorSpecificity, migrate_predecessor_policy,
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
    PeerClaim, PendingState, PredecessorSources, RenameMap, StateDecision, StateError,
    StateErrorKind, StateKey, StateLedger, StateReceipt, current_state_path, varied_elastic_limit,
};
pub use toolchain::{
    ArmFinding, ArmMeasurement, ArmObservation, ArmOutcome, ConfigurationSource,
    EnvironmentLedgerKind, FindingDetail, JavascriptUnusedStatus, LOCAL_PATH_MARKER,
    MeasurementEngine, MigrationRefusal as ToolchainMigrationRefusal, PACKAGED_ENV_INCLUDES,
    PACKAGED_ENV_PROFILE, PACKAGED_JAVASCRIPT_EXEMPTIONS, PACKAGED_SCANNER_EXCLUDES,
    PACKAGED_TASTE_WORDS, PACKAGED_TEST_PATHS, PackagedEnvironmentProfile, PredecessorComparison,
    PredecessorComparisonError, PredecessorConfiguration, PredecessorFinding,
    PredecessorObservation, PredecessorResult, SourceLocus, TOOLCHAIN_ARMS,
    TOOLCHAIN_REPORT_VERSION, ToolEvidence, ToolRequirement, ToolchainDeclaration, ToolchainError,
    ToolchainReport, TreeInput, TreeJurisdiction, compare_predecessor, measure_toolchain,
};
#[cfg(not(target_arch = "wasm32"))]
pub use toolchain::{ToolchainSource, acquire_toolchain};
#[cfg(not(target_arch = "wasm32"))]
pub use tree::{SelectedTree, Selector, TreeError};
pub use verdict::{
    CurrentRun, GATE_RECEIPT_VERSION, PublishError, ReceiptDecodeError, ReceiptTool, Run, RunError,
    Standing, Verdict,
};
