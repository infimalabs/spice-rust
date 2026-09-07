//! Dispatch parity for the steering selection lifted out of [`super::run`].
//!
//! Every steering contract the parser can select is driven through
//! [`super::select`] here, so rerouting one arm to another
//! [`SelectedCommand`] reds this module rather than reaching the resident.

use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use clap::ArgMatches;
use wire::intent::{CommandAnswer, InboxAnswer};
use wire::{AgentMediaReference, CanonicalUuid, Disposition, FaultClass, Outcome};

use super::{SelectedCommand, hook_document_at, select};

const LANE: &str = "0a2f5b6c-1d3e-4f50-8a91-b2c3d4e5f607";
const DIRECTIVE: &str = "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718";
const KEY: &str = "1kL5y53c";
const EPOCH: &str = "2c4f7d8e-3f50-4162-ac13-d4e5f6071829";
const OCCUPANT: &str = "3d508e9f-4051-4273-bd24-e5f60718293a";
const OTHER_OCCUPANT: &str = "4e619fa0-5162-4384-8e35-f60718293a4b";
const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";
const SECOND_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgpB";
static NEXT_HOOK_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
        panic!("hook without {} refuses", steer::hook::LANE);
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

#[test]
fn simultaneous_duplicate_content_preserves_each_directive_identity() {
    let scratch = HookScratch::new();
    let simultaneous = inbox_answer(
        OCCUPANT,
        10,
        &[
            ("1kFirst1", "published", "hold the exact boundary"),
            ("1kSecond", "published", "hold the exact boundary"),
        ],
    );

    let document = deliver(&scratch, &simultaneous).expect("duplicate keys publish together");
    let context = additional_context(&document);
    assert!(context.contains("key=1kFirst1"));
    assert!(context.contains("key=1kSecond"));
    assert_eq!(context.matches("hold the exact boundary").count(), 2);
    assert!(context.contains("ack 1kFirst1"));
    assert!(context.contains("ack 1kSecond"));
    assert_eq!(deliver(&scratch, &simultaneous), None);
}

#[test]
fn sequential_duplicate_content_publishes_each_new_key_without_replaying_text() {
    let scratch = HookScratch::new();
    let first = inbox_answer(
        OCCUPANT,
        11,
        &[("1kFirst1", "published", "hold the exact boundary")],
    );
    let second = inbox_answer(
        OCCUPANT,
        11,
        &[("1kSecond", "published", "hold the exact boundary")],
    );

    let initial = deliver(&scratch, &first).expect("first content publishes");
    assert!(additional_context(&initial).contains("hold the exact boundary"));
    let alias = deliver(&scratch, &second).expect("new duplicate key remains visible");
    let context = additional_context(&alias);
    assert!(context.contains("1kSecond"));
    assert!(context.contains("ack 1kSecond"));
    assert!(context.contains("Same-content steering was already delivered"));
    assert!(!context.contains("hold the exact boundary"));
    assert_eq!(deliver(&scratch, &second), None);
}

#[test]
fn task_contract_revision_is_a_system_hud_notice_not_authored_steering() {
    let scratch = HookScratch::new();
    let answer = task_contract_answer();
    let CommandAnswer::Inbox(inbox) = &answer else {
        panic!("fixture answer is an inbox");
    };
    assert!(inbox.lanes[0].pending.is_empty());
    assert_eq!(inbox.lanes[0].task_contract_notices.len(), 1);

    let document = deliver(&scratch, &answer).expect("task-contract HUD publishes");
    let context = additional_context(&document);
    assert!(context.starts_with("Rust Spice system status."));
    assert!(context.contains("TASK CONTRACT REVISION"));
    assert!(
        context.contains("task=1kKRRsDP edit=44 claim=33 key=1kL5y53c"),
        "{context}"
    );
    assert!(context.contains("Task contract changed for SPICE-1kKRRsDP."));
    assert!(context.contains("ack 1kL5y53c"));
    assert!(!context.contains("Rust Spice steering is pending"));
    assert_eq!(deliver(&scratch, &answer), None);
}

