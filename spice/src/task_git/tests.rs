use super::*;

#[test]
fn plan_publication_requires_a_clean_captured_opening() {
    let clean = GitBoundary {
        head: "opening-head".to_owned(),
        integrated: "opening-head".to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some("opening-head".to_owned()),
        local_commits: 0,
    };
    assert!(require_plan_opening("plan", false, Some(&clean)).is_ok());
    assert!(require_plan_opening("todo", true, None).is_ok());

    let missing = require_plan_opening("plan", false, None)
        .expect_err("a plan without a captured opening must fail closed");
    assert_eq!(missing.reason(), "git::plan-implementation");

    let resolution = require_plan_opening("plan", true, Some(&clean))
        .expect_err("a plan in merge-resolution state must fail closed");
    assert_eq!(resolution.reason(), "git::plan-implementation");

    let mut committed = clean;
    committed.local_commits = 1;
    let local = require_plan_opening("plan", false, Some(&committed))
        .expect_err("a plan opened with implementation history must fail closed");
    assert_eq!(local.reason(), "git::plan-implementation");
}
