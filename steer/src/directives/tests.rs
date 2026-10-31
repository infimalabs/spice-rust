use std::error::Error as _;

use super::refusal::disposition_word;
use super::{Ledger, MediaResolutionError, Refusal, Steering};
use crate::ack::keys::Key;
use crate::ack::scan::Standing;
use wire::body::{
    AcknowledgmentMiss, DirectiveAttachment, SteerAcked, SteerCanceled, SteerDeclined,
    SteerEscalated, SteerLapsed, SteerMissed, SteerRecovered, SteerSent, SteeringStage,
    TasteReminded, TranscriptLocus,
};
use wire::steering::SteerDeadLettered;
use wire::{AgentMediaReference, CanonicalUuid, Digest, Disposition, Instant};

const ROOT: &str = "1kJK6N3R";
const SECOND: &str = "1kJK6M0V";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const ELSEWHERE: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const EXPIRES: &str = "2026-08-01T07:07:19.000Z";
const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";
const SECOND_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgpB";

#[test]
fn a_sent_directive_stands_pending_until_something_retires_it() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    let pending = ledger.pending(lane());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key(), &key(ROOT));
    assert_eq!(pending[0].text(), "hold the line");
    assert_eq!(pending[0].control(), Some("drive-drain-queue"));
    assert_eq!(pending[0].stage(), SteeringStage::Published);
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Live));
}

#[test]
fn a_lane_hears_only_its_own_direction() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    assert!(ledger.pending(elsewhere()).is_empty());
}

#[test]
fn replay_refuses_invalid_media_before_it_can_enter_directive_standing() {
    let mut malformed = sent(ROOT);
    malformed.attachments[0].content_type = "text/plain".to_owned();
    malformed.attachments[0].data_url = "data:text/plain;base64,aGVsbG8=".to_owned();
    let mut ledger = Ledger::new();

    assert!(matches!(
        ledger.absorb(Steering::Sent(&malformed)),
        Err(Refusal::InvalidMedia { directive, .. }) if directive == malformed.directive
    ));
    assert!(
        ledger.directives().is_empty(),
        "invalid media cannot partially enter durable standing",
    );
}

#[test]
fn media_resolution_returns_each_attachment_in_journal_order_without_mutation() {
    let mut publication = sent(ROOT);
    publication.attachments.push(DirectiveAttachment {
        name: "second.png".to_owned(),
        content_type: "image/png".to_owned(),
        data_url: SECOND_PNG_DATA_URL.to_owned(),
    });
    let references =
        wire::media::agent_media_references(publication.directive, &publication.attachments)
            .expect("fixture references");
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Sent(&publication))
        .expect("media directive");

    for (reference, attachment) in references.iter().zip(&publication.attachments) {
        let before = ledger.clone();
        let encoded = wire::encode(reference).expect("reference encodes");
        assert_eq!(
            ledger.resolve_media(lane(), &encoded).unwrap(),
            attachment.decoded_bytes().unwrap(),
        );
        assert_eq!(ledger, before, "resolution cannot alter durable standing");
    }
}