#[test]
fn task_contract_status_and_authored_direction_share_one_hook_document_without_conflation() {
    let scratch = HookScratch::new();
    let CommandAnswer::Inbox(mut mixed) = task_contract_answer() else {
        panic!("task-contract fixture is an inbox");
    };
    let CommandAnswer::Inbox(authored) = inbox_answer(
        OCCUPANT,
        1,
        &[("1kFirst1", "published", "Inspect the authored request.")],
    ) else {
        panic!("authored fixture is an inbox");
    };
    mixed.lanes[0].pending = authored.lanes[0].pending.clone();

    let document =
        deliver(&scratch, &CommandAnswer::Inbox(mixed)).expect("mixed hook status publishes once");
    let context = additional_context(&document);
    let system = context
        .find("Rust Spice system status.")
        .expect("system HUD");
    let steering = context
        .find("Rust Spice steering is pending.")
        .expect("authored steering section");
    assert!(system < steering);
    assert!(context.contains("TASK CONTRACT REVISION"));
    assert!(context.contains("Inspect the authored request."));
    assert!(context.contains("ack 1kL5y53c"));
    assert!(context.contains("ack 1kFirst1"));
}

#[test]
fn content_only_delivery_state_migrates_to_key_tracking_once() {
    let scratch = HookScratch::new();
    let first = inbox_answer(
        OCCUPANT,
        12,
        &[("1kBefore", "published", "retain old suppression")],
    );
    let later = inbox_answer(
        OCCUPANT,
        12,
        &[("1kLater1", "published", "retain old suppression")],
    );
    assert!(deliver(&scratch, &first).is_some());
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&scratch.state).expect("read current delivery state"))
            .expect("delivery state decodes");
    state["lanes"][0]
        .as_object_mut()
        .expect("lane state is an object")
        .remove("delivered_keys");
    fs::write(
        &scratch.state,
        serde_json::to_vec(&state).expect("retained state encodes"),
    )
    .expect("publish retained state fixture");

    let migrated = deliver(&scratch, &later).expect("retained content admits its unseen key");
    let context = additional_context(&migrated);
    assert!(context.contains("1kLater1"));
    assert!(!context.contains("retain old suppression"));
    assert_eq!(deliver(&scratch, &later), None);
}

#[test]
fn a_new_compaction_epoch_reopens_identical_content_for_the_same_agent() {
    let scratch = HookScratch::new();
    let first = inbox_answer(
        OCCUPANT,
        20,
        &[("1kBefore", "reminded", "recheck the invariant")],
    );
    let compacted = inbox_answer(
        OCCUPANT,
        21,
        &[("1kAfter1", "reminded", "recheck the invariant")],
    );

    assert!(deliver(&scratch, &first).is_some());
    assert_eq!(deliver(&scratch, &first), None);
    let after = deliver(&scratch, &compacted).expect("compaction reopens delivery");
    assert!(additional_context(&after).contains("key=1kAfter1"));
}

#[test]
fn a_different_agent_and_materially_different_output_remain_deliverable() {
    let scratch = HookScratch::new();
    let original = inbox_answer(OCCUPANT, 30, &[("1kOrigin", "published", "inspect alpha")]);
    let changed = inbox_answer(OCCUPANT, 30, &[("1kChange", "published", "inspect beta")]);
    let successor = inbox_answer(
        OTHER_OCCUPANT,
        30,
        &[("1kNext11", "published", "inspect alpha")],
    );

    assert!(deliver(&scratch, &original).is_some());
    assert!(deliver(&scratch, &changed).is_some());
    assert_eq!(deliver(&scratch, &changed), None);
    assert!(deliver(&scratch, &successor).is_some());
}

#[test]
fn concurrent_hook_invocations_share_one_durable_delivery_decision() {
    let scratch = Arc::new(HookScratch::new());
    let answer = Arc::new(inbox_answer(
        OCCUPANT,
        40,
        &[("1kRace11", "overdue", "serialize this publication")],
    ));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let scratch = Arc::clone(&scratch);
            let answer = Arc::clone(&answer);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                deliver(&scratch, &answer)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let delivered = workers
        .into_iter()
        .map(|worker| worker.join().expect("hook worker completes").is_some())
        .filter(|delivered| *delivered)
        .count();

    assert_eq!(delivered, 1);
}

