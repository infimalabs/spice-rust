//! Disposable, generation-stamped materializations over journal facts.

#![forbid(unsafe_code)]

mod contract;
mod inventory;
#[cfg(not(target_arch = "wasm32"))]
mod store;

pub use contract::{
    Availability, Beyond, ConfigSource, Cursor, FactPlane, FoldId, HeadSource, Horizon, Recovery,
    RingId, Source, TapeSource,
};
pub use inventory::{Definition, INVENTORY, Inventory, RingDefinition};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    Materialization, ReadOutcome, RebuildCause, Reducer, Reduction, Refusal, Store, StoreError,
};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
