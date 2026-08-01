//! Disposable, generation-stamped materializations over journal facts.

#![forbid(unsafe_code)]

mod contract;
mod inventory;
#[cfg(not(target_arch = "wasm32"))]
mod store;

pub use contract::{
    AnswerContent, Availability, Beyond, ConfigSource, Cursor, FactPlane, HeadSource, Horizon,
    Recovery, Source, TapeSource,
};
pub use inventory::{Definition, FoldId, INVENTORY, Inventory, RingDefinition, RingId};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    Materialization, ReadOutcome, RebuildCause, Reducer, Reduction, Refusal, Store, StoreError,
};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
