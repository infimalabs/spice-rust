//! Dispatch parity for the steering selection lifted out of [`super::run`].
//!
//! Every steering contract the parser can select is driven through
//! [`super::select`] here, so rerouting one arm to another
//! [`SelectedCommand`] reds this module rather than reaching the resident.

use std::ffi::OsString;

use clap::ArgMatches;
use wire::{CanonicalUuid, Disposition, FaultClass, Outcome};

use super::{SelectedCommand, select};

const LANE: &str = "0a2f5b6c-1d3e-4f50-8a91-b2c3d4e5f607";
const DIRECTIVE: &str = "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718";
const KEY: &str = "1kL5y53c";

/// Parse one argv the way the binary does, keeping the leaf matches owned.
fn parse(argv: &[&str]) -> (&'static crate::cli::Contract, ArgMatches) {
    let matches = crate::cli::command()
        .try_get_matches_from(argv)
        .expect("the declared steering argv parses");
    let contract = crate::cli::selected_contract(&matches).expect("the argv selects a contract");
    let leaf = crate::cli::selected_matches(&matches).clone();
    (contract, leaf)
}

/// Take the selection a steering contract answers, or fail with its path.
fn selection<'a>(
    leaf: &'a ArgMatches,
    contract: &crate::cli::Contract,
    lane_environment: Option<OsString>,
    key: Option<&'a steer::Key>,
) -> (SelectedCommand<'a>, Option<CanonicalUuid>) {
    match select(leaf, contract, lane_environment, key) {
        Ok(selection) => selection,
        Err(_) => panic!("{:?} selects a steering command", contract.path),
    }
}

fn lane() -> CanonicalUuid {
    LANE.parse().expect("the fixture lane is canonical")
}

fn directive() -> CanonicalUuid {
    DIRECTIVE
        .parse()
        .expect("the fixture directive is canonical")
}

#[test]
fn steer_selects_send_with_no_lane_context() {
    let (contract, leaf) = parse(&["spice", "steer", LANE, "hold the line"]);
    let (selected, context) = selection(&leaf, contract, None, None);

    assert!(matches!(
        selected,
        SelectedCommand::SteerSend { lane, message }
            if lane == self::lane() && message == "hold the line"
    ));
    assert_eq!(context, None);
}

#[test]
fn steer_cancel_selects_cancel_rather_than_recover() {
    let (contract, leaf) = parse(&["spice", "steer", "cancel", DIRECTIVE]);
    let (selected, context) = selection(&leaf, contract, None, None);

    assert!(matches!(
        selected,
        SelectedCommand::SteerCancel { directive } if directive == self::directive()
    ));
    assert_eq!(context, None);
}

#[test]
fn steer_resend_selects_live_resend_rather_than_lapsed_recovery() {
    let (contract, leaf) = parse(&["spice", "steer", "resend", DIRECTIVE]);
    let selected = select(&leaf, contract, None, None).expect("resend selects");
    assert!(matches!(
        selected.0,
        SelectedCommand::SteerResend { directive } if directive == self::directive()
    ));
}

#[test]
fn steer_recover_selects_recover_rather_than_cancel() {
    let (contract, leaf) = parse(&["spice", "steer", "recover", DIRECTIVE]);
    let (selected, context) = selection(&leaf, contract, None, None);

    assert!(matches!(
        selected,
        SelectedCommand::SteerRecover { directive } if directive == self::directive()
    ));
    assert_eq!(context, None);
}

#[test]
fn inbox_carries_its_lane_operand_and_the_shell_context() {
    let (contract, leaf) = parse(&["spice", "inbox", LANE]);
    let (selected, context) = selection(&leaf, contract, Some(OsString::from(LANE)), None);

    assert!(matches!(
        selected,
        SelectedCommand::SteerInbox { lane: Some(lane) } if lane == self::lane()
    ));
    assert_eq!(context, Some(lane()));
}

#[test]
fn hook_reads_its_lane_from_the_shell_rather_than_an_operand() {
    let (contract, leaf) = parse(&["spice", "hook", "claude"]);
    let (selected, context) = selection(&leaf, contract, Some(OsString::from(LANE)), None);

    assert!(matches!(
        selected,
        SelectedCommand::SteerInbox { lane: None }
    ));
    assert_eq!(context, Some(lane()));
}

#[test]
fn hook_refuses_outside_an_activated_lane_shell() {
    let (contract, leaf) = parse(&["spice", "hook", "claude"]);

    let Err(Outcome::Refusal(refusal)) = select(&leaf, contract, None, None) else {
        panic!("hook without SPICE_LANE refuses");
    };
    assert_eq!(refusal.reason(), "steer::lane-context-absent");
}

#[test]
fn ack_carries_the_key_run_parsed_and_the_acked_disposition() {
    let (contract, leaf) = parse(&["spice", "ack", KEY]);
    let parsed = KEY.parse::<steer::Key>().expect("the fixture key admits");
    let (selected, context) = selection(&leaf, contract, Some(OsString::from(LANE)), Some(&parsed));

    assert!(matches!(
        selected,
        SelectedCommand::SteerAck { key, disposition }
            if *key == parsed && disposition == Disposition::Acked
    ));
    assert_eq!(context, Some(lane()));
}

#[test]
fn ack_decline_carries_the_declined_disposition() {
    let (contract, leaf) = parse(&["spice", "ack", KEY, "--decline"]);
    let parsed = KEY.parse::<steer::Key>().expect("the fixture key admits");
    let (selected, _) = selection(&leaf, contract, None, Some(&parsed));

    assert!(matches!(
        selected,
        SelectedCommand::SteerAck { key, disposition }
            if *key == parsed && disposition == Disposition::Declined
    ));
}

#[test]
fn a_non_steering_contract_faults_rather_than_selecting_a_steering_command() {
    let (contract, leaf) = parse(&["spice", "down"]);

    let Err(Outcome::Fault(fault)) = select(&leaf, contract, None, None) else {
        panic!("a non-steering contract faults");
    };
    assert_eq!(fault.class, FaultClass::Environment);
    assert_eq!(fault.reason, "steer::dispatch-invalid");
}

#[test]
fn acknowledgment_key_parses_only_for_the_ack_contract() {
    let (ack, ack_leaf) = parse(&["spice", "ack", KEY]);
    let (inbox, inbox_leaf) = parse(&["spice", "inbox"]);

    assert_eq!(
        super::acknowledgment_key(&ack_leaf, ack).ok().flatten(),
        Some(KEY.parse::<steer::Key>().expect("the fixture key admits"))
    );
    assert_eq!(
        super::acknowledgment_key(&inbox_leaf, inbox).ok().flatten(),
        None
    );
}