#[test]
fn a_delayed_pre_compaction_snapshot_cannot_reopen_an_older_epoch() {
    let scratch = HookScratch::new();
    let before = inbox_answer(
        OCCUPANT,
        50,
        &[("1kOlder1", "published", "keep this monotone")],
    );
    let after = inbox_answer(
        OTHER_OCCUPANT,
        51,
        &[("1kNewer1", "published", "keep this monotone")],
    );

    assert!(deliver(&scratch, &before).is_some());
    assert!(deliver(&scratch, &after).is_some());
    assert_eq!(deliver(&scratch, &before), None);
    assert_eq!(deliver(&scratch, &after), None);
}

#[test]
fn a_delayed_predecessor_cannot_mutate_current_media_delivery() {
    let scratch = HookScratch::new();
    let predecessor = media_inbox_answer(
        OCCUPANT,
        50,
        "inspect the predecessor capture",
        &[("predecessor.png", PNG_DATA_URL)],
    );
    let current = media_inbox_answer(
        OTHER_OCCUPANT,
        51,
        "inspect the current capture",
        &[("current.png", SECOND_PNG_DATA_URL)],
    );

    let predecessor_document =
        deliver(&scratch, &predecessor).expect("predecessor media publishes");
    let predecessor_path = projection_path_from_document(&predecessor_document);
    let current_document = deliver(&scratch, &current).expect("current media publishes");
    let current_path = projection_path_from_document(&current_document);
    assert!(
        !predecessor_path.exists(),
        "current media retires predecessor projection"
    );
    let state = fs::read(&scratch.state).expect("current delivery state is readable");
    let bytes = fs::read(&current_path).expect("current projection is readable");

    assert_eq!(deliver(&scratch, &predecessor), None);
    assert_eq!(
        fs::read(&scratch.state).expect("state remains readable"),
        state,
        "a stale predecessor leaves serialized delivery authority unchanged",
    );
    assert_eq!(
        fs::read(&current_path).expect("current projection remains readable"),
        bytes,
        "a stale predecessor leaves current projection bytes unchanged",
    );
    assert!(!predecessor_path.exists(), "stale media stays retired");
    assert_eq!(
        deliver(&scratch, &current),
        None,
        "stale predecessor input cannot reopen current media delivery",
    );
}

#[test]
fn hook_media_references_are_compact_and_preserve_attachment_order() {
    let scratch = HookScratch::new();
    let answer = media_inbox_answer(
        OCCUPANT,
        60,
        "inspect both captures",
        &[
            ("first.png", PNG_DATA_URL),
            ("second.png", SECOND_PNG_DATA_URL),
        ],
    );

    let document = deliver(&scratch, &answer).expect("media directive publishes");
    let context = additional_context(&document);
    let references = media_references(&context);
    let projections = media_projections(&context);

    assert_eq!(references.len(), 2);
    assert_eq!(projections.len(), 2);
    assert_eq!(references[0].name(), "first.png");
    assert_eq!(references[0].ordinal(), 0);
    assert_eq!(references[1].name(), "second.png");
    assert_eq!(references[1].ordinal(), 1);
    for (projection, reference, source) in [
        (&projections[0], &references[0], PNG_DATA_URL),
        (&projections[1], &references[1], SECOND_PNG_DATA_URL),
    ] {
        assert_eq!(projection["lane"], LANE);
        assert_eq!(projection["directive"], DIRECTIVE);
        assert_eq!(projection["ordinal"], reference.ordinal());
        assert_eq!(projection["digest"], reference.digest().as_str());
        assert_eq!(projection["bytes"], reference.bytes());
        assert_eq!(projection["content_type"], reference.content_type());
        assert_eq!(projection["standing"], "live");
        let path = PathBuf::from(projection["path"].as_str().expect("projection path"));
        assert!(path.starts_with(&scratch.projections));
        let expected = wire::Media::admit("capture.png", "image/png", source)
            .expect("projection fixture media")
            .decoded_bytes()
            .expect("decode projection fixture");
        assert_eq!(fs::read(&path).expect("read projected media"), expected);
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&scratch.projections)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700,
    );
    assert!(!context.contains("data_url"));
    assert!(!context.contains("data:image"));
    assert!(!context.contains("iVBORw0KGgo="));
    assert!(!context.contains("iVBORw0KGgpB"));
    assert_eq!(
        deliver(&scratch, &answer),
        None,
        "an exact repeat is suppressed"
    );
}