#[test]
fn malformed_encoded_media_refuses_before_authority_lookup_without_echoing_input() {
    let publication = sent(ROOT);
    let reference = media_reference(&publication, 0);
    let valid = String::from_utf8(wire::encode(&reference).unwrap()).unwrap();
    let unknown_field = valid.replacen('}', r#","unknown":true}"#, 1).into_bytes();
    let payload_in_wrong_field = valid
        .replacen(
            &format!(r#""bytes":{}"#, reference.bytes()),
            r#""bytes":"iVBORw0KGgo=""#,
            1,
        )
        .into_bytes();
    let oversized = vec![b' '; wire::media::MAX_AGENT_MEDIA_REFERENCE_BYTES + 1];
    let ledger = Ledger::new();

    for encoded in [
        b"{}".to_vec(),
        unknown_field,
        payload_in_wrong_field,
        oversized,
    ] {
        let before = ledger.clone();
        let error = ledger.resolve_media(lane(), &encoded).unwrap_err();
        assert_eq!(error, MediaResolutionError::Malformed);
        assert!(error.source().is_none());
        assert_eq!(
            ledger, before,
            "malformed input cannot inspect or alter authority"
        );
        assert!(!error.to_string().contains("iVBORw0KGgo="));
    }
}

#[test]
fn media_resolution_refuses_unknown_lane_missing_and_mismatched_references_inertly() {
    let publication = sent(ROOT);
    let reference = media_reference(&publication, 0);
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Sent(&publication))
        .expect("media directive");

    let unknown = replace_reference(
        &reference,
        &format!(r#""directive":"{}""#, directive(ROOT)),
        &format!(r#""directive":"{}""#, directive(SECOND)),
    );
    let error = resolve_error(&ledger, lane(), &unknown);
    assert!(matches!(
        error,
        MediaResolutionError::Unknown { directive: found } if found == directive(SECOND)
    ));

    let error = resolve_error(&ledger, elsewhere(), &reference);
    assert!(matches!(
        error,
        MediaResolutionError::WrongLane {
            directive: found,
            expected,
            observed,
        } if found == directive(ROOT) && expected == lane() && observed == elsewhere()
    ));

    let missing = replace_reference(&reference, r#""ordinal":0"#, r#""ordinal":1"#);
    let error = resolve_error(&ledger, lane(), &missing);
    assert!(matches!(
        error,
        MediaResolutionError::Missing { directive: found, ordinal: 1 }
            if found == directive(ROOT)
    ));

    for (field, from, to) in [
        (
            "name",
            r#""name":"operator-capture.png""#,
            r#""name":"renamed.png""#,
        ),
        (
            "content_type",
            r#""content_type":"image/png""#,
            r#""content_type":"image/jpeg""#,
        ),
    ] {
        let mismatched = replace_reference(&reference, from, to);
        let error = resolve_error(&ledger, lane(), &mismatched);
        assert!(matches!(
            error,
            MediaResolutionError::Mismatched {
                directive: found,
                ordinal: 0,
                field: observed,
            } if found == directive(ROOT) && observed == field
        ));
    }
}

#[test]
fn media_resolution_recomputes_digest_and_length_and_refuses_undecodable_authority() {
    let publication = sent(ROOT);
    let reference = media_reference(&publication, 0);
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Sent(&publication))
        .expect("media directive");

    let old_digest = String::from_utf8(wire::encode(reference.digest()).unwrap()).unwrap();
    let new_digest = String::from_utf8(
        wire::encode(&Digest::compute(
            wire::Domain::MediaContent,
            b"different image",
        ))
        .unwrap(),
    )
    .unwrap();
    let stale_digest = replace_reference(
        &reference,
        &format!(r#""digest":{old_digest}"#),
        &format!(r#""digest":{new_digest}"#),
    );
    assert!(matches!(
        resolve_error(&ledger, lane(), &stale_digest),
        MediaResolutionError::Stale { directive: found, ordinal: 0 }
            if found == directive(ROOT)
    ));

    let stale_length = replace_reference(
        &reference,
        &format!(r#""bytes":{}"#, reference.bytes()),
        &format!(r#""bytes":{}"#, reference.bytes() + 1),
    );
    assert!(matches!(
        resolve_error(&ledger, lane(), &stale_length),
        MediaResolutionError::Stale { directive: found, ordinal: 0 }
            if found == directive(ROOT)
    ));

    let authority = ledger
        .directives
        .get_mut(&key(ROOT))
        .expect("fixture directive remains");
    authority.attachments[0].data_url = "data:image/png;base64,%%%".to_owned();
    let error = resolve_error(&ledger, lane(), &reference);
    assert!(matches!(
        error,
        MediaResolutionError::Undecodable {
            directive: found,
            ordinal: 0,
            error: wire::MediaError::Base64,
        } if found == directive(ROOT)
    ));
}

#[test]
fn media_resolution_names_every_retired_disposition_without_exposing_payloads() {
    let publication = sent(ROOT);
    let reference = media_reference(&publication, 0);
    let mut live = Ledger::new();
    live.absorb(Steering::Sent(&publication))
        .expect("media directive");

    for disposition in [
        Disposition::Acked,
        Disposition::Declined,
        Disposition::Canceled,
        Disposition::Lapsed,
        Disposition::DeadLettered,
    ] {
        let mut retired = live.clone();
        retired
            .directives
            .get_mut(&key(ROOT))
            .expect("fixture directive remains")
            .disposition = disposition;
        let error = resolve_error(&retired, lane(), &reference);
        assert!(matches!(
            error,
            MediaResolutionError::Retired {
                directive: found,
                disposition: observed,
            } if found == directive(ROOT) && observed == disposition
        ));
        assert!(error.to_string().contains(disposition_word(disposition)));
    }
}

#[test]
fn a_repeated_key_is_refused() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    assert_eq!(
        ledger.absorb(Steering::Sent(&sent(ROOT))),
        Err(Refusal::Duplicate { key: key(ROOT) })
    );
}

/// A key from the retired base-62 mint is durable, so replay must pass it.
///
/// The envelopes are shaped after a real pair out of the resident's own
/// journal, where the pre-codec fact sits ahead of a current one. Replay
/// propagates a refusal rather than skipping the fact, so a grammar that
/// turns the first key away does not lose one directive, it loses that one
/// and every fact behind it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_retired_alphabet_key_replays_and_does_not_bury_the_facts_behind_it() {
    let snapshot = snapshot_of(&[
        fact_bytes(1, "YTVtQvOX", "sent before the moment codec landed"),
        fact_bytes(2, "1kJK6N3R", "sent after it"),
    ]);

    let ledger = Ledger::from_snapshot(&snapshot).expect("replay admits the retired key");

    let pending = ledger.pending(lane());
    assert_eq!(pending.len(), 2, "a fact behind the retired key was lost");
    assert_eq!(pending[0].key().as_str(), "YTVtQvOX");
    assert_eq!(pending[1].key().as_str(), "1kJK6N3R");
}

/// One `steer::sent` envelope as the journal stores it.
#[cfg(not(target_arch = "wasm32"))]
fn fact_bytes(seq: u64, key: &str, text: &str) -> String {
    format!(
        concat!(
            r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"operator","#,
            r#""origin":"resident:e2e9f1f2-19b2-4ddc-83fa-340b02dee2c4","#,
            r#""fact":{{"kind":"steer::sent","#,
            r#""directive":"a0ae31a7-eb68-463a-b3c0-272bfeaaa5d{}","#,
            r#""expires":"{}","key":"{}","lane":"{}","text":"{}"}}}}"#
        ),
        seq, seq, seq, EXPIRES, key, LANE, text
    )
}

/// One fixture journal, decoded exactly as the resident reads one.
#[cfg(not(target_arch = "wasm32"))]
fn snapshot_of(facts: &[String]) -> journal::FactSnapshot {
    let decoded = facts
        .iter()
        .map(|text| wire::decode(text.as_bytes()).expect("registered envelope"))
        .collect();
    journal::FactSnapshot::from_facts(decoded).expect("fixture snapshot")
}

/// One `taste::reminded` envelope as the journal stores it.
#[cfg(not(target_arch = "wasm32"))]
fn reminded_bytes(seq: u64, key: &str) -> String {
    format!(
        concat!(
            r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"spice","#,
            r#""origin":"resident:e2e9f1f2-19b2-4ddc-83fa-340b02dee2c4","#,
            r#""fact":{{"kind":"taste::reminded","#,
            r#""directive":"a0ae31a7-eb68-463a-b3c0-272bfeaaa5d{}","#,
            r#""key":"{}","lane":"{}","taste":"polling","content":"{}","#,
            r#""occupant":"{}","compaction":2,"#,
            r#""locus":{{"member":1,"offset":40,"length":80,"event":0,"line":3,"column":1}},"#,
            r#""text":"polling is not a completion event","expires":"{}"}}}}"#
        ),
        seq,
        seq,
        seq,
        key,
        LANE,
        prose().as_str(),
        ELSEWHERE,
        EXPIRES
    )
}

/// Escalation changes presentation without multiplying identity or pending rows.
#[test]
fn an_escalation_retains_one_stable_pending_directive() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("reminded");

    let pending = ledger.pending(lane());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key(), &key(ROOT));
    assert_eq!(pending[0].directive(), directive(ROOT));
    assert_eq!(pending[0].stage(), SteeringStage::Reminded);
    assert_eq!(pending[0].text(), "hold the line");
    assert_eq!(ledger.directives().len(), 1);
}

#[test]
fn an_escalation_that_skips_or_repeats_a_stage_is_refused() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    assert_eq!(
        ledger.absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Escalated,
        ))),
        Err(Refusal::Stage {
            key: key(ROOT),
            expected: SteeringStage::Reminded,
            observed: SteeringStage::Escalated,
        })
    );
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("first stage");
    assert_eq!(
        ledger.absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        ))),
        Err(Refusal::Stage {
            key: key(ROOT),
            expected: SteeringStage::Escalated,
            observed: SteeringStage::Reminded,
        })
    );
}

