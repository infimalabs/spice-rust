//! Deterministic distribution build and staging orchestration.
//!
//! Heavyweight build adapters live at the rim. This crate owns their order,
//! pin admission, content identities, stage layout, receipt-free wire payload,
//! and the embedded app ZIP. A pin mismatch returns before any builder call.

mod authority;
mod canvas;
mod notes;
mod pins;
mod release;
mod stage;

pub use authority::{
    AuthorityError, GitCommand, GitMutation, RefState, ReleaseAuthority, ReleaseRefs,
    admit_authority, admit_workspace_publish_policy, current_authority, current_product_version,
};
pub use canvas::{Canvas, CanvasError, ObjectTarget, canvas_object, linked_archive};
pub use notes::{
    Correction, HIGHLIGHTS_PLACEHOLDER, INVENTORY_OPEN, Landing, NotesError, NotesInput,
    PRE_PROJECTION_RELEASE, ReviewStanding, curated_release_notes, release_notes,
};
pub use pins::{PinError, PinName, RequiredPins};
pub use release::{
    AcceptedOperation, Convergence, EffectError, EffectPort, ExecuteError, Execution, Operation,
    OperationReceipt, OperationState, PlanError, PlanInput, RegistryEvidence, ReleaseAction,
    ReleaseExecutor, ReleaseJournal, ReleaseObservations, ReleasePlan, SealedTarget, plan_release,
};
pub use stage::{
    BuildError, BuildStep, Builder, FilePublisher, Pipeline, PublishError, Publisher, StageError,
    StagedTarget,
};
pub use wire::manifest::ProductVersion;
