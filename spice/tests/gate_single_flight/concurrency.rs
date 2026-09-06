use std::fs;

use super::fixture::{Fixture, execution_starts, streams};

#[test]
fn identical_actions_from_different_commits_execute_once() {
    let fixture = Fixture::new();
    let older = fixture.prove(&fixture.older);
    fixture.wait_for_execution_starts(1);
    let newer = fixture.prove(&fixture.newer);
    fixture.release();
    let older = older.wait_with_output().expect("older proof completes");
    let newer = newer.wait_with_output().expect("newer proof completes");
    assert!(
        older.status.success(),
        "older proof failed: {}",
        streams(&older)
    );
    assert!(
        newer.status.success(),
        "newer proof failed: {}",
        streams(&newer)
    );
    let older_body: serde_json::Value = serde_json::from_slice(&older.stdout).unwrap();
    let newer_body: serde_json::Value = serde_json::from_slice(&newer.stdout).unwrap();
    assert_ne!(older_body["body"]["source"], newer_body["body"]["source"]);
    assert_eq!(
        older_body["body"]["actions"][0]["action_key"],
        newer_body["body"]["actions"][0]["action_key"]
    );
    assert_eq!(
        older_body["body"]["actions"][0]["receipt"],
        newer_body["body"]["actions"][0]["receipt"]
    );
    let mut dispositions = [
        older_body["body"]["actions"][0]["disposition"]
            .as_str()
            .unwrap(),
        newer_body["body"]["actions"][0]["disposition"]
            .as_str()
            .unwrap(),
    ];
    dispositions.sort_unstable();
    assert_eq!(dispositions, ["present", "published"]);
    assert_eq!(execution_starts(&fixture.executions), 1);

    let replay = fixture
        .prove(&fixture.older)
        .wait_with_output()
        .expect("replayed proof completes");
    assert!(
        replay.status.success(),
        "replay failed: {}",
        streams(&replay)
    );
    let replay_body: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay_body["body"]["actions"][0]["disposition"], "present");
    assert_eq!(execution_starts(&fixture.executions), 1);
}

#[test]
fn distinct_action_keys_execute_in_parallel() {
    let fixture = Fixture::new();
    fixture.stage_value(&fixture.older, 3);
    fixture.stage_value(&fixture.newer, 4);
    let older = fixture.prove(&fixture.older);
    fixture.wait_for_execution_starts(1);
    let newer = fixture.prove(&fixture.newer);
    fixture.wait_for_execution_starts(2);
    fixture.release();
    let older = older.wait_with_output().expect("older proof completes");
    let newer = newer.wait_with_output().expect("newer proof completes");
    assert!(
        older.status.success(),
        "older proof failed: {}",
        streams(&older)
    );
    assert!(
        newer.status.success(),
        "newer proof failed: {}",
        streams(&newer)
    );
    let lines = fs::read_to_string(&fixture.executions).expect("read formatter executions");
    assert_eq!(
        lines.lines().take(2).collect::<Vec<_>>(),
        ["start", "start"]
    );
    assert_eq!(execution_starts(&fixture.executions), 2);
}
