//! The source boundary a task lifecycle fact is bound to.
//!
//! Every boundary answer is either proven from stored heads or is a read
//! failure. There is no third answer here, and that is the whole point: the
//! `Incomplete` spelling exists for a producer with no lane to ask about, so
//! degrading a broken read into it would record *unknown usage* where the truth
//! is *broken authority*, and a reducer withholding the first would silently
//! withhold the second forever.

use super::*;

use wire::body::SourceBoundary;

/// A source head that cannot be read fails the boundary instead of answering one.
///
/// The corruption is the same disagreement between a stored row key and its
/// body that the transcript plane already refuses, reused here because it is the
/// cheapest way to make one head unreadable without inventing a second defect.
/// The pre-corruption read is not decoration: it binds the failure to the
/// corruption by proving this exact lane answered a boundary a moment earlier.
#[test]
fn an_unreadable_source_head_fails_the_boundary_instead_of_answering_one() {
    let journal = authority_fixture();
    let lane = transcript_source_head().lane;

    let proven = journal
        .read(|read| read.source_boundary(&lane))
        .expect("an intact journal proves the lane's boundary");
    assert_eq!(
        proven,
        SourceBoundary::Proven {
            heads: vec![transcript_source_head()],
        },
    );

    let mut renamed = transcript_source_head();
    renamed.stream = "sidecar".to_owned();
    journal
        .connection
        .execute(
            "UPDATE transcript_source_heads SET body = ?1",
            params![wire::encode(&renamed).expect("encode renamed transcript head")],
        )
        .expect("corrupt transcript head fixture");

    let error = journal
        .read(|read| read.source_boundary(&lane))
        .expect_err("an unreadable head cannot prove a boundary");
    assert!(
        error.to_string().contains("names"),
        "the boundary failed for some reason other than the unreadable head: {error}",
    );
}

/// An unreadable claim head fails the boundary instead of dropping its lane.
///
/// `lane_source_boundaries` answers only for lanes a live claim is bound to, so
/// the set of claimed lanes decides which lanes get measured at a membership
/// transition. A swallowed decode there would shorten that set, and a shortened
/// set encodes as the same empty list as *no claimed lane moved* -- attribution
/// lost in a shape no later reader could tell apart from nothing to attribute.
#[test]
fn an_unreadable_claim_head_fails_the_boundary_instead_of_dropping_its_lane() {
    let journal = authority_fixture();
    let lane = claim_head().lane;

    let measured = journal
        .read(|read| read.lane_source_boundaries(&[lane]))
        .expect("an intact journal measures the claimed lane");
    assert_eq!(
        measured.len(),
        1,
        "the fixture's claimed lane was not measured"
    );
    assert_eq!(measured[0].lane, lane);

    journal
        .connection
        .execute(
            "UPDATE claim_heads SET body = ?1",
            params![b"not a claim head".to_vec()],
        )
        .expect("corrupt claim head fixture");

    let error = journal
        .read(|read| read.lane_source_boundaries(&[lane]))
        .expect_err("an unreadable claim head cannot decide which lanes are claimed");
    assert!(
        error.to_string().contains("claim head"),
        "the boundary failed for some reason other than the unreadable claim head: {error}",
    );
}