/// Past overdue, presentation stops changing and expiry remains separate.
#[test]
fn an_overdue_directive_refuses_another_escalation() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    for stage in [
        SteeringStage::Reminded,
        SteeringStage::Escalated,
        SteeringStage::Overdue,
    ] {
        ledger
            .absorb(Steering::Escalated(&escalated(ROOT, stage)))
            .expect("ordered stage");
    }

    assert_eq!(
        ledger.absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Overdue,
        ))),
        Err(Refusal::FinalStage { key: key(ROOT) })
    );
}

#[test]
fn an_escalation_addressing_another_lane_is_refused() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    let mut wandered = escalated(ROOT, SteeringStage::Reminded);
    wandered.lane = elsewhere();
    assert_eq!(
        ledger.absorb(Steering::Escalated(&wandered)),
        Err(Refusal::Lane {
            key: key(ROOT),
            expected: lane(),
            observed: elsewhere(),
        })
    );
}

#[test]
fn an_escalation_cannot_precede_its_original_publication() {
    let mut ledger = Ledger::new();
    let published: Instant = "2026-08-01T06:00:15.000Z"
        .parse()
        .expect("publication instant");
    let observed: Instant = "2026-08-01T06:00:14.000Z"
        .parse()
        .expect("transition instant");
    ledger
        .absorb_recorded(Steering::Sent(&sent(ROOT)), Some(published))
        .expect("sent");
    let before = ledger.clone();

    assert_eq!(
        ledger.absorb_recorded(
            Steering::Escalated(&escalated(ROOT, SteeringStage::Reminded)),
            Some(observed),
        ),
        Err(Refusal::Chronology {
            key: key(ROOT),
            previous: published,
            observed,
        })
    );
    assert_eq!(ledger, before);
}

