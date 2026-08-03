//! Repository documents: per-actor caps and peer-held bound claims.
//!
//! Both properties belong to the acting actor rather than to the document, so
//! each is asserted by acquiring one fixture repository twice under different
//! actors instead of by calling the measurement engine directly.

use gate::{
    ActorCapture, ActorInputs, ClaimCapture, ClaimStatus, CoreEvidence, CoreObservation,
    CoreStanding, CurrentState, GateId, PeerClaim, PredecessorSources, RenameMap, RepoDocLimit,
};

mod support;

use support::{Tree, acquired, acquired_arm, contract_repository, git_commit};

/// One document, two actors, two caps.
///
/// Elastic headroom is seeded from the actor that is acting, so the same
/// oversized document is judged against a different cap for each of them. Both
/// expected caps are the predecessor `jittered_flex_limit(10000, 15000,
/// "DOCTRINE.md", actor, jitter_percent=5)` for that actor's spelling. An engine
/// seeded from the document's own path instead hands every actor 14800, so it
/// can produce neither number and cannot make both assertions hold at once.
#[test]
fn repository_documents_are_selected_measured_and_judged_only_by_the_engine() {
    let repository = contract_repository("core-acquire-repo-docs");
    let doctrine = "x".repeat(20_000);
    repository.write("DOCTRINE.md", &doctrine);
    repository.write(
        "docs/reference/generated/REFERENCE.md",
        "unlimited doctrine",
    );
    git_commit(&repository, "repository document fixture");

    assert_repository_documents(&repository, "worktree-author", 14_850);
    assert_repository_documents(&repository, "worktree-peer", 15_000);

    repository.remove();
}

/// Acquire one fixture as `actor` and assert the whole document inventory.
fn assert_repository_documents(repository: &Tree, actor: &str, active_limit: u64) {
    let report = acquired(
        repository,
        ClaimCapture::Idle,
        ActorCapture::Acting {
            actor: actor.to_owned(),
            bound_claims: Vec::new(),
        },
    );
    let arm = acquired_arm(&report, GateId::RepoDocs);
    let CoreObservation::Measured {
        evidence: CoreEvidence::RepoDocs(evidence),
    } = arm.observation()
    else {
        panic!("repository-document provider emitted no measurement: {arm:?}");
    };
    let [bounded, unlimited] = evidence.measurements() else {
        panic!("unexpected repository-document inventory: {evidence:?}");
    };
    assert_eq!(bounded.path(), "DOCTRINE.md");
    assert_eq!(bounded.characters(), 20_000);
    assert_eq!(
        bounded.bound(),
        RepoDocLimit::AtMost {
            characters: active_limit
        },
        "{actor} received a cap no predecessor run seeded from that actor produces"
    );
    assert!(!bounded.peer_claim());
    assert_eq!(unlimited.path(), "docs/reference/generated/REFERENCE.md");
    assert_eq!(unlimited.characters(), 18);
    assert_eq!(unlimited.bound(), RepoDocLimit::Unlimited);
    assert!(!unlimited.peer_claim());
    assert_eq!(arm.standing(), CoreStanding::Failed);
    assert!(arm.diagnostic().contains(&format!(
        "DOCTRINE.md: 20000 characters (cap {active_limit})"
    )));
    assert!(!arm.diagnostic().contains("REFERENCE"));
}

/// A document a peer already claimed is judged as someone else's seam.
///
/// The two documents are identical across both acquisitions below; only the
/// ledger the acting actor carries differs. An acquisition that reports a
/// constant `false` for every document produces the character-cap sentence in
/// both, so the two halves cannot both hold unless the claim is really
/// resolved. Both documents are claimed each time, so the untouched one also
/// shows that a document inside its bound claims nothing regardless of who
/// holds its ledger entry.
#[test]
fn a_peer_held_bound_claim_redirects_the_document_diagnostic() {
    let repository = contract_repository("core-acquire-repo-docs-claimed");
    repository.write("DOCTRINE.md", &"x".repeat(20_000));
    repository.write("GUIDE.md", "a document well inside its bound");
    git_commit(&repository, "repository document fixture");

    assert_claimed_document(
        &repository,
        "worktree-author",
        minted_claims(&repository, "worktree-peer"),
        true,
        concat!(
            "repo-truth docs hit peer-held bound claims; keep this change append-only or move ",
            "to another seam:"
        ),
    );
    assert_claimed_document(
        &repository,
        "  worktree-author  ",
        minted_claims(&repository, "worktree-author"),
        false,
        "repo-truth docs exceed the character cap; tighten the doctrine:",
    );

    repository.remove();
}

