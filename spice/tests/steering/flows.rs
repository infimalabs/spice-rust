use super::*;

fn restart_preserves_pending_journal_authority(
    fixture: &Fixture,
    resident: RunningResident,
    lane: wire::CanonicalUuid,
) {
    let lane = lane.to_string();
    let sent = text_directive(&invoke(
        fixture,
        &["steer", &lane, "remain authoritative across restart"],
        false,
    ));
    let (_, pending) = inbox(fixture, &lane);
    assert!(holds(&pending, sent.directive));

    stop(fixture, resident);
    let mut restarted = RunningResident::spawn(fixture);
    restarted.assert_ready(fixture);
    let (_, pending) = inbox(fixture, &lane);
    assert!(
        holds(&pending, sent.directive),
        "restart retired the pending operator directive"
    );
    let canceled = text_directive(&invoke(
        fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    stop(fixture, restarted);
}

pub(super) fn directive_authored(fixture: &Fixture, directive: wire::CanonicalUuid) -> Instant {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    snapshot
        .facts()
        .iter()
        .find_map(|envelope| {
            let authored = envelope
                .fact
                .read::<wire::body::SteerSent>()
                .map(|body| body.directive)
                .or_else(|| {
                    envelope
                        .fact
                        .read::<wire::body::TasteReminded>()
                        .map(|body| body.directive)
                })?;
            (authored == directive).then_some(envelope.at)
        })
        .expect("pending directive has an authoring fact")
}

pub(super) fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

pub(super) fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

#[test]
fn resend_before_the_first_horizon_redelivers_one_unchanged_stable_directive() {
    let (fixture, resident, lanes) = setup();
    let lane = lanes.target.to_string();
    let sent = text_directive(&invoke(
        &fixture,
        &["steer", &lane, "deliver this stable direction again"],
        false,
    ));
    let resent = text_directive(&invoke(
        &fixture,
        &["steer", "resend", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(resent.directive, sent.directive);
    assert_eq!(resent.key, sent.key);
    assert_eq!(resent.authored, sent.authored);
    assert_eq!(resent.disposition, sent.disposition);

    let (_, pending) = inbox(&fixture, &lane);
    let pending = pending
        .iter()
        .find(|pending| pending["directive"] == sent.directive.to_string())
        .expect("redelivered directive remains pending");
    assert_eq!(pending["key"], sent.key);
    assert_eq!(pending["stage"], "published");

    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let snapshot = journal.fact_snapshot().expect("steering facts read");
    assert!(snapshot.facts().iter().all(|envelope| {
        envelope
            .fact
            .read::<wire::body::SteerEscalated>()
            .is_none_or(|body| body.directive != sent.directive)
    }));

    let canceled = text_directive(&invoke(
        &fixture,
        &["steer", "cancel", &sent.directive.to_string()],
        false,
    ));
    assert_eq!(canceled.directive, sent.directive);
    assert_eq!(canceled.key, sent.key);
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
    stop(&fixture, resident);
}

fn shared_canvas_fanout(
    fixture: &Fixture,
    lanes: Lanes,
    correction: &str,
) -> (
    wire::CanonicalUuid,
    wire::intent::DirectiveAnswer,
    wire::intent::DirectiveAnswer,
) {
    let fanout = uuid("00000000-0000-4000-8000-000000000080");
    let (mut socket, discovery) = operator_socket(fixture);
    let target = send_operator_direction(
        &mut socket,
        &discovery,
        uuid("00000000-0000-4000-8000-000000000081"),
        fanout,
        lanes.target,
        current_occupant(fixture, lanes.target),
        correction,
    );
    let other = send_operator_direction(
        &mut socket,
        &discovery,
        uuid("00000000-0000-4000-8000-000000000082"),
        fanout,
        lanes.other,
        current_occupant(fixture, lanes.other),
        correction,
    );
    drop(socket);
    (fanout, target, other)
}

fn distinct_cli_fanouts(
    fixture: &Fixture,
    lanes: Lanes,
    correction: &str,
) -> (wire::intent::DirectiveAnswer, wire::intent::DirectiveAnswer) {
    let target_lane = lanes.target.to_string();
    let other_lane = lanes.other.to_string();
    let target = text_directive(&invoke(
        fixture,
        &["steer", &target_lane, correction],
        false,
    ));
    let other = text_directive(&invoke(fixture, &["steer", &other_lane, correction], false));
    (target, other)
}

fn assert_fanout_authority(
    fixture: &Fixture,
    fanout: wire::CanonicalUuid,
    target: &wire::intent::DirectiveAnswer,
    other: &wire::intent::DirectiveAnswer,
    cli_target: &wire::intent::DirectiveAnswer,
    cli_other: &wire::intent::DirectiveAnswer,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("steering journal opens");
    let facts = journal.fact_snapshot().expect("steering facts read");
    let sent_fanout = |directive| {
        facts
            .facts()
            .iter()
            .filter_map(|fact| fact.fact.read::<wire::body::SteerSent>())
            .find(|sent| sent.directive == directive)
            .expect("operator direction is durable")
            .fanout
            .expect("current operator direction carries fanout")
    };
    assert_eq!(sent_fanout(target.directive), fanout);
    assert_eq!(sent_fanout(other.directive), fanout);
    let cli_target_fanout = sent_fanout(cli_target.directive);
    let cli_other_fanout = sent_fanout(cli_other.directive);
    assert_ne!(cli_target_fanout, fanout);
    assert_ne!(cli_other_fanout, fanout);
    assert_ne!(cli_target_fanout, cli_other_fanout);
    for directive in [
        target.directive,
        other.directive,
        cli_target.directive,
        cli_other.directive,
    ] {
        let head = journal
            .read(|read| read.steering_head(directive))
            .expect("steering head reads")
            .expect("steering head exists");
        assert_eq!(head.body().fanout, Some(sent_fanout(directive)));
    }
}

/// One operator fanout names every directive it sent, while each CLI send
/// mints its own.
#[test]
fn a_canvas_fanout_is_shared_while_cli_sends_mint_their_own() {
    const CORRECTION: &str = "Do not use grep for repository searches; use rg instead.";

    let (fixture, resident, lanes) = setup();
    let (fanout, target, other) = shared_canvas_fanout(&fixture, lanes, CORRECTION);
    let (cli_target, cli_other) = distinct_cli_fanouts(&fixture, lanes, CORRECTION);
    assert_fanout_authority(&fixture, fanout, &target, &other, &cli_target, &cli_other);
    stop(&fixture, resident);
}

#[test]
fn steering_commands_preserve_pending_authority_through_cancel_and_restart() {
    let (fixture, mut resident, lanes) = setup();
    publication_path(&fixture, &mut resident, lanes);
    cancellation_path(&fixture, lanes.target);
    restart_preserves_pending_journal_authority(&fixture, resident, lanes.target);
}