/// Acknowledgment retires the same root after presentation changes.
#[test]
fn acknowledging_an_escalated_directive_retires_the_stable_root() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("reminded");
    ledger
        .absorb(Steering::Acked(&acked(ROOT)))
        .expect("the stable directive is acknowledged");

    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Acked));
    assert_eq!(ledger.directives().len(), 1);
    assert!(ledger.pending(lane()).is_empty());
}

#[test]
fn a_retired_directive_refuses_a_second_disposition() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger.absorb(Steering::Acked(&acked(ROOT))).expect("acked");

    assert_eq!(
        ledger.absorb(Steering::Declined(&declined(ROOT))),
        Err(Refusal::Retired {
            key: key(ROOT),
            disposition: Disposition::Acked,
        })
    );
    assert_eq!(
        ledger.absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        ))),
        Err(Refusal::Retired {
            key: key(ROOT),
            disposition: Disposition::Acked,
        })
    );
}

#[test]
fn a_declined_directive_retires_as_declined() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger
        .absorb(Steering::Declined(&declined(ROOT)))
        .expect("declined");

    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Declined));
}

#[test]
fn an_acknowledgment_binds_the_directive_identity_and_lane() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    let before = ledger.clone();

    let mut wrong_identity = acked(ROOT);
    wrong_identity.directive = directive(SECOND);
    assert!(matches!(
        ledger.absorb(Steering::Acked(&wrong_identity)),
        Err(Refusal::Identity { .. })
    ));
    assert_eq!(ledger, before);

    let mut wrong_lane = acked(ROOT);
    wrong_lane.lane = "018f82d4-6f59-7d06-92e3-bce913116399"
        .parse()
        .expect("other lane");
    assert!(matches!(
        ledger.absorb(Steering::Acked(&wrong_lane)),
        Err(Refusal::Lane { .. })
    ));
    assert_eq!(ledger, before);
}

#[test]
fn an_operator_cancel_retires_a_live_directive() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger
        .absorb(Steering::Canceled(&SteerCanceled {
            key: ROOT.to_owned(),
            lane: lane(),
        }))
        .expect("canceled");

    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Canceled));
    assert!(ledger.pending(lane()).is_empty());
}

#[test]
fn a_lapse_must_name_the_expiry_the_directive_carries() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

    let early: Instant = "2026-08-01T06:00:00.000Z".parse().expect("instant");
    assert_eq!(
        ledger.absorb(Steering::Lapsed(&SteerLapsed {
            key: ROOT.to_owned(),
            lane: lane(),
            expires: early,
        })),
        Err(Refusal::Expiry {
            key: key(ROOT),
            expected: expires(),
            observed: early,
        })
    );

    ledger
        .absorb(Steering::Lapsed(&SteerLapsed {
            key: ROOT.to_owned(),
            lane: lane(),
            expires: expires(),
        }))
        .expect("lapsed at its own expiry");
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Lapsed));
}

#[test]
fn a_dead_letter_is_distinctly_parked_and_recovers_with_the_same_identity() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    ledger
        .absorb(Steering::DeadLettered(&SteerDeadLettered {
            directive: directive(ROOT),
            key: ROOT.to_owned(),
            lane: lane(),
            reason: "lifecycle::wake-launch-failed".to_owned(),
            requeue: format!("spice steer recover {}", directive(ROOT)),
        }))
        .expect("undeliverable directive parks");
    assert_eq!(
        ledger.disposition(&key(ROOT)),
        Some(Disposition::DeadLettered)
    );
    assert!(ledger.pending(lane()).is_empty());

    let recovered_expiry: Instant = "2026-08-01T08:07:19.000Z"
        .parse()
        .expect("recovery instant");
    ledger
        .absorb(Steering::Recovered(&SteerRecovered {
            directive: directive(ROOT),
            key: ROOT.to_owned(),
            lane: lane(),
            lapsed_at: expires(),
            expires: recovered_expiry,
        }))
        .expect("dead letter requeues");
    let recovered = ledger
        .directive(directive(ROOT))
        .expect("same directive identity remains");
    assert_eq!(recovered.disposition(), Disposition::Live);
    assert_eq!(recovered.expires(), recovered_expiry);
}

