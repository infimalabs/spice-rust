use super::*;

fn opening(integrated: &str, local_commits: u32) -> GitBoundary {
    GitBoundary {
        head: integrated.to_owned(),
        integrated: integrated.to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some(integrated.to_owned()),
        local_commits,
    }
}

#[test]
fn plan_completion_accepts_only_the_exact_clean_opening_history() {
    let boundary = opening("opening-head", 0);
    assert!(require_plan_boundary(Some(&boundary), "opening-head").is_ok());

    let moved = require_plan_boundary(Some(&boundary), "implementation-head")
        .expect_err("a commit made during plan must refuse completion");
    assert_eq!(moved.reason(), "git::plan-implementation");
    assert!(moved.detail().contains("lane HEAD moved"));

    let loose = opening("opening-head", 2);
    let loose = require_plan_boundary(Some(&loose), "opening-head")
        .expect_err("a dirty opening boundary must fail closed");
    assert_eq!(loose.reason(), "git::plan-implementation");
    assert!(loose.detail().contains("opened with 2 local commit(s)"));

    let absent = require_plan_boundary(None, "opening-head")
        .expect_err("an unmeasured plan opening must fail closed");
    assert_eq!(absent.reason(), "git::plan-implementation");
}