#[test]
fn retiring_media_removes_only_its_exact_disposable_projection() {
    let scratch = HookScratch::new();
    let answer = media_inbox_answer(
        OCCUPANT,
        64,
        "inspect this capture",
        &[("capture.png", PNG_DATA_URL)],
    );
    let document = deliver(&scratch, &answer).expect("media directive publishes");
    let projection = media_projections(&additional_context(&document))
        .pop()
        .expect("one media projection");
    let path = PathBuf::from(projection["path"].as_str().expect("projection path"));
    let sentinel = scratch.projections.join("sentinel");
    fs::write(&sentinel, b"unowned").expect("write unrelated projection-root file");

    let retired = inbox_answer(OCCUPANT, 64, &[]);
    assert_eq!(deliver(&scratch, &retired), None);
    assert!(!path.exists(), "retired projection remains");
    assert_eq!(fs::read(&sentinel).unwrap(), b"unowned");
    let CommandAnswer::Inbox(original) = answer else {
        panic!("fixture answer remains an inbox")
    };
    assert_eq!(original.lanes[0].pending[0].attachments.len(), 1);
}

#[test]
fn hook_delivery_identity_observes_image_bytes_even_when_text_does_not_change() {
    for text in ["", "inspect the capture"] {
        let scratch = HookScratch::new();
        let first = media_inbox_answer(OCCUPANT, 61, text, &[("capture.png", PNG_DATA_URL)]);
        let changed =
            media_inbox_answer(OCCUPANT, 61, text, &[("capture.png", SECOND_PNG_DATA_URL)]);

        let first = deliver(&scratch, &first).expect("first image publishes");
        let changed = deliver(&scratch, &changed).expect("changed image publishes");
        assert_ne!(
            media_references(&additional_context(&first)),
            media_references(&additional_context(&changed)),
            "image bytes must participate in delivery identity for {text:?}",
        );
    }
}

#[test]
fn identical_hook_media_reopens_for_a_new_occupant_and_compaction() {
    let scratch = HookScratch::new();
    let original = media_inbox_answer(
        OCCUPANT,
        62,
        "inspect the capture",
        &[("capture.png", PNG_DATA_URL)],
    );
    let successor = media_inbox_answer(
        OTHER_OCCUPANT,
        62,
        "inspect the capture",
        &[("capture.png", PNG_DATA_URL)],
    );
    let compacted = media_inbox_answer(
        OTHER_OCCUPANT,
        63,
        "inspect the capture",
        &[("capture.png", PNG_DATA_URL)],
    );

    assert!(deliver(&scratch, &original).is_some());
    assert_eq!(deliver(&scratch, &original), None);
    assert!(deliver(&scratch, &successor).is_some());
    assert_eq!(deliver(&scratch, &successor), None);
    assert!(deliver(&scratch, &compacted).is_some());
    assert_eq!(deliver(&scratch, &compacted), None);
}

fn inbox_answer(
    occupant: &str,
    compaction: u64,
    directives: &[(&str, &str, &str)],
) -> CommandAnswer {
    let pending = directives
        .iter()
        .enumerate()
        .map(|(index, (key, stage, text))| {
            serde_json::json!({
                "directive": if index == 0 {
                    "5f72a0b1-6273-4495-9f46-0718293a4b5c"
                } else {
                    "6073b1c2-7384-45a6-8057-18293a4b5c6d"
                },
                "key": key,
                "text": text,
                "authored": "2026-08-30T09:00:00.000Z",
                "stage": stage,
                "expires": "2026-08-30T10:00:00.000Z",
            })
        })
        .collect::<Vec<_>>();
    let inbox: InboxAnswer = serde_json::from_value(serde_json::json!({
        "epoch": EPOCH,
        "lanes": [{
            "lane": LANE,
            "occupant": occupant,
            "compaction": compaction,
            "pending": pending,
        }],
    }))
    .expect("hook inbox fixture admits");
    CommandAnswer::Inbox(inbox)
}