#[test]
fn recovery_restores_only_the_exact_lapsed_directive_without_losing_payload() {
    let mut ledger = Ledger::new();
    let original = sent(ROOT);
    ledger.absorb(Steering::Sent(&original)).expect("sent");
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("reminded");
    ledger
        .absorb(Steering::Lapsed(&SteerLapsed {
            key: ROOT.to_owned(),
            lane: lane(),
            expires: expires(),
        }))
        .expect("lapsed at its own expiry");
    let recovered_expiry: Instant = "2026-08-01T08:07:19.000Z"
        .parse()
        .expect("recovery instant");
    let recovered = SteerRecovered {
        directive: directive(ROOT),
        key: ROOT.to_owned(),
        lane: lane(),
        lapsed_at: expires(),
        expires: recovered_expiry,
    };

    let mut unbounded = recovered.clone();
    unbounded.expires = expires();
    let before = ledger.clone();
    assert_eq!(
        ledger.absorb(Steering::Recovered(&unbounded)),
        Err(Refusal::RecoveryExpiry {
            key: key(ROOT),
            lapsed_at: expires(),
            expires: expires(),
        })
    );
    assert_eq!(ledger, before);

    ledger
        .absorb(Steering::Recovered(&recovered))
        .expect("exact lapsed directive recovers");

    let restored = ledger
        .directive(directive(ROOT))
        .expect("recovered directive retains identity");
    assert_eq!(restored.text(), original.text);
    assert_eq!(restored.control(), original.control.as_deref());
    assert_eq!(restored.attachments(), original.attachments);
    assert_eq!(restored.expires(), recovered_expiry);
    assert_eq!(restored.stage(), SteeringStage::Reminded);
    assert_eq!(restored.disposition(), Disposition::Live);
    assert_eq!(ledger.pending(lane()), vec![restored]);
    assert_eq!(
        ledger.absorb(Steering::Recovered(&recovered)),
        Err(Refusal::Retired {
            key: key(ROOT),
            disposition: Disposition::Live,
        }),
        "the same failed directive cannot be recovered twice",
    );
    ledger
        .absorb(Steering::Acked(&acked(ROOT)))
        .expect("the recovered directive can retire normally");
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Acked));
}

/// A missed acknowledgment is a diagnostic, never a retirement. Its key is
/// not even read, because the fact exists precisely when the key was
/// unknown or malformed.
#[test]
fn a_missed_acknowledgment_alters_nothing() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    let before = ledger.clone();

    for named in [ROOT, "nothing", ""] {
        ledger
            .absorb(Steering::Missed(&SteerMissed {
                key: named.to_owned(),
                lane: lane(),
                epoch: epoch(),
                locus: transcript_locus(12, 1),
                miss: AcknowledgmentMiss::Unknown,
            }))
            .expect("a missed acknowledgment is always absorbed");
    }

    assert_eq!(ledger, before, "a missed acknowledgment altered standing");
}

#[test]
fn a_key_that_is_not_spelled_as_one_names_the_field() {
    let mut ledger = Ledger::new();
    let mut malformed = sent(ROOT);
    malformed.key = "1kJK6N3".to_owned();

    assert_eq!(
        ledger.absorb(Steering::Sent(&malformed)),
        Err(Refusal::Malformed {
            field: "key",
            text: "1kJK6N3".to_owned(),
        })
    );

    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    let mut wandering = escalated(ROOT, SteeringStage::Reminded);
    wandering.key = "nope".to_owned();
    assert_eq!(
        ledger.absorb(Steering::Escalated(&wandering)),
        Err(Refusal::Malformed {
            field: "key",
            text: "nope".to_owned(),
        })
    );
}

#[test]
fn an_acknowledgment_of_nothing_is_refused() {
    let mut ledger = Ledger::new();

    assert_eq!(
        ledger.absorb(Steering::Acked(&acked(ROOT))),
        Err(Refusal::Unknown { key: key(ROOT) })
    );
}

/// A refusal is not a half-applied fact.
#[test]
fn a_refused_fact_leaves_standing_untouched() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    let before = ledger.clone();

    for refused in [
        Steering::Sent(&sent(ROOT)),
        Steering::Escalated(&escalated(ROOT, SteeringStage::Escalated)),
        Steering::Acked(&acked(SECOND)),
    ] {
        assert!(ledger.absorb(refused).is_err(), "fact was admitted");
        assert_eq!(ledger, before, "a refused fact altered standing");
    }
}

/// A reminder stands as one ordinary live directive.
///
/// It is prose addressed to a lane and nothing else: no steering control to
/// drive, no attachments to open. Asserting the absent halves is the point,
/// because a reminder that quietly carried a control string would steer a
/// lane through a channel no operator opened.
#[test]
fn a_taste_reminder_stands_as_one_live_directive_carrying_only_prose() {
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Reminded(&reminded(ROOT)))
        .expect("reminded");

    let pending = ledger.pending(lane());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key(), &key(ROOT));
    assert_eq!(pending[0].text(), "polling is not a completion event");
    assert_eq!(pending[0].control(), None);
    assert!(pending[0].attachments().is_empty());
    assert_eq!(pending[0].stage(), SteeringStage::Published);
    assert_eq!(pending[0].expires(), expires());
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Live));
}

