//! An owed Design request yields only to a strictly better match from the same
//! session's current evidence.

use super::*;

/// Retained conversation prose about Windows runner archives.
const WINDOWS_PROSE: &str = "Windows native runner validates archive compression headers \
                             libraries toolchain manifests receipts deployment replay smoke";
const WINDOWS_TITLE: &str = "Validate Windows native runner archives";
const WINDOWS_DESCRIPTION: &str =
    "Compression headers libraries toolchain manifests receipts deployment replay smoke.";
/// A Design sharing only its title's subject with [`WINDOWS_PROSE`].
const SIGNING_DESCRIPTION: &str = "Signing certificates notarization keychains entitlements.";

/// FIRST's accounted transcript source, the owner of retained conversation.
fn conversation_source(lanes: &Lanes) -> CanonicalUuid {
    lanes.account(FIRST);
    lanes
        .bus
        .read(|read| read.transcript_source_heads(&test_uuid(FIRST)))
        .expect("transcript sources read")
        .into_iter()
        .next()
        .expect("declared lane has a transcript source")
        .into_body()
        .source
}

/// Newer evidence that fits another Design strictly better replaces the owed
/// request, and both delivery surfaces then present only the replacement.
#[test]
fn a_strictly_better_match_replaces_the_owed_request() {
    let lanes = Lanes::new("design-request-better-match");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let mut diagnostics = Vec::new();
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let source = conversation_source(&lanes);
    retain_conversation(&profiles, source, 0, WINDOWS_PROSE);
    let partial = lanes.design("dVrD245Kd", WINDOWS_TITLE, SIGNING_DESCRIPTION);
    let database = lanes.design(
        "dVrD245Km",
        "Repair database migration",
        "Indexes transactions snapshots rollback consistency schema storage locks recovery.",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let owed = lanes
        .request(&config, FIRST)
        .expect("the partial match is owed first");
    assert_eq!(owed.design, partial);
    assert_eq!(lanes.noticed().len(), 1);

    retain_conversation(
        &profiles,
        source,
        1,
        "Repair database migration indexes transactions snapshots rollback consistency schema \
         storage locks recovery",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let replacement = lanes
        .request(&config, FIRST)
        .expect("the strictly better match is owed");
    assert_eq!(replacement.design, database);
    let notices = lanes.noticed();
    assert_eq!(notices.len(), 2, "one replacing notice is recorded");
    assert_eq!(notices[1].design, stamp(&database));

    let notice = pending_catch_up(&lanes, &seated, &mut diagnostics)
        .expect("room catch-up delivers the owed request");
    assert!(
        notice
            .text
            .contains(&spice::rooms::awareness::request_text(&replacement)),
        "catch-up presents the replacement: {}",
        notice.text
    );
    assert!(
        !notice.text.contains(WINDOWS_TITLE),
        "catch-up no longer presents the replaced request: {}",
        notice.text
    );
    seated.release_all();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    lanes.finish();
}

/// A tied twin, a weaker Design, and newer evidence whose best fit is the owed
/// Design itself each leave the owed request in place and record nothing.
#[test]
fn a_match_no_better_than_the_owed_request_records_nothing() {
    let lanes = Lanes::new("design-request-no-better-match");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let source = conversation_source(&lanes);
    retain_conversation(&profiles, source, 0, WINDOWS_PROSE);
    let owed = lanes.design("dVrD245Kd", WINDOWS_TITLE, WINDOWS_DESCRIPTION);
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("the matching Design is owed")
            .design,
        owed
    );

    let _twin = lanes.design("dVrD245Km", WINDOWS_TITLE, WINDOWS_DESCRIPTION);
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let _weaker = lanes.design("dVrD245Kn", WINDOWS_TITLE, SIGNING_DESCRIPTION);
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    retain_conversation(
        &profiles,
        source,
        1,
        "Windows native runner archive compression headers libraries toolchain manifests \
         receipts deployment replay smoke checks",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);

    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("the owed request survives")
            .design,
        owed
    );
    assert_eq!(lanes.noticed().len(), 1, "no replacing notice is recorded");
    seated.release_all();
    lanes.finish();
}
