//! The fact vocabulary is closed, and every kind in it is spelled exactly once.
//!
//! [`PINNED`] states each kind's agreed wire spelling independently of the
//! crate under test. The compiled vocabulary must cross as exactly those
//! spellings, name no kind the table lacks, and decode every pinned spelling
//! back to its own kind, so two kinds can never share one word.

use std::collections::BTreeMap;

use wire::FactKind;

/// Every kind's wire spelling, written out here rather than read from the crate
/// under test.
///
/// A spelling drawn from `wire/src/fact.rs` would move with every rename of
/// that file; this answers "what were these spellings agreed to be", and only a
/// deliberate edit here can change that answer.
///
/// Silence is the expensive part. These spellings cross the socket, rest in the
/// journal, and compile into the browser app, so a rename nobody is forced to
/// notice does not fail at a version boundary — it stops matching journal rows
/// already written under the old word, which is a decoder error against durable
/// history rather than a build error against a diff.
///
/// So editing this table is the deliberate act, and the tests below exist to
/// make it impossible to take by accident.
const PINNED: &[(&str, &str)] = &[
    ("TaskAdded", "task::added"),
    ("TaskPhased", "task::phased"),
    ("TaskDone", "task::done"),
    ("TaskReviewed", "task::reviewed"),
    ("TaskAssigned", "task::assigned"),
    ("TaskNoted", "task::noted"),
    ("TaskDeferred", "task::deferred"),
    ("TaskWoken", "task::woken"),
    ("TaskFiled", "task::filed"),
    ("TaskEdited", "task::edited"),
    ("TaskDeleted", "task::deleted"),
    ("TaskClosed", "task::closed"),
    ("TaskConcluded", "task::concluded"),
    ("OopsGenerationAdvanced", "oops::generation"),
    ("OutputControlled", "task::controlled"),
    ("ClaimWon", "claim::won"),
    ("ClaimCarried", "claim::carried"),
    ("ClaimRebound", "claim::rebound"),
    ("ClaimAcknowledged", "claim::acknowledged"),
    ("ClaimFreed", "claim::freed"),
    ("ClaimLapsed", "claim::lapsed"),
    ("ClaimRenewed", "claim::renewed"),
    ("ClaimLeaseWarned", "claim::warned"),
    ("RoomPublished", "room::published"),
    ("RoomMessaged", "room::messaged"),
    ("RoomLeft", "room::left"),
    ("RoomContributed", "room::contributed"),
    ("RoomDelivered", "room::delivered"),
    ("RoomHorizon", "room::horizon"),
    ("DesignNoticed", "room::noticed"),
    ("DesignReturnDelivered", "room::relayed"),
    ("RoomRelevanceObserved", "room::observed"),
    ("RoomRelevanceAssessed", "room::assessed"),
    ("RoomConversed", "room::conversed"),
    ("RoomPosted", "room::posted"),
    ("RoomDisposed", "room::disposed"),
    ("RoomReceiptLapsed", "room::lapsed"),
    ("RoomReceiptRecovered", "room::recovered"),
    ("WorkingGroupMoved", "group::moved"),
    ("WorkingGroupsArranged", "group::arranged"),
    ("DesignProposed", "proposal::retained"),
    ("CandidateCritiqued", "proposal::critiqued"),
    ("CandidateCovered", "proposal::covered"),
    ("PreparationDecided", "proposal::decided"),
    ("CandidateWithdrawn", "proposal::withdrawn"),
    ("ExecutionAuthorized", "proposal::authorized"),
    ("DesignNudged", "proposal::nudged"),
    ("DocApplied", "doc::applied"),
    ("GlobalSettingsChanged", "settings::changed"),
    ("SteerSent", "steer::sent"),
    ("SteerEscalated", "steer::escalated"),
    ("SteerRecovered", "steer::recovered"),
    ("SteerAcked", "steer::acked"),
    ("SteerDeclined", "steer::declined"),
    ("SteerReplied", "steer::replied"),
    ("SteerMessaged", "steer::messaged"),
    ("SteerQueued", "steer::queued"),
    ("SteerWithheld", "steer::withheld"),
    ("SteerMissed", "steer::missed"),
    ("SteerLapsed", "steer::lapsed"),
    ("SteerDeadLettered", "steer::deadlettered"),
    ("SteerCanceled", "steer::canceled"),
    ("ConsentAsked", "consent::asked"),
    ("ConsentGranted", "consent::granted"),
    ("ConsentDenied", "consent::denied"),
    ("ConsentQueued", "consent::queued"),
    ("ConsentLapsed", "consent::lapsed"),
    ("ConsentFloored", "consent::floored"),
    ("LaneSeated", "lane::seated"),
    ("LaneArranged", "lane::arranged"),
    ("LaneRetuned", "lane::retuned"),
    ("LaneEffortRetuned", "lane::effort"),
    ("LaneFastRetuned", "lane::fast"),
    ("LaneRuntimeRetuned", "lane::runtime"),
    ("TeamConfigured", "team::configured"),
    ("TeamArranged", "team::arranged"),
    ("LaneSessionDeclared", "lane::declared"),
    ("LaneStarted", "lane::started"),
    ("LaneImported", "lane::imported"),
    ("LaneRemoved", "lane::removed"),
    ("LaneExited", "lane::exited"),
    ("LaneTimedOut", "lane::timedout"),
    ("LaneCancelled", "lane::cancelled"),
    ("LaneSignalled", "lane::signalled"),
    ("LaneCompacted", "lane::compacted"),
    ("LaneRenewed", "lane::renewed"),
    ("LaneRenewalChanged", "lane::renewal"),
    ("LaneDrained", "lane::drained"),
    ("LaneKilled", "lane::killed"),
    ("LaneDegraded", "lane::degraded"),
    ("LaneHeld", "lane::held"),
    ("LaneResumed", "lane::resumed"),
    ("TasteConfirmed", "taste::confirmed"),
    ("TasteCleared", "taste::cleared"),
    ("TasteJudged", "taste::judged"),
    ("TasteReminded", "taste::reminded"),
    ("NarrationProdded", "narration::prodded"),
    ("NarrationCleared", "narration::cleared"),
    ("CanaryAlarmed", "canary::alarmed"),
    ("CanaryContradicted", "canary::contradicted"),
    ("VendorArmed", "vendor::armed"),
    ("TrustPended", "trust::pended"),
    ("TrustGranted", "trust::granted"),
    ("TrustRevoked", "trust::revoked"),
    ("PrincipalAdmitted", "principal::admitted"),
    ("PrincipalProfiled", "principal::profiled"),
    ("PrincipalGranted", "principal::granted"),
    ("PrincipalRevoked", "principal::revoked"),
    ("PrincipalDeauthorized", "principal::deauthorized"),
    ("CommitLanded", "commit::landed"),
    ("GateFound", "gate::found"),
    ("GateCleared", "gate::cleared"),
    ("JournalRetained", "journal::retained"),
    ("JournalMigrated", "journal::migrated"),
    ("JournalBacked", "journal::backed"),
    ("JournalRestored", "journal::restored"),
    ("JournalIngested", "journal::ingested"),
    ("UpSeated", "up::seated"),
    ("UpQuiescing", "up::quiescing"),
    ("UpDraining", "up::draining"),
    ("UpTerminating", "up::terminating"),
    ("UpKilling", "up::killing"),
    ("UpVerifying", "up::verifying"),
    ("UpDark", "up::dark"),
    ("ProofSealed", "proof::sealed"),
    ("ReleasePublished", "release::published"),
    ("RackAdmitted", "rack::admitted"),
];

