//! Deterministic distribution build and staging orchestration.
//!
//! Heavyweight build adapters live at the rim. This crate owns their order,
//! pin admission, content identities, stage layout, receipt-free wire payload,
//! and the linked spice rack. A pin mismatch returns before any builder call.

mod authority;
mod canvas;
mod darwin_signing;
mod notes;
mod pins;
mod release;
mod stage;
mod version;

pub use authority::{
    AuthorityError, GitCommand, GitMutation, RefState, ReleaseAuthority, ReleaseRefs,
    admit_authority, admit_workspace_publish_policy, current_authority,
};
pub use canvas::{Canvas, CanvasError, ObjectTarget, canvas_object, linked_archive};
pub use darwin_signing::{
    DEVELOPER_ID_PREFIX, DarwinSigning, DarwinSigningError, DeveloperIdentity, NotaryProfile,
    ReleaseConfig,
};
pub use notes::{
    Correction, HIGHLIGHTS_PLACEHOLDER, INVENTORY_OPEN, Landing, NotesError, NotesInput,
    PRE_PROJECTION_RELEASE, ReviewStanding, curated_release_notes, release_notes,
};
pub use pins::{PinError, PinName, RequiredPins};
pub use release::{
    AcceptedOperation, Convergence, EffectError, EffectPort, ExecuteError, Execution,
    OperationReceipt, OperationState, PlanError, PlanInput, ReleaseExecutor, ReleaseJournal,
    gate_receipt, plan_release, verify_plan,
};
pub use stage::{BuildError, BuildStep, Builder, Pipeline, StageError, StagedTarget};
pub use version::{
    Build, SourceInstant, SourceInstantError, TrackedVersion, TrackedVersionError,
    admit_tracked_version, current_tracked_version,
};
pub use wire::manifest::ProductVersion;
