//! Deterministic distribution build and staging orchestration.
//!
//! Heavyweight build adapters live at the rim. This crate owns their order,
//! pin admission, content identities, stage layout, and receipt-free wire
//! payload. A pin mismatch returns before any builder method is called.

mod authority;
mod notes;
mod pins;
mod stage;

pub use authority::{
    AuthorityError, GitCommand, GitMutation, RefState, ReleaseAuthority, ReleaseRefs,
    admit_authority, admit_workspace_publish_policy, current_authority,
};
pub use notes::{NotesError, release_notes};
pub use pins::{PinError, PinName, RequiredPins};
pub use stage::{
    BuildError, BuildStep, Builder, FilePublisher, Pipeline, PublishError, Publisher, StageError,
    StagedTarget,
};
pub use wire::manifest::ProductVersion;
