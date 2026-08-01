use wire::body::{ConsentAsked, ConsentQueueCause};

use crate::{
    Actuator, Answer, Authority, Budgets, Outcome, POSTURE, Pending, Policy, PolicyError,
    Reversibility, Rule, Spend,
};

#[test]
fn the_first_matching_rule_answers_and_later_matches_do_not() {
    let policy = table(vec![
        rule("reads", "read", Answer::Allow, None),
        rule("pushes", "git push*", Answer::Queue, None),
        // A rule that would also match `git push --force`, placed after the
        // one that answers, so a first-match failure shows up as this name.
        rule("everything-git", "git *", Answer::Allow, None),
    ]);

    assert_eq!(
        policy.decide("git push --force").authority(),
        &Authority::Rule("pushes".to_owned()),
    );
    assert_eq!(policy.decide("git push --force").answer(), Answer::Queue);
    assert_eq!(
        policy.decide("git status").authority(),
        &Authority::Rule("everything-git".to_owned()),
    );
}

#[test]
fn an_unmatched_operation_is_answered_by_the_posture_under_its_own_name() {
    let policy = Policy::new(
        Answer::Queue,
        vec![rule("reads", "read", Answer::Allow, None)],
    )
    .expect("valid table");
    let decision = policy.decide("filesystem.write");

    assert_eq!(decision.authority(), &Authority::Posture);
    assert_eq!(decision.authority().name(), POSTURE);
    assert_eq!(decision.answer(), Answer::Queue);
}

#[test]
fn a_rule_cannot_take_the_name_the_posture_answers_to() {
    assert_eq!(
        Rule::new(POSTURE, "read", Answer::Allow, None),
        Err(PolicyError::ReservedName),
    );
}

#[test]
fn two_rules_cannot_share_the_identity_a_fact_cites() {
    let duplicate = Policy::new(
        Answer::Queue,
        vec![
            rule("reads", "read", Answer::Allow, None),
            rule("reads", "cat", Answer::Deny, None),
        ],
    );

    assert_eq!(
        duplicate,
        Err(PolicyError::DuplicateName {
            rule: "reads".to_owned()
        }),
    );
}

#[test]
fn a_rule_that_never_grants_cannot_carry_a_grant_budget() {
    assert_eq!(
        Rule::new("pushes", "git push*", Answer::Queue, Some(3)),
        Err(PolicyError::UngrantableBudget {
            rule: "pushes".to_owned()
        }),
    );
    assert_eq!(
        Rule::new("blocked", "rm *", Answer::Deny, Some(3)),
        Err(PolicyError::UngrantableBudget {
            rule: "blocked".to_owned()
        }),
    );
}

#[test]
fn every_admission_refusal_names_a_remedy_in_the_one_refusal_shape() {
    let refused = [
        Rule::new("", "read", Answer::Allow, None).expect_err("unnamed rule"),
        Rule::new(POSTURE, "read", Answer::Allow, None).expect_err("reserved name"),
        Rule::new("reads", "", Answer::Allow, None).expect_err("empty pattern"),
        Rule::new("pushes", "git push*", Answer::Deny, Some(2)).expect_err("ungrantable budget"),
        Rule::new("reads", "read", Answer::Allow, Some(0)).expect_err("zero budget"),
        Policy::new(
            Answer::Queue,
            vec![
                rule("reads", "read", Answer::Allow, None),
                rule("reads", "cat", Answer::Allow, None),
            ],
        )
        .expect_err("duplicate name"),
    ];

    for error in refused {
        let refusal = error.refusal();
        assert!(!refusal.reason.is_empty(), "{error:?} states no reason");
        assert!(!refusal.remedy.is_empty(), "{error:?} offers no remedy");
        assert_eq!(
            refusal.witness.as_deref(),
            error.rule(),
            "{error:?} disagrees with its own refusal about the rule at fault",
        );
        assert!(
            error.to_string().contains(&refusal.remedy),
            "{error:?} displays without its remedy",
        );
    }
}