fn task_contract_answer() -> CommandAnswer {
    let inbox: InboxAnswer = serde_json::from_value(serde_json::json!({
        "epoch": EPOCH,
        "lanes": [{
            "lane": LANE,
            "occupant": OCCUPANT,
            "compaction": 1,
            "pending": [],
            "task_contract_notices": [{
                "directive": DIRECTIVE,
                "key": KEY,
                "task": "1kKRRsDP",
                "edit": 44,
                "claim": 33,
                "text": "Task contract changed for SPICE-1kKRRsDP.",
                "stage": "published",
                "expires": "2026-08-30T10:00:00.000Z",
            }],
        }],
    }))
    .expect("task-contract hook inbox fixture admits");
    CommandAnswer::Inbox(inbox)
}

fn media_inbox_answer(
    occupant: &str,
    compaction: u64,
    text: &str,
    attachments: &[(&str, &str)],
) -> CommandAnswer {
    let attachments = attachments
        .iter()
        .map(|(name, data_url)| {
            serde_json::json!({
                "name": name,
                "content_type": "image/png",
                "data_url": data_url,
            })
        })
        .collect::<Vec<_>>();
    let inbox: InboxAnswer = serde_json::from_value(serde_json::json!({
        "epoch": EPOCH,
        "lanes": [{
            "lane": LANE,
            "occupant": occupant,
            "compaction": compaction,
            "pending": [{
                "directive": DIRECTIVE,
                "key": "1kMedia1",
                "text": text,
                "attachments": attachments,
                "authored": "2026-08-30T09:00:00.000Z",
                "stage": "published",
                "expires": "2026-08-30T10:00:00.000Z",
            }],
        }],
    }))
    .expect("media hook inbox fixture admits");
    CommandAnswer::Inbox(inbox)
}

fn deliver(scratch: &HookScratch, answer: &CommandAnswer) -> Option<Vec<u8>> {
    hook_document_at(
        answer,
        "claude",
        &scratch.state,
        &scratch.lock,
        &scratch.projections,
    )
    .expect("hook delivery succeeds")
}

fn additional_context(document: &[u8]) -> String {
    let value: serde_json::Value = serde_json::from_slice(document).expect("hook document decodes");
    value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("hook context is text")
        .to_owned()
}

fn media_references(context: &str) -> Vec<AgentMediaReference> {
    context
        .lines()
        .filter_map(|line| line.strip_prefix("media-reference="))
        .map(|encoded| wire::decode(encoded.as_bytes()).expect("hook media reference decodes"))
        .collect()
}

fn media_projections(context: &str) -> Vec<serde_json::Value> {
    context
        .lines()
        .filter_map(|line| line.strip_prefix("media-projection="))
        .map(|encoded| serde_json::from_str(encoded).expect("hook media projection decodes"))
        .collect()
}

fn projection_path_from_document(document: &[u8]) -> PathBuf {
    let mut projections = media_projections(&additional_context(document));
    assert_eq!(
        projections.len(),
        1,
        "fixture publishes one media projection"
    );
    PathBuf::from(
        projections
            .pop()
            .expect("one projection")
            .get("path")
            .and_then(serde_json::Value::as_str)
            .expect("projection has a path"),
    )
}

struct HookScratch {
    root: PathBuf,
    state: PathBuf,
    lock: PathBuf,
    projections: PathBuf,
}

impl HookScratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-hook-delivery-{}-{}",
            std::process::id(),
            NEXT_HOOK_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("locks")).expect("create hook scratch");
        Self {
            state: root.join("steering-delivery.json"),
            lock: root.join("locks/steering-delivery"),
            projections: root.join("steering-media"),
            root,
        }
    }
}

impl Drop for HookScratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove hook scratch");
    }
}
