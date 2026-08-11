//! Shared canonical workspace-test coordinator.

pub use gate::{
    WorkspaceTestError, WorkspaceTestExecution, prebuild_shared_at, run_prebuilt_shared_at,
    run_workspace_tests as run, run_workspace_tests_at as run_at,
};
