use super::*;

#[test]
fn parser_ignores_blank_lines_and_preserves_each_update() {
    let updates = parse_updates("old new refs/heads/main\n\nzero tip refs/tags/v1\n").unwrap();
    assert_eq!(
        updates,
        [
            RefUpdate {
                old: "old",
                new: "new",
                reference: "refs/heads/main",
            },
            RefUpdate {
                old: "zero",
                new: "tip",
                reference: "refs/tags/v1",
            },
        ]
    );
}

#[test]
fn the_declared_old_value_is_retained_only_for_transaction_correlation() {
    let real = format!("{:040} new refs/heads/main\n", 1);
    let zero = format!("{:040} new refs/heads/main\n", 0);
    let real = parse_updates(&real).unwrap();
    let zero = parse_updates(&zero).unwrap();
    assert_ne!(real, zero);
    assert_ne!(
        transaction_id(TransactionState::Prepared, &real).unwrap(),
        transaction_id(TransactionState::Prepared, &zero).unwrap()
    );
    assert_eq!(real[0].new, zero[0].new);
    assert_eq!(real[0].reference, zero[0].reference);
}

#[test]
fn parser_names_the_first_malformed_line() {
    let error = parse_updates("old new refs/heads/main\nmissing field\n").unwrap_err();
    assert!(matches!(
        error,
        ReferenceTransactionError::MalformedInput { line: 2 }
    ));
}

#[test]
fn only_local_branch_updates_require_repository_observation() {
    let cleanup = parse_updates(
        "0000000000000000000000000000000000000000 \
         0000000000000000000000000000000000000000 AUTO_MERGE\n",
    )
    .unwrap();
    assert!(!requires_repository(&cleanup));

    let branch = parse_updates(
        "0000000000000000000000000000000000000000 \
         aaaabbbb refs/heads/main\n",
    )
    .unwrap();
    assert!(requires_repository(&branch));
}

fn assert_operational_error_names_tree_and_recovery(error: ReferenceTransactionError) {
    let message = error.to_string();
    assert!(message.contains("HEAD is not moved"));
    assert!(message.contains("index and worktree may already have changed"));
    assert!(message.contains("git reset --hard HEAD"));
}

#[test]
fn malformed_input_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::MalformedInput {
        line: 2,
    });
}

#[test]
fn git_launch_failure_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::GitLaunch {
        repository: PathBuf::from("/fixture/repository"),
        arguments: vec!["status".to_owned()],
        source: io::Error::new(io::ErrorKind::NotFound, "fixture git is absent"),
    });
}

#[test]
fn non_utf8_output_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::NonUtf8Output {
        arguments: vec!["symbolic-ref".to_owned(), "HEAD".to_owned()],
    });
}

fn refusal(damage: WorktreeDamage) -> String {
    ReferenceTransactionError::ProtectedHistory {
        reference: "refs/heads/main".to_owned(),
        commits: "deadbee".to_owned(),
        baseline: "refs/remotes/origin/main".to_owned(),
        baseline_commit: "0ddba11".to_owned(),
        damage,
    }
    .to_string()
}

#[test]
fn a_refusal_names_its_baseline_commit_beside_the_reference_it_resolved() {
    let message = refusal(WorktreeDamage::Untouched);
    assert!(message.contains("resolved to refs/remotes/origin/main at 0ddba11"));
}

#[test]
fn a_refusal_lists_its_abandoned_commits_beside_the_branch_they_were_on() {
    let message = refusal(WorktreeDamage::Untouched);
    assert!(message.contains("commits on current branch refs/heads/main: deadbee."));
}

#[test]
fn a_rewritten_worktree_is_named_without_destructive_recovery() {
    let message = refusal(WorktreeDamage::Rewritten);
    assert!(message.contains("HEAD was not moved"));
    assert!(message.contains("index and worktree match"));
    assert!(!message.contains("git reset --hard HEAD"));
    assert!(message.contains("Inspect `git status`"));
}

#[test]
fn an_untouched_worktree_is_never_reported_as_rewritten() {
    let message = refusal(WorktreeDamage::Untouched);
    assert!(!message.contains("index and worktree"));
    assert!(!message.contains("git reset --hard HEAD"));
    assert!(message.contains("did not write the working tree"));
}

#[test]
fn preparing_is_inert_before_input_or_repository_access() {
    apply(
        Path::new("repository-does-not-exist"),
        OsStr::new("preparing"),
        "malformed",
    )
    .unwrap();
}
