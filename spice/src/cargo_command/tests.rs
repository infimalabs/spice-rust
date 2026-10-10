use std::ffi::OsStr;

use super::*;
#[test]
fn developer_children_begin_outside_supervised_shell_state_and_may_opt_in() {
    let mut command = Command::new("cargo");
    command
        .env(steer::hook::BASH_ENV, "/fixture/bash-env")
        .env(steer::hook::ZDOTDIR, "/fixture/zdotdir")
        .env(vendor::OMP.thread_id_environment(), "fixture-omp");
    isolate_developer_environment(&mut command);

    for variable in spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES {
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == variable && value.is_none()),
            "developer child does not inherit {variable}"
        );
    }

    command.env(vendor::OMP.thread_id_environment(), "owned-omp");
    assert!(command.get_envs().any(|(name, value)| {
        name == vendor::OMP.thread_id_environment() && value == Some(OsStr::new("owned-omp"))
    }));
}
