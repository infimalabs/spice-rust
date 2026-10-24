use super::*;

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture uuid")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture position")
}

fn task() -> TaskStamp {
    "1kKRRsDR".parse().expect("fixture task stamp")
}

fn author(lane: u64, occupant: u64) -> ClaimAuthor {
    ClaimAuthor {
        lane: uuid(lane),
        occupant: uuid(occupant),
    }
}

/// `who` claims the task and completes its phase.
fn worked(index: &mut ReviewAuthors, who: ClaimAuthor) {
    index.claim(task(), who.lane, who.occupant);
    index.complete(&Actor::Lane(who.lane), &task());
}

/// `who` claims the task for review and records `verdict` at `at`.
fn reviewed(index: &mut ReviewAuthors, who: ClaimAuthor, at: u64, verdict: Verdict) {
    index.claim(task(), who.lane, who.occupant);
    index.review(seq(at), &task(), verdict);
}

fn authors(index: &ReviewAuthors, at: u64) -> Option<Result<FindingAuthors, AuthorshipFault>> {
    index.findings.get(&seq(at)).copied()
}

#[test]
fn each_finding_keeps_the_authors_of_the_phase_it_judged_across_rework() {
    let mut index = ReviewAuthors::default();
    let (first_author, first_reviewer) = (author(1, 2), author(3, 4));
    let (second_author, second_reviewer) = (author(1, 5), author(6, 7));

    worked(&mut index, first_author);
    reviewed(&mut index, first_reviewer, 10, Verdict::Finding);
    worked(&mut index, second_author);
    reviewed(&mut index, second_reviewer, 20, Verdict::Finding);

    assert_eq!(
        authors(&index, 10),
        Some(Ok(FindingAuthors {
            author: first_author,
            reviewer: first_reviewer,
        }))
    );
    assert_eq!(
        authors(&index, 20),
        Some(Ok(FindingAuthors {
            author: second_author,
            reviewer: second_reviewer,
        }))
    );
}

#[test]
fn the_completing_claim_is_the_author_after_carries_and_releases() {
    let mut index = ReviewAuthors::default();
    // A freed claim authors nothing; the claim that completes the phase does,
    // and a rebound replaces the occupant the claim was won with.
    index.claim(task(), uuid(8), uuid(9));
    index.release(&task());
    index.claim(task(), uuid(1), uuid(2));
    index.claim(task(), uuid(1), uuid(5));
    index.complete(&Actor::Lane(uuid(1)), &task());
    reviewed(&mut index, author(3, 4), 10, Verdict::Finding);

    assert_eq!(
        authors(&index, 10),
        Some(Ok(FindingAuthors {
            author: author(1, 5),
            reviewer: author(3, 4),
        }))
    );
}

#[test]
fn a_clean_review_records_no_finding_and_ends_the_reviewer_claim() {
    let mut index = ReviewAuthors::default();
    worked(&mut index, author(1, 2));
    reviewed(&mut index, author(3, 4), 10, Verdict::Clean);
    // The clean review's claim is over, so a later review without its own
    // claim has no reviewer.
    index.review(seq(20), &task(), Verdict::Finding);

    assert_eq!(authors(&index, 10), None);
    assert_eq!(
        authors(&index, 20),
        Some(Err(AuthorshipFault::ReviewUnclaimed))
    );
}

#[test]
fn undefined_authorship_is_recorded_as_its_fault() {
    let mut unclaimed = ReviewAuthors::default();
    unclaimed.complete(&Actor::Lane(uuid(1)), &task());
    reviewed(&mut unclaimed, author(3, 4), 10, Verdict::Finding);
    assert_eq!(
        authors(&unclaimed, 10),
        Some(Err(AuthorshipFault::CompletionUnclaimed))
    );

    let mut mismatched = ReviewAuthors::default();
    mismatched.claim(task(), uuid(1), uuid(2));
    mismatched.complete(&Actor::Lane(uuid(6)), &task());
    reviewed(&mut mismatched, author(3, 4), 10, Verdict::Finding);
    assert_eq!(
        authors(&mismatched, 10),
        Some(Err(AuthorshipFault::CompletionMismatched))
    );

    let mut uncompleted = ReviewAuthors::default();
    reviewed(&mut uncompleted, author(3, 4), 10, Verdict::Finding);
    assert_eq!(
        authors(&uncompleted, 10),
        Some(Err(AuthorshipFault::ReviewUncompleted))
    );
}