/// Both publication paths answer to the same two laws, in both orders.
///
/// The refusals are checked across origins rather than each kind against
/// itself, because that is the pair a shared admission exists to stop: a
/// taste firing on prose that quotes a live directive key, and a taste
/// re-firing on prose already reminded under a spent identity.
#[test]
fn a_reminder_and_a_send_cannot_open_the_same_key_or_identity() {
    let mut ledger = Ledger::new();
    ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
    let before = ledger.clone();

    assert_eq!(
        ledger.absorb(Steering::Reminded(&reminded(ROOT))),
        Err(Refusal::Duplicate { key: key(ROOT) })
    );
    assert_eq!(ledger, before, "a refused reminder altered standing");

    let mut respelled = reminded(SECOND);
    respelled.directive = directive(ROOT);
    assert_eq!(
        ledger.absorb(Steering::Reminded(&respelled)),
        Err(Refusal::IdentityDuplicate {
            directive: directive(ROOT),
        })
    );
    assert_eq!(ledger, before, "a refused reminder altered standing");
}

#[test]
fn a_send_cannot_reopen_a_key_or_identity_a_reminder_holds() {
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Reminded(&reminded(ROOT)))
        .expect("reminded");
    let before = ledger.clone();

    assert_eq!(
        ledger.absorb(Steering::Sent(&sent(ROOT))),
        Err(Refusal::Duplicate { key: key(ROOT) })
    );
    assert_eq!(ledger, before, "a refused send altered standing");

    let mut respelled = sent(SECOND);
    respelled.directive = directive(ROOT);
    assert_eq!(
        ledger.absorb(Steering::Sent(&respelled)),
        Err(Refusal::IdentityDuplicate {
            directive: directive(ROOT),
        })
    );
    assert_eq!(ledger, before, "a refused send altered standing");
}

/// A reminder is ordinary steering and retains its key as urgency changes.
#[test]
fn acknowledging_an_escalated_reminder_retires_its_stable_key() {
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Reminded(&reminded(ROOT)))
        .expect("reminded");
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("presentation reminded");
    ledger
        .absorb(Steering::Acked(&acked(ROOT)))
        .expect("acked at the reminder itself");

    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Acked));
    assert_eq!(ledger.directives().len(), 1);
    assert!(ledger.pending(lane()).is_empty());
}

#[test]
fn declining_an_escalated_reminder_retires_its_stable_key() {
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Reminded(&reminded(ROOT)))
        .expect("reminded");
    ledger
        .absorb(Steering::Escalated(&escalated(
            ROOT,
            SteeringStage::Reminded,
        )))
        .expect("presentation reminded");
    ledger
        .absorb(Steering::Declined(&declined(ROOT)))
        .expect("declined");

    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Declined));
    assert_eq!(ledger.directives().len(), 1);
    assert!(ledger.pending(lane()).is_empty());
}

/// The rest of a directive's life is the same life, whatever opened it.
///
/// Lapse, recovery, and cancel key on standing rather than on origin, so this
/// holds the remaining named behaviors by subtraction: acknowledgment and
/// refusal are proved above, and a reminder that lapsed but could not be
/// recovered or withdrawn would be a directive the operator can neither answer
/// nor take back.
#[test]
fn a_reminder_lapses_recovers_and_cancels_exactly_as_a_send_does() {
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Reminded(&reminded(ROOT)))
        .expect("reminded");

    ledger
        .absorb(Steering::Lapsed(&SteerLapsed {
            key: ROOT.to_owned(),
            lane: lane(),
            expires: expires(),
        }))
        .expect("lapsed at the expiry the reminder carries");
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Lapsed));

    let renewed: Instant = "2026-08-01T08:07:19.000Z"
        .parse()
        .expect("recovery instant");
    ledger
        .absorb(Steering::Recovered(&SteerRecovered {
            directive: directive(ROOT),
            key: ROOT.to_owned(),
            lane: lane(),
            lapsed_at: expires(),
            expires: renewed,
        }))
        .expect("the reminder recovers on its own identity");
    let restored = ledger
        .directive(directive(ROOT))
        .expect("a recovered reminder keeps its identity");
    assert_eq!(restored.text(), "polling is not a completion event");
    assert_eq!(restored.expires(), renewed);
    assert_eq!(restored.disposition(), Disposition::Live);

    ledger
        .absorb(Steering::Canceled(&SteerCanceled {
            key: ROOT.to_owned(),
            lane: lane(),
        }))
        .expect("the operator withdraws a reminder like any directive");
    assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Canceled));
    assert!(ledger.pending(lane()).is_empty());
}