#[test]
fn a_glob_covers_a_prefix_and_an_exact_pattern_covers_only_itself() {
    let policy = table(vec![
        rule("pushes", "git push*", Answer::Allow, None),
        rule("reads", "read", Answer::Allow, None),
    ]);

    assert!(policy.rules()[0].matches("git push"));
    assert!(policy.rules()[0].matches("git push --force"));
    assert!(!policy.rules()[0].matches("git pull"));
    assert!(policy.rules()[1].matches("read"));
    assert!(!policy.rules()[1].matches("readme"));
    assert!(!policy.rules()[1].matches("re"));
}

#[test]
fn a_glob_backtracks_rather_than_stopping_at_its_first_candidate() {
    // `*` must be able to give back what it took: the first `.txt` it can
    // reach is not the one that lets the rest of the pattern match.
    let pattern = rule("archives", "tar *.txt.gz", Answer::Allow, None);

    assert!(pattern.matches("tar notes.txt.gz"));
    assert!(pattern.matches("tar a.txt.gz.txt.gz"));
    assert!(!pattern.matches("tar notes.txt.bz2"));
}

#[test]
fn a_bounded_rule_grants_to_its_bound_and_then_queues_for_renewal() {
    let mut actuator = Actuator::new(table(vec![rule("reads", "read", Answer::Allow, Some(2))]));

    let first = actuator.decide(&asked("read"), Reversibility::Reversible);
    let second = actuator.decide(&asked("read"), Reversibility::Reversible);
    let third_request = asked("read");
    let third = actuator.decide(&third_request, Reversibility::Reversible);

    assert_eq!(granted_budget(&first), Some(Some(1)));
    assert_eq!(granted_budget(&second), Some(Some(0)));
    let Outcome::Queued(queued) = &third else {
        panic!("a spent budget queues for renewal, but the third request answered {third:?}");
    };
    assert_eq!(queued.rule, "reads");
    assert_eq!(
        queued.cause,
        ConsentQueueCause::BudgetExhausted { bound: 2 }
    );
    let held = actuator
        .pending()
        .get(third_request.request)
        .expect("the exhausted request remains held");
    assert_eq!(held.rule(), "reads");
    assert_eq!(
        held.cause(),
        ConsentQueueCause::BudgetExhausted { bound: 2 }
    );
    assert_eq!(actuator.budgets().remaining("reads", 2), 0);
}

#[test]
fn an_exhausted_budget_is_not_pushed_further_past_its_bound_by_asking_again() {
    let mut budgets = Budgets::new();

    assert_eq!(budgets.spend("reads", 1), Spend::Granted { remaining: 0 });
    assert_eq!(budgets.spend("reads", 1), Spend::Exhausted);
    assert_eq!(budgets.spend("reads", 1), Spend::Exhausted);
    // Had the refused spends been recorded, raising the bound by two would
    // show no grants left rather than the two the operator just added.
    assert_eq!(budgets.remaining("reads", 3), 2);
}

#[test]
fn renewal_restores_the_bound_rather_than_extending_it() {
    let mut actuator = Actuator::new(table(vec![rule("reads", "read", Answer::Allow, Some(1))]));

    assert!(matches!(
        actuator.decide(&asked("read"), Reversibility::Reversible),
        Outcome::Granted(_)
    ));
    let exhausted = asked("read");
    assert!(matches!(
        actuator.decide(&exhausted, Reversibility::Reversible),
        Outcome::Queued(_)
    ));

    let renewed = actuator
        .renew(exhausted.request)
        .expect("the exhausted request names the budget to renew");
    assert_eq!(renewed.rule(), "reads");
    assert_eq!(
        renewed.cause(),
        ConsentQueueCause::BudgetExhausted { bound: 1 }
    );
    assert!(!actuator.pending().holds(exhausted.request));
    let after = actuator.decide(&asked("read"), Reversibility::Reversible);

    assert_eq!(
        granted_budget(&after),
        Some(Some(0)),
        "renewal restores exactly the bound, so one grant leaves none",
    );
}

