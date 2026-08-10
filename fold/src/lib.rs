//! Disposable, generation-stamped operational folds over journal facts.
//!
//! The registry contains only folds whose typed reducer is also the production
//! read path. `task::effort` is currently the sole member: the CLI and Canvas
//! resident surface decode its stored payload. Steering, consent, taste,
//! canary, verdict, other Canvas axes, and release remain with their direct
//! operational owners and deliberately have no competing fold-store aliases.

#![forbid(unsafe_code)]

mod contract;
mod inventory;
#[cfg(not(target_arch = "wasm32"))]
mod store;
#[cfg(not(target_arch = "wasm32"))]
mod task_effort;

pub use contract::{Availability, Beyond, Cursor, FactPlane, Horizon, Source};
pub use inventory::{Definition, FoldId, FoldIdError, INVENTORY, Inventory};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    Materialization, ReadOutcome, RebuildCause, Reducer, Reduction, Refusal, Store, StoreError,
};
#[cfg(not(target_arch = "wasm32"))]
pub use task_effort::{TaskEffortReducer, select_task_effort, task_effort_answer};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