/// Replay raises a reminder from stored bytes, not only from a live absorb.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_stored_reminder_replays_into_one_live_directive() {
    let snapshot = snapshot_of(&[reminded_bytes(1, ROOT)]);

    let ledger = Ledger::from_snapshot(&snapshot).expect("a stored reminder replays");

    let pending = ledger.pending(lane());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key(), &key(ROOT));
    assert_eq!(pending[0].text(), "polling is not a completion event");
    assert_eq!(pending[0].control(), None);
}

/// Replay is the same reduction, so standing does not depend on when the
/// facts were read.
#[test]
fn replaying_the_same_facts_produces_the_same_standing() {
    let build = || {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger
            .absorb(Steering::Escalated(&escalated(
                ROOT,
                SteeringStage::Reminded,
            )))
            .expect("reminded");
        ledger.absorb(Steering::Acked(&acked(ROOT))).expect("acked");
        ledger
    };

    assert_eq!(build(), build());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_committed_suffix_advances_to_the_same_ledger_as_restart_replay() {
    let snapshot = snapshot_of(&[
        fact_bytes(1, ROOT, "first publication"),
        fact_bytes(2, SECOND, "second publication"),
    ]);
    let restarted = Ledger::from_snapshot(&snapshot).expect("restart replay");
    let mut incremental = Ledger::from_facts(&snapshot.facts()[..1]).expect("prefix replay");
    incremental
        .advance(&snapshot.facts()[1..])
        .expect("one committed suffix advances");

    assert_eq!(incremental, restarted);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_refused_committed_suffix_rolls_back_every_prior_steering_transition() {
    let snapshot = snapshot_of(&[
        fact_bytes(1, ROOT, "admitted prefix"),
        fact_bytes(2, SECOND, "valid suffix publication"),
        fact_bytes(3, SECOND, "late duplicate suffix publication"),
    ]);
    let mut incremental = Ledger::from_facts(&snapshot.facts()[..1]).expect("prefix replay");
    let before = incremental.clone();

    incremental
        .advance(&snapshot.facts()[1..])
        .expect_err("late duplicate refuses the suffix");

    assert_eq!(incremental, before);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn replayed_authority_heads_reproduce_the_canonical_ledger() {
    let snapshot = snapshot_of(&[
        fact_bytes(1, ROOT, "first publication"),
        fact_bytes(2, SECOND, "second publication"),
    ]);
    let canonical = Ledger::from_snapshot(&snapshot).expect("canonical replay");
    let index = Ledger::authority_index(&snapshot).expect("authority fold");
    let indexed = Ledger::from_authority_heads(index.heads().iter().map(|head| head.body()))
        .expect("indexed heads reduce");

    assert_eq!(indexed, canonical);
    assert_eq!(index.heads().len(), 2);
    assert_eq!(index.heads()[0].position().get(), 1);
    assert_eq!(index.heads()[1].position().get(), 2);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn contradictory_replay_produces_no_partial_authority_index() {
    let snapshot = snapshot_of(&[
        fact_bytes(1, ROOT, "first publication"),
        fact_bytes(2, ROOT, "contradictory duplicate key"),
    ]);
    let before = snapshot.facts().to_vec();

    Ledger::authority_index(&snapshot).expect_err("duplicate key refuses replay");
    assert_eq!(
        snapshot.facts(),
        before.as_slice(),
        "replay cannot mutate audit facts"
    );
}

fn key(text: &str) -> Key {
    text.parse().expect("test key")
}

fn lane() -> CanonicalUuid {
    LANE.parse().expect("test lane")
}

fn elsewhere() -> CanonicalUuid {
    ELSEWHERE.parse().expect("other lane")
}

fn expires() -> Instant {
    EXPIRES.parse().expect("test instant")
}

fn sent(key: &str) -> SteerSent {
    SteerSent {
        from: None,
        room: None,
        directive: directive(key),
        fanout: None,
        key: key.to_owned(),
        lane: lane(),
        text: "hold the line".to_owned(),
        control: Some("drive-drain-queue".to_owned()),
        attachments: vec![DirectiveAttachment {
            name: "operator-capture.png".to_owned(),
            content_type: "image/png".to_owned(),
            data_url: PNG_DATA_URL.to_owned(),
        }],
        expires: expires(),
    }
}

fn media_reference(publication: &SteerSent, ordinal: usize) -> AgentMediaReference {
    AgentMediaReference::from_media(
        publication.directive,
        ordinal,
        &publication.attachments[ordinal],
    )
    .expect("fixture media reference")
}

fn replace_reference(reference: &AgentMediaReference, from: &str, to: &str) -> AgentMediaReference {
    let encoded = String::from_utf8(wire::encode(reference).expect("reference encodes"))
        .expect("wire JSON is UTF-8");
    let replaced = encoded.replacen(from, to, 1);
    assert_ne!(replaced, encoded, "fixture replacement must match {from:?}");
    wire::decode(replaced.as_bytes()).expect("forged reference remains structurally valid")
}

fn resolve_error(
    ledger: &Ledger,
    authenticated_lane: CanonicalUuid,
    reference: &AgentMediaReference,
) -> MediaResolutionError {
    let before = ledger.clone();
    let error = ledger
        .resolve_media(
            authenticated_lane,
            &wire::encode(reference).expect("reference encodes"),
        )
        .expect_err("fixture reference must refuse");
    assert_eq!(ledger, &before, "a refusal cannot alter durable standing");

    let rendered = error.to_string();
    match &error {
        MediaResolutionError::Malformed => {
            assert!(error.source().is_none());
        }
        MediaResolutionError::Unknown { directive } => {
            assert!(rendered.contains(&directive.to_string()));
        }
        MediaResolutionError::WrongLane {
            directive,
            expected,
            observed,
        } => {
            for identity in [directive, expected, observed] {
                assert!(rendered.contains(&identity.to_string()));
            }
        }
        MediaResolutionError::Retired {
            directive,
            disposition,
        } => {
            assert!(rendered.contains(&directive.to_string()));
            assert!(rendered.contains(disposition_word(*disposition)));
        }
        MediaResolutionError::Stale { directive, ordinal }
        | MediaResolutionError::Missing { directive, ordinal }
        | MediaResolutionError::Undecodable {
            directive, ordinal, ..
        } => {
            assert!(rendered.contains(&directive.to_string()));
            assert!(rendered.contains(&format!("attachment {ordinal}")));
        }
        MediaResolutionError::Mismatched {
            directive,
            ordinal,
            field,
        } => {
            assert!(rendered.contains(&directive.to_string()));
            assert!(rendered.contains(&format!("attachment {ordinal}")));
            assert!(rendered.contains(field));
        }
    }
    if matches!(&error, MediaResolutionError::Undecodable { .. }) {
        assert!(
            error.source().is_some(),
            "undecodable authority retains its typed source"
        );
    }

    let mut diagnostics = vec![rendered];
    let mut source = error.source();
    while let Some(error) = source {
        diagnostics.push(error.to_string());
        source = error.source();
    }
    for diagnostic in diagnostics {
        for forbidden in [
            "data_url",
            "data:image",
            "iVBORw0KGgo=",
            "DirectiveAttachment",
            "operator-capture.png",
            "[137, 80, 78, 71",
        ] {
            assert!(
                !diagnostic.contains(forbidden),
                "media refusal exposed {forbidden:?}: {diagnostic}",
            );
        }
    }
    error
}

/// The same directive an operator would send, raised by a taste instead.
///
/// The absent halves are as deliberate as the present ones: a reminder is
/// prose addressed to a lane, so it opens no steering control and carries no
/// attachment. Everything downstream reads it as the plainest send.
fn reminded(key: &str) -> TasteReminded {
    TasteReminded {
        directive: directive(key),
        key: key.to_owned(),
        lane: lane(),
        taste: "polling".to_owned(),
        content: prose(),
        occupant: elsewhere(),
        compaction: 2,
        locus: transcript_locus(3, 1),
        text: "polling is not a completion event".to_owned(),
        expires: expires(),
    }
}

/// Identity of the authored prose the fixture taste fired on.
///
/// Computed rather than written out, so the fixture cannot drift from the
/// one domain a reminder's evidence is minted under.
fn prose() -> wire::Digest {
    wire::Digest::compute(wire::Domain::TasteProse, b"we will poll until it settles")
}

fn escalated(key: &str, stage: SteeringStage) -> SteerEscalated {
    SteerEscalated {
        directive: directive(key),
        key: key.to_owned(),
        lane: lane(),
        stage,
    }
}

fn acked(key: &str) -> SteerAcked {
    SteerAcked {
        directive: directive(key),
        key: key.to_owned(),
        lane: lane(),
        epoch: epoch(),
        locus: transcript_locus(3, 1),
    }
}

fn declined(key: &str) -> SteerDeclined {
    SteerDeclined {
        directive: directive(key),
        key: key.to_owned(),
        lane: lane(),
        epoch: epoch(),
        locus: transcript_locus(3, 1),
        reason: "the tree is red".to_owned(),
    }
}

fn transcript_locus(line: u32, column: u32) -> TranscriptLocus {
    TranscriptLocus {
        source: None,
        member: 1,
        offset: 40,
        length: 80,
        event: 0,
        line,
        column,
    }
}

fn epoch() -> CanonicalUuid {
    "018f82d4-6f59-7d06-92e3-bce913116303"
        .parse()
        .expect("fixture epoch")
}

fn directive(key: &str) -> CanonicalUuid {
    let suffix = key.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    format!("00000000-0000-4000-8000-{:012x}", suffix & 0xffff_ffff_ffff)
        .parse()
        .expect("fixture directive UUID")
}