/// Acquire as `acting` carrying `claims` and assert what the over-limit
/// document was told while its within-bounds neighbour claims nothing.
///
/// The caller hands over whatever spelling it holds, so the padded call below
/// is judged against the same cap and the same owner as the bare one: a claim
/// belongs to an identity, not to a spelling with surrounding space.
fn assert_claimed_document(
    repository: &Tree,
    acting: &str,
    claims: Vec<PeerClaim>,
    peer_claim: bool,
    header: &str,
) {
    let report = acquired(
        repository,
        ClaimCapture::Idle,
        ActorCapture::Acting {
            actor: acting.to_owned(),
            bound_claims: claims,
        },
    );
    let arm = acquired_arm(&report, GateId::RepoDocs);
    let CoreObservation::Measured {
        evidence: CoreEvidence::RepoDocs(evidence),
    } = arm.observation()
    else {
        panic!("repository-document provider emitted no measurement: {arm:?}");
    };
    let [bounded, within] = evidence.measurements() else {
        panic!("unexpected repository-document inventory: {evidence:?}");
    };
    assert_eq!(bounded.path(), "DOCTRINE.md");
    assert_eq!(
        bounded.bound(),
        RepoDocLimit::AtMost { characters: 14_850 },
        "the acting actor's cap is unchanged by who holds the claim"
    );
    assert_eq!(arm.standing(), CoreStanding::Failed);
    assert_eq!(
        bounded.peer_claim(),
        peer_claim,
        "the claim over DOCTRINE.md is held by {}",
        if peer_claim { "a peer" } else { "the actor" }
    );
    assert!(
        arm.diagnostic().starts_with(header),
        "a {} document was told the wrong thing: {}",
        if peer_claim { "peer-held" } else { "self-held" },
        arm.diagnostic()
    );
    assert_eq!(within.path(), "GUIDE.md");
    assert!(
        matches!(within.bound(), RepoDocLimit::AtMost { .. }),
        "the neighbour must carry a cap it stayed inside, not no cap at all"
    );
    assert!(
        !within.peer_claim(),
        "a document inside its bound breached nothing to be claimed over"
    );
    assert!(!arm.diagnostic().contains("GUIDE"));
}

/// Genuine claims over both fixture documents, created by `actor` through the
/// ledger itself.
///
/// A claim assembled by the test could disagree with the ownership rule
/// acquisition resolves against; one minted by a real transaction cannot.
fn minted_claims(repository: &Tree, actor: &str) -> Vec<PeerClaim> {
    let paths = ["DOCTRINE.md", "GUIDE.md"];
    let state_root = repository.path(".spice");
    let sources = PredecessorSources::new(&state_root, &state_root);
    let mut state = CurrentState::load(repository.path("current-state.json"), &sources)
        .expect("load one empty current ledger");
    let renames = RenameMap::new(Vec::<(String, String)>::new()).expect("no staged renames");
    state
        .begin_run(&renames, paths, 1_700_000_000.0)
        .expect("begin one run holding the claimed paths live");
    let decisions = state
        .claim(
            paths,
            &ActorInputs::new(paths[0])
                .expect("actor provenance for the claimed paths")
                .with_active_actor(actor),
        )
        .expect("claim the paths for one actor");
    assert!(
        decisions
            .iter()
            .all(|decision| decision.status() == ClaimStatus::Claimed),
        "an empty ledger leaves every requested path free: {decisions:?}"
    );
    state
        .reconcile(std::iter::empty())
        .expect("reconcile an empty observation inventory");
    state
        .finish()
        .expect("finish the claiming transaction")
        .after()
        .claims()
        .to_vec()
}