#[test]
fn a_budget_meters_per_rule_name_rather_than_across_the_table() {
    let mut budgets = Budgets::new();

    assert_eq!(budgets.spend("reads", 1), Spend::Granted { remaining: 0 });

    assert_eq!(budgets.spend("writes", 1), Spend::Granted { remaining: 0 });
    assert_eq!(budgets.remaining("reads", 1), 0);
    assert_eq!(budgets.remaining("renamed", 1), 1);
}

#[test]
fn a_held_request_lets_reversible_work_continue_while_the_irreversible_waits() {
    let mut actuator = Actuator::new(table(vec![rule(
        "pushes",
        "git push*",
        Answer::Queue,
        None,
    )]));

    assert!(actuator.proceeds(Reversibility::Irreversible));

    let request = asked("git push --force");
    let outcome = actuator.decide(&request, Reversibility::Irreversible);

    let Outcome::Queued(queued) = outcome else {
        panic!("the queue rule must pend its request");
    };
    assert_eq!(queued.rule, "pushes");
    assert_eq!(queued.cause, ConsentQueueCause::Policy {});
    assert!(actuator.pending().holds(request.request));
    assert!(
        actuator.proceeds(Reversibility::Reversible),
        "queueing must not stop reversible work; that is why it is not a denial",
    );
    assert!(
        !actuator.proceeds(Reversibility::Irreversible),
        "an irreversible action has no undo, so it waits for the operator",
    );
    let refused = actuator
        .renew(request.request)
        .expect_err("a policy queue is resolved directly rather than renewed");
    assert_eq!(refused.reason, "consent::renewal-not-required");
    assert_eq!(refused.witness.as_deref(), Some("pushes"));
    assert!(actuator.pending().holds(request.request));

    let released = actuator.release(request.request).expect("held request");

    assert_eq!(released.operation(), "git push --force");
    assert_eq!(released.reversibility(), Reversibility::Irreversible);
    assert!(actuator.proceeds(Reversibility::Irreversible));
}

#[test]
fn a_queue_holds_each_request_under_its_own_identity() {
    let mut pending = Pending::new();
    let first = asked("git push");
    let second = asked("git push --force");

    pending.hold(
        &first,
        Reversibility::Irreversible,
        "pushes".to_owned(),
        ConsentQueueCause::Policy {},
    );
    pending.hold(
        &second,
        Reversibility::Reversible,
        "writes".to_owned(),
        ConsentQueueCause::BudgetExhausted { bound: 4 },
    );

    assert_eq!(pending.len(), 2);
    assert!(pending.holds(first.request));
    assert!(pending.holds(second.request));
    let held = pending.get(second.request).expect("second request is held");
    assert_eq!(held.rule(), "writes");
    assert_eq!(
        held.cause(),
        ConsentQueueCause::BudgetExhausted { bound: 4 }
    );
    assert_eq!(
        pending.release(first.request).map(|held| held.lane()),
        Some(first.lane)
    );
    assert!(!pending.holds(first.request));
    assert!(!pending.is_empty());
}

#[test]
fn a_denial_refuses_loudly_and_a_grant_or_a_queue_does_not_refuse_at_all() {
    let mut actuator = Actuator::new(
        Policy::new(
            Answer::Queue,
            vec![
                rule("blocked", "rm *", Answer::Deny, None),
                rule("reads", "read", Answer::Allow, None),
            ],
        )
        .expect("valid table"),
    );

    let denied = actuator.decide(&asked("rm -rf /"), Reversibility::Irreversible);
    let refusal = denied.refusal().expect("a denial refuses");

    assert_eq!(refusal.witness.as_deref(), Some("blocked"));
    assert!(
        refusal.remedy.contains("blocked"),
        "a refusal names the rule in its remedy, not only in its witness: {refusal:?}",
    );
    assert!(!refusal.reason.is_empty());

    assert_eq!(
        actuator
            .decide(&asked("read"), Reversibility::Reversible)
            .refusal(),
        None,
        "a grant happened, so it cannot pretend to be a refusal",
    );
    let posture = actuator.decide(&asked("anything else"), Reversibility::Reversible);
    assert_eq!(
        posture.refusal(),
        None,
        "a queued request is not answered yet, so it is not a refusal either",
    );
    let Outcome::Queued(queued) = posture else {
        panic!("an unmatched request follows the queue posture");
    };
    assert_eq!(queued.rule, POSTURE);
    assert_eq!(queued.cause, ConsentQueueCause::Policy {});
}

