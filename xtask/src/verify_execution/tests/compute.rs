//! Canonical workspace-test compute-ratchet decisions.

use super::super::action::execute_workspace_tests;
use super::super::{CommandRun, ProcessOutput, Status};
use crate::{VerifyError, WORKSPACE_TEST_COMPUTE_BUDGET, WORKSPACE_TEST_LABEL};
use std::time::Duration;

#[test]
fn canonical_workspace_test_compute_admits_at_the_frozen_budget() {
    let completion = execute_workspace_tests(
        Ok(CommandRun::Exited(ProcessOutput {
            status: Status {
                success: true,
                code: Some(0),
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            usage: Some(sill::ProcessUsage::new(
                WORKSPACE_TEST_COMPUTE_BUDGET,
                Duration::ZERO,
            )),
        })),
        WORKSPACE_TEST_LABEL,
    );

    assert!(completion.result.is_ok());
    let output = String::from_utf8(completion.output).expect("compute record is UTF-8");
    assert!(output.contains(&format!(
        "compute disposition=admitted user_cpu_ms={} system_cpu_ms=0 total_cpu_ms={} budget_cpu_ms={}",
        WORKSPACE_TEST_COMPUTE_BUDGET.as_millis(),
        WORKSPACE_TEST_COMPUTE_BUDGET.as_millis(),
        WORKSPACE_TEST_COMPUTE_BUDGET.as_millis(),
    )));
}

#[test]
fn canonical_workspace_test_compute_refuses_above_the_frozen_budget() {
    let measured = WORKSPACE_TEST_COMPUTE_BUDGET + Duration::from_millis(1);
    let completion = execute_workspace_tests(
        Ok(CommandRun::Exited(ProcessOutput {
            status: Status {
                success: true,
                code: Some(0),
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            usage: Some(sill::ProcessUsage::new(Duration::ZERO, measured)),
        })),
        WORKSPACE_TEST_LABEL,
    );

    assert!(matches!(
        completion.result,
        Err(VerifyError::TestComputeBudget { measured: actual, budget })
            if actual == measured && budget == WORKSPACE_TEST_COMPUTE_BUDGET
    ));
    let output = String::from_utf8(completion.output).expect("compute refusal is UTF-8");
    assert!(output.contains(&format!(
        "compute disposition=refused user_cpu_ms=0 system_cpu_ms={} total_cpu_ms={} budget_cpu_ms={}",
        measured.as_millis(),
        measured.as_millis(),
        WORKSPACE_TEST_COMPUTE_BUDGET.as_millis(),
    )));
}

#[test]
fn canonical_workspace_test_compute_refuses_absent_kernel_accounting() {
    let completion = execute_workspace_tests(
        Ok(CommandRun::Exited(ProcessOutput {
            status: Status {
                success: true,
                code: Some(0),
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            usage: None,
        })),
        WORKSPACE_TEST_LABEL,
    );

    assert!(matches!(
        completion.result,
        Err(VerifyError::Cleanup { ref label, ref detail })
            if label == WORKSPACE_TEST_LABEL
                && detail.contains("no kernel-accounted resource usage")
    ));
}
