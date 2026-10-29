//! A session's best Design that falls short of its floor is recorded once per
//! statement revision as a shadow that delivers nothing, and only
//! `room awareness --shadows` pages it, with each lane's later outcome.

use super::*;

/// A completion sharing one term with [`PITCH`] and one with the longer
/// [`FAR_DESCRIPTION`], diluted so neither Design clears its floor.
const WEAK_TITLE: &str = "Keep the warm cache nightly";
const WEAK_DESCRIPTION: &str = "Warm every seat cache at startup, evict idle entries, sample \
    memory pressure, report eviction counts, and keep the resident cache bounded.";
const FAR_DESCRIPTION: &str = "Rotate the signing keys nightly, archive each expired key under \
    the vault, verify archived signatures, and alert when verification fails.";

#[test]
fn the_best_design_that_falls_short_is_shadowed_once_per_statement() {
    let lanes = Lanes::new("design-shadow");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let near = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let far = lanes.design("dVrD245Km", "Rotate journal keys", FAR_DESCRIPTION);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    let first = lanes.seed_task(
        "dVrD245Kc",
        WEAK_TITLE,
        Some(WEAK_DESCRIPTION),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&first, "Kept it warm.");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a sub-floor Design is not requested"
    );
    let shadows = lanes.shadowed();
    let [shadow] = shadows.as_slice() else {
        panic!("exactly one shadow records the best sub-floor Design: {shadows:?}");
    };
    assert_eq!(
        shadow.design,
        stamp(&near),
        "the shadow is the best shortfall"
    );
    assert_ne!(shadow.design, stamp(&far));
    assert!(
        shadow.score.get() < shadow.floor.get(),
        "{} fell short of {}",
        shadow.score.get(),
        shadow.floor.get()
    );
    assert!(shadow.score.get() > 0.0, "a shadow shares a term");
    assert_eq!(shadow.lane, test_uuid(FIRST));
    assert!(lanes.noticed().is_empty(), "a shadow delivers no request");

    let second = lanes.seed_task(
        "dVrD245Kp",
        WEAK_TITLE,
        Some(WEAK_DESCRIPTION),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&second, "Kept it warm again.");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert_eq!(
        lanes.shadowed().len(),
        1,
        "a second opportunity on the same statement records nothing"
    );

    lanes.retitle(&near, "Page room catch-up soon");
    let third = lanes.seed_task(
        "dVrD245Kg",
        WEAK_TITLE,
        Some(WEAK_DESCRIPTION),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&third, "Kept it warm once more.");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    let shadows = lanes.shadowed();
    assert_eq!(
        shadows.len(),
        2,
        "a new statement revision earns one more shadow: {shadows:?}"
    );
    assert_ne!(shadows[0].statement, shadows[1].statement);
    seated.release_all();
    lanes.finish();
}

#[test]
fn awareness_pages_shadows_only_on_request_with_each_lanes_later_outcome() {
    let lanes = Lanes::new("design-shadow-awareness");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let near = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let weak = lanes.seed_task(
        "dVrD245Kc",
        WEAK_TITLE,
        Some(WEAK_DESCRIPTION),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&weak, "Kept it warm.");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(lanes.request(&config, FIRST).is_none());
    assert_eq!(lanes.shadowed().len(), 1);

    let rows = |words: &[&str]| {
        let page = lanes.room_read(
            &["awareness", near.as_str()]
                .into_iter()
                .chain(words.iter().copied())
                .collect::<Vec<_>>(),
        );
        page["page"]["rows"].as_array().expect("rows").clone()
    };
    assert!(
        rows(&[]).is_empty(),
        "default awareness never shows a shadow"
    );
    let publications = {
        let task = stamp(&near);
        lanes
            .bus
            .read(move |read| {
                read.room_publications(&task, None, journal::FactReadBudget::RETAINED)
            })
            .expect("publications read")
            .rows
    };
    assert!(
        publications.is_empty(),
        "room history never shows a shadow: {publications:?}"
    );
    let shadows = rows(&["--shadows"]);
    let [shadow] = shadows.as_slice() else {
        panic!("one shadow pages: {shadows:?}");
    };
    assert_eq!(shadow["lane"], serde_json::json!(test_uuid(FIRST)));
    assert_eq!(shadow["reach"], "wide");
    assert_eq!(shadow["outcome"], serde_json::json!({ "kind": "none" }));
    let similarity = |field: &str| {
        shadow[field]
            .as_str()
            .and_then(|spelling| spelling.parse::<f64>().ok())
            .unwrap_or_else(|| panic!("{field} is a decimal: {shadow}"))
    };
    let floor = similarity("floor");
    let score = similarity("score");
    assert!(score < floor, "{score} fell short of {floor}");
    assert!(
        shadow["generation"]
            .as_str()
            .is_some_and(|hex| !hex.is_empty())
    );

    lanes.contribute(50, FIRST, &near);
    let shadows = rows(&["--shadows"]);
    assert_eq!(shadows.len(), 1, "a contribution adds no shadow");
    assert_eq!(shadows[0]["outcome"]["kind"], "contributed");
    assert!(
        shadows[0]["outcome"]["at"].as_u64() > shadows[0]["seq"].as_u64(),
        "the outcome follows the shadow"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn a_session_owed_a_request_records_no_shadow() {
    let lanes = Lanes::new("design-shadow-owed");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let owed = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let far = lanes.design("dVrD245Km", "Rotate journal keys", FAR_DESCRIPTION);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    let matching = lanes.claimed("dVrD245Kc", FIRST);
    lanes.complete(&matching, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    let request = lanes
        .request(&config, FIRST)
        .expect("the matching completion owes its Design");
    assert_eq!(request.design, owed);

    let weak = lanes.seed_task(
        "dVrD245Kp",
        WEAK_TITLE,
        Some(WEAK_DESCRIPTION),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&weak, "Kept it warm.");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.shadowed().is_empty(),
        "a session with an owed request records no shadow for {far}"
    );
    assert_eq!(lanes.noticed().len(), 1);
    seated.release_all();
    lanes.finish();
}