#[test]
fn a_decided_request_keeps_the_identity_and_lane_it_arrived_with() {
    let mut actuator = Actuator::new(table(vec![
        rule("blocked", "rm *", Answer::Deny, None),
        rule("reads", "read", Answer::Allow, None),
    ]));
    let request = asked("read");

    match actuator.decide(&request, Reversibility::Reversible) {
        Outcome::Granted(granted) => {
            assert_eq!(granted.request, request.request);
            assert_eq!(granted.lane, request.lane);
            assert_eq!(granted.rule, "reads");
        }
        other => panic!("an allow rule grants, but this answered {other:?}"),
    }
}

#[test]
fn every_outcome_this_seat_can_reach_is_a_fact_of_the_declared_consent_plane() {
    // The sweep asks fold what the consent plane contains rather than
    // restating it here, so a kind that leaves the plane fails by name.
    let mut actuator = Actuator::new(table(vec![
        rule("reads", "read", Answer::Allow, None),
        rule("blocked", "rm *", Answer::Deny, None),
        rule("pushes", "git push*", Answer::Queue, None),
    ]));

    for operation in ["read", "rm -rf /", "git push", "unmatched"] {
        let kind = actuator
            .decide(&asked(operation), Reversibility::Reversible)
            .kind();
        assert!(
            fold::FactPlane::Consent.contains(kind),
            "deciding {operation:?} produced {kind:?}, which is not a consent fact",
        );
    }
}

#[test]
fn a_seated_table_and_its_queue_read_back_as_the_operator_configured_them() {
    // An operator has to be able to see the table that decides and the
    // requests waiting on them, so every accessor that renders either one is
    // exercised here rather than left as surface nothing calls.
    let mut actuator = Actuator::new(
        Policy::new(
            Answer::Deny,
            vec![
                rule("reads", "read", Answer::Allow, None),
                rule("pushes", "git push*", Answer::Queue, None),
            ],
        )
        .expect("valid table"),
    );

    assert_eq!(actuator.policy().posture(), Answer::Deny);
    assert_eq!(
        actuator
            .policy()
            .rules()
            .iter()
            .map(|rule| (rule.name(), rule.tool()))
            .collect::<Vec<_>>(),
        [("reads", "read"), ("pushes", "git push*")],
        "the table renders in the order it decides",
    );

    let request = asked("git push --force");
    actuator.decide(&request, Reversibility::Irreversible);

    assert_eq!(
        actuator
            .pending()
            .iter()
            .map(|(identity, held)| (identity, held.operation().to_owned()))
            .collect::<Vec<_>>(),
        [(request.request, "git push --force".to_owned())],
        "a held request is listable, which is what pending it to an operator means",
    );
}

fn table(rules: Vec<Rule>) -> Policy {
    Policy::new(Answer::Queue, rules).expect("valid table")
}

fn rule(name: &str, tool: &str, answer: Answer, budget: Option<u32>) -> Rule {
    Rule::new(name, tool, answer, budget).expect("valid rule")
}

/// Mint a request the way `vendor::acp` states one.
///
/// Each call takes a fresh request identity so a fixture that holds two
/// requests really holds two, rather than overwriting one entry twice.
fn asked(operation: &str) -> ConsentAsked {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let ordinal = NEXT.fetch_add(1, Ordering::Relaxed);
    ConsentAsked {
        request: format!("018f7777-7777-7777-8777-{ordinal:012x}")
            .parse()
            .expect("canonical request"),
        lane: "018f8888-8888-7888-8888-888888888888"
            .parse()
            .expect("canonical lane"),
        operation: operation.to_owned(),
    }
}

fn granted_budget(outcome: &Outcome) -> Option<Option<u32>> {
    match outcome {
        Outcome::Granted(granted) => Some(granted.budget),
        _ => None,
    }
}