/// What the compiled vocabulary actually crosses as, keyed by variant.
///
/// The key comes from `Debug` and the value from serde, so the two halves of a
/// row are read through different derives and a rename cannot move both.
fn crossing_spellings() -> BTreeMap<String, String> {
    FactKind::ALL
        .iter()
        .map(|kind| {
            let encoded = wire::encode(kind).expect("a fact kind encodes");
            let quoted = String::from_utf8(encoded).expect("a spelling is text");
            let spelling = quoted
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("a fact kind crosses as a JSON string")
                .to_owned();
            (format!("{kind:?}"), spelling)
        })
        .collect()
}

#[test]
fn every_kind_crosses_as_the_spelling_this_table_pins() {
    let pinned: BTreeMap<&str, &str> = PINNED.iter().copied().collect();
    assert_eq!(
        pinned.len(),
        PINNED.len(),
        "this table names some variant twice, so one of its rows is unreachable",
    );

    let crossing = crossing_spellings();
    let mut drift = Vec::new();
    for (variant, crossed) in &crossing {
        match pinned.get(variant.as_str()) {
            Some(expected) if *expected == crossed.as_str() => {}
            Some(expected) => drift.push(format!(
                "{variant} was pinned as {expected:?} but now crosses as {crossed:?}"
            )),
            None => drift.push(format!(
                "{variant} crosses as {crossed:?} but this table pins no spelling for it"
            )),
        }
    }
    for variant in pinned.keys() {
        assert!(
            crossing.contains_key(*variant),
            "{variant} is pinned here but is no longer a fact kind",
        );
    }

    assert!(
        drift.is_empty(),
        "the crossing spellings no longer match the ones pinned in this file; \
         change the table only if stored facts should be read under the new \
         word:\n{}",
        drift.join("\n"),
    );
}

#[test]
fn every_pinned_spelling_still_decodes_to_its_kind() {
    // The direction durable history depends on. A journal row written under a
    // pinned spelling has to keep decoding as the kind it was written as, and a
    // rename breaks that here rather than at a read of real bytes.
    for (variant, spelling) in PINNED {
        let quoted = format!("\"{spelling}\"");
        let decoded: FactKind = wire::decode(quoted.as_bytes()).unwrap_or_else(|_| {
            panic!("{spelling:?} was pinned for {variant} but no longer decodes as any kind")
        });

        assert_eq!(
            format!("{decoded:?}"),
            *variant,
            "{spelling:?} was pinned for {variant} but now decodes as {decoded:?}",
        );
    }
}
