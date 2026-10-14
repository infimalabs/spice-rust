use super::*;

#[test]
fn planning_publication_requires_a_clean_captured_opening() {
    let clean = GitBoundary {
        head: "opening-head".to_owned(),
        integrated: "opening-head".to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some("opening-head".to_owned()),
        local_commits: 0,
    };

    for phase in ["plan", "design", "gather"] {
        assert!(require_planning_opening(phase, false, Some(&clean)).is_ok());

        let missing = require_planning_opening(phase, false, None)
            .expect_err("a planning claim without a captured opening must fail closed");
        assert_eq!(missing.reason(), "git::planning-implementation");

        let resolution = require_planning_opening(phase, true, Some(&clean))
            .expect_err("a planning claim in merge-resolution state must fail closed");
        assert_eq!(resolution.reason(), "git::planning-implementation");

        let mut committed = clean.clone();
        committed.local_commits = 1;
        let local = require_planning_opening(phase, false, Some(&committed))
            .expect_err("a planning claim opened with implementation history must fail closed");
        assert_eq!(local.reason(), "git::planning-implementation");
    }
    for phase in ["todo", "review", "verify", "research"] {
        assert!(require_planning_opening(phase, true, None).is_ok());
    }
}
