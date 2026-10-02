use std::fs;

use super::fixture::{Fixture, execution_starts, proof_action, streams};

#[test]
fn not_applicable_app_web_proof_does_not_claim_tool_movement() {
    let fixture = Fixture::new();
    let output = fixture.prove_action_output(&fixture.older, "app-web-typecheck");
    assert!(
        output.status.success(),
        "not-applicable app proof failed: {}",
        streams(&output)
    );
    let action = proof_action(&output);
    assert_eq!(action["action"], "app-web-typecheck");
    assert_eq!(action["disposition"], "published");
    assert_eq!(execution_starts(&fixture.executions), 0);
}

#[test]
fn changed_tool_bytes_cannot_reuse_a_stale_receipt() {
    let fixture = Fixture::new();
    fixture.release();
    let first = fixture.prove_output(&fixture.older);
    assert!(
        first.status.success(),
        "first proof failed: {}",
        streams(&first)
    );
    let first = proof_action(&first);

    fixture.rewrite_tool("# executable authority changed\n");
    let second = fixture.prove_output(&fixture.older);
    assert!(
        second.status.success(),
        "proof after tool drift failed: {}",
        streams(&second)
    );
    let second = proof_action(&second);

    assert_ne!(first["action_key"], second["action_key"]);
    assert_ne!(first["receipt"], second["receipt"]);
    assert_eq!(second["disposition"], "published");
    assert_eq!(execution_starts(&fixture.executions), 2);
}

#[test]
fn ancestry_sensitive_actions_do_not_cross_committed_baselines() {
    let fixture = Fixture::new();
    let older = fixture.prove_action_output(&fixture.older, "file-shape");
    let newer = fixture.prove_action_output(&fixture.newer, "file-shape");
    assert!(
        older.status.success(),
        "older ancestry proof failed: {}",
        streams(&older)
    );
    assert!(
        newer.status.success(),
        "newer ancestry proof failed: {}",
        streams(&newer)
    );
    let older = proof_action(&older);
    let newer = proof_action(&newer);

    assert_ne!(older["action_key"], newer["action_key"]);
    assert_ne!(older["receipt"], newer["receipt"]);
    assert_eq!(older["disposition"], "published");
    assert_eq!(newer["disposition"], "published");
}

#[test]
fn index_movement_during_execution_publishes_no_receipt() {
    let fixture = Fixture::new();
    fixture.release();
    fs::write(&fixture.move_index, b"move\n").expect("arm index movement");

    let output = fixture.prove_output(&fixture.newer);
    assert!(
        !output.status.success(),
        "moved proof admitted: {}",
        streams(&output)
    );
    assert!(
        streams(&output).contains("gate::prove-source-moved"),
        "moved proof used the wrong refusal: {}",
        streams(&output)
    );
    assert_eq!(fixture.action_index_count(), 0);
}

#[test]
fn tool_movement_during_execution_publishes_no_receipt() {
    let fixture = Fixture::new();
    fixture.release();
    fs::write(&fixture.move_tool, b"move\n").expect("arm tool movement");

    let output = fixture.prove_output(&fixture.older);
    assert!(
        !output.status.success(),
        "tool-moved proof admitted: {}",
        streams(&output)
    );
    assert!(
        streams(&output).contains("gate::prove-tool-moved"),
        "tool-moved proof used the wrong refusal: {}",
        streams(&output)
    );
    assert_eq!(fixture.action_index_count(), 0);
}

#[test]
fn failed_runner_publishes_nothing_and_same_key_retries_cannot_launder_it() {
    let fixture = Fixture::new();
    fixture.release();
    fs::write(&fixture.fail_execution, b"fail\n").expect("arm runner failure");

    let failed = fixture.prove_output(&fixture.older);
    assert!(
        !failed.status.success(),
        "failed action admitted: {}",
        streams(&failed)
    );
    assert_eq!(fixture.action_index_count(), 0);
    fs::remove_file(&fixture.fail_execution).expect("disarm runner failure");

    let same_key = fixture.prove_output(&fixture.older);
    assert!(
        !same_key.status.success(),
        "terminally failed action was retried as clean: {}",
        streams(&same_key)
    );
    assert_eq!(execution_starts(&fixture.executions), 1);
    assert_eq!(fixture.action_index_count(), 0);

    fixture.stage_value(&fixture.older, 3);
    let retried = fixture.prove_output(&fixture.older);
    assert!(
        retried.status.success(),
        "retry after failed action did not recover: {}",
        streams(&retried)
    );
    assert_eq!(proof_action(&retried)["disposition"], "published");
    assert_eq!(execution_starts(&fixture.executions), 2);
    assert_eq!(fixture.action_index_count(), 1);
}
