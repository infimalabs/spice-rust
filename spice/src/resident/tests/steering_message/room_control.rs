//! Operator controls settle only the live operator items the operator owes,
//! and room reads name each item and decision author apart without changing
//! any of them.

use serde_json::Value;
use wire::intent::RoomControl;
use wire::rooms::{RoomAuthorityStamp, RoomReceiptDisposition, RoomReceiptRef, RoomReceiptState};

use super::*;
use crate::resident::timers::{Fired, RoomDue};

#[path = "packet_context.rs"]
mod packet_context;

impl Lanes {
    /// Admit `body` as `actor` under the current principal roles, as the
    /// resident's command door does.
    fn admitted(
        &self,
        id: u8,
        actor: Actor,
        kind: CommandKind,
        body: &impl serde::Serialize,
    ) -> wire::AdmittedCommand {
        let roles = self
            .bus
            .read(|read| read.principal_roles())
            .expect("principal roles read");
        Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind,
            expected: Vec::new(),
            body: serde_json::to_value(body).expect("control encodes"),
        }
        .admit(
            &wire::Accepted {
                wire: wire::WireVersion::CURRENT,
                actor,
                epoch: self.epoch,
                release: "1.2.3".parse().expect("fixture release"),
            },
            &roles,
        )
        .expect("control admits")
    }

    fn operate(&self, id: u8, control: RoomControl) -> Outcome<CommandAnswer> {
        self.operate_as(id, Actor::Operator, control)
    }

    fn operate_as(&self, id: u8, actor: Actor, control: RoomControl) -> Outcome<CommandAnswer> {
        let command = self.admitted(id, actor, CommandKind::RoomControl, &control);
        crate::board_command::room_control(&self.bus, &command, &control, &steer::Config::default())
            .0
    }

    fn dispose(
        &self,
        id: u8,
        disposition: RoomReceiptDisposition,
        keys: &[RoomReceiptRef],
        clause: &str,
    ) -> Outcome<CommandAnswer> {
        self.operate(
            id,
            RoomControl::Dispose {
                disposition,
                keys: keys.to_vec(),
                clause: clause.to_owned(),
            },
        )
    }

    /// `spice room WORDS...` read in process, as its JSON reply body.
    pub(super) fn room_read(&self, words: &[&str]) -> Value {
        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "room"].iter().chain(words))
            .expect("room read parses");
        let contract = crate::cli::selected_contract(&matches).expect("room read is registered");
        let leaf = crate::cli::selected_matches(&matches);
        let journal =
            journal::JournalReader::open(&self.paths.journal).expect("the journal opens to read");
        let body = match contract.path {
            ["room", "inbox"] => match crate::room_command::inbox::answer(
                &journal,
                &self.paths.tape,
                leaf,
                contract.path,
            ) {
                Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                other => panic!("room inbox answered {other:?}"),
            },
            ["room", path @ ("relevance" | "awareness")] => {
                let projects =
                    board::ProjectPolicy::new(Vec::new(), 2, 3).expect("fixture project policy");
                if *path == "relevance" {
                    match crate::room_command::relevance::answer(
                        &journal,
                        &projects,
                        leaf,
                        contract.path,
                    ) {
                        Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                        other => panic!("room relevance answered {other:?}"),
                    }
                } else {
                    match crate::room_command::awareness::answer(
                        &journal,
                        &projects,
                        leaf,
                        contract.path,
                    ) {
                        Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                        other => panic!("room awareness answered {other:?}"),
                    }
                }
            }
            other => panic!("not a room read: {other:?}"),
        };
        body.expect("the read encodes")
    }

    /// Every open operator item key, following each inbox page's
    /// continuation from `words`.
    fn inbox_items(&self, words: &[&str]) -> Vec<(String, String)> {
        let mut words = words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>();
        let mut items = Vec::new();
        for _ in 0..8 {
            let read = ["inbox"]
                .into_iter()
                .chain(words.iter().map(String::as_str))
                .collect::<Vec<_>>();
            let page = self.room_read(&read);
            items.extend(
                page["page"]["rows"]
                    .as_array()
                    .expect("rows")
                    .iter()
                    .map(|row| {
                        (
                            row["receipt"]["item"]
                                .as_str()
                                .expect("item key")
                                .to_owned(),
                            row["verb"].as_str().expect("verb").to_owned(),
                        )
                    }),
            );
            let continuation = &page["page"]["omission"]["continuation"];
            if continuation.is_null() {
                return items;
            }
            words = serde_json::from_value::<Vec<String>>(continuation.clone())
                .expect("a continuation is one argument list")
                .split_off(2);
        }
        panic!("paging did not finish: {items:?}");
    }

    /// Record `principal`'s birth holding the operator role.
    fn admit_operator(&self, principal: CanonicalUuid) {
        let roles = std::collections::BTreeSet::from([wire::principal::PrincipalRole::Operator]);
        let mut write =
            write(Actor::Spice, self.epoch, Producer::CommandDoor).expect("admission write starts");
        let born = write
            .fact(
                wire::principal::PrincipalAdmitted {
                    principal,
                    profile: wire::principal::PrincipalProfile::default(),
                    roles: roles.clone(),
                    capability: None,
                },
                FactCause::Root,
            )
            .expect("principal birth fact builds");
        write.principal_head(born, move |born| wire::principal::PrincipalHead {
            principal,
            born,
            profile: wire::principal::PrincipalProfile::default(),
            roles,
            capability: None,
        });
        commit(&self.bus, write).expect("the birth commits");
    }

    /// The principal whose inbox `publication`'s operator item reaches, as
    /// the resident's conversation text read answers it.
    fn recipient(&self, publication: RoomAuthorityStamp) -> Option<Actor> {
        match crate::room_command::conversation_text(
            &self.bus,
            &self.paths.tape,
            &wire::intent::RoomConversationText { publication },
        ) {
            Outcome::Reply(Reply {
                body: CommandAnswer::RoomConversationText(answer),
                ..
            }) => answer.recipient,
            other => panic!("the conversation text read answered {other:?}"),
        }
    }
}

fn reason(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("the control refused: {other:?}"),
    }
}

fn committed(outcome: &Outcome<CommandAnswer>) {
    assert!(
        matches!(
            outcome,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                ..
            })
        ),
        "the control committed: {outcome:?}"
    );
}

/// A reply returns once to the lane whose MSG opened the item, as operator
/// direction citing the direction it concerned, and settles that item.
#[test]
fn the_operator_replies_to_a_lane_item_as_direction() {
    let lanes = Lanes::new("control-reply");
    let room = lanes.room("dVrD244Jd");
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key} #{room}: shipped"))
        .expect("the room MSG publishes");
    let told = lanes
        .inbox_items(&[])
        .into_iter()
        .find(|(_, verb)| verb == "msg")
        .map(|(item, _)| RoomReceiptRef::new(item.parse().expect("item key")))
        .expect("the MSG opened an item");
    let reply = |id, item: &RoomReceiptRef, text: &str| {
        lanes.operate(
            id,
            RoomControl::Reply {
                item: item.clone(),
                text: text.to_owned(),
            },
        )
    };

    assert_eq!(reason(&reply(81, &told, "  ")), "room::reply-empty");
    committed(&reply(82, &told, "merge it"));
    assert!(matches!(
        lanes.item_state(&told),
        RoomReceiptState::Acked { .. }
    ));
    let returned = lanes
        .pending(SENDER)
        .into_iter()
        .filter(|directive| directive.text == format!("RE #{key}: merge it"))
        .count();
    assert_eq!(returned, 1, "the reply returns once to the lane");
    let history = lanes.room_page(&[room.as_str()]);
    let texts = history["page"]["rows"]
        .as_array()
        .expect("room history rows")
        .iter()
        .map(|row| row["content"]["text"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [Some("shipped"), Some("merge it")],
        "the reply records in the room its item names"
    );
    assert_eq!(reason(&reply(83, &told, "again")), "room::item-settled");
    lanes.finish();
}

/// The inbox pages every open operator item whole and resumably; an ACK
/// settles an item without a reply, and settled items leave the inbox.
#[test]
fn operator_items_page_and_settle_exactly() {
    let lanes = Lanes::new("control-items");
    let room = lanes.room("dVrD244Jd");
    let key = lanes.direct(SENDER, 20, "ship when green");
    for text in ["shipped", "and then?", "anything else?"] {
        lanes
            .say(SENDER, &format!("MSG #{key}: {text}"))
            .expect("the MSG publishes");
    }

    let items = lanes.inbox_items(&["--limit", "1"]);
    assert_eq!(
        items
            .iter()
            .map(|(_, verb)| verb.as_str())
            .collect::<Vec<_>>(),
        ["msg", "msg", "msg"],
        "single-row pages resume at exactly the withheld item"
    );
    assert_eq!(items, lanes.inbox_items(&[]));
    let told = items
        .iter()
        .map(|(item, _)| RoomReceiptRef::new(item.parse().expect("item key")))
        .collect::<Vec<_>>();
    assert_eq!(lanes.inbox_items(&[]), items, "reading settles nothing");

    let unknown = RoomReceiptRef::new(lanes.post(&room, 1, "aside"));
    assert_eq!(
        reason(&lanes.dispose(73, RoomReceiptDisposition::Ack, &[unknown], "seen")),
        "room::item-unknown"
    );
    let directions = lanes.pending(SENDER).len();
    committed(&lanes.dispose(74, RoomReceiptDisposition::Ack, &told[..1], ""));
    committed(&lanes.dispose(76, RoomReceiptDisposition::Ack, &told[1..], "seen"));
    for item in &told {
        assert!(matches!(
            lanes.item_state(item),
            RoomReceiptState::Acked { .. }
        ));
    }
    assert_eq!(
        lanes.pending(SENDER).len(),
        directions,
        "settling the item sends no reply"
    );
    assert!(lanes.inbox_items(&[]).is_empty());
    assert_eq!(
        reason(&lanes.dispose(77, RoomReceiptDisposition::Ack, &told[..1], "again")),
        "room::item-settled"
    );
    lanes.finish();
}

/// Each operator item reaches only the principal that sent the direction a
/// lane's `#KEY` names: a historical operator direction reaches the
/// bootstrap principal, whom the local inbox reads as, and a second
/// operator-role principal's direction reaches only that principal.
#[test]
fn operator_items_reach_only_the_directing_principal() {
    let lanes = Lanes::new("control-recipients");
    let second = test_uuid(90);
    lanes.admit_operator(second);
    let historical = lanes.direct(SENDER, 20, "ship when green");
    let theirs = lanes.direct_as(Actor::Human(second), SENDER, 30, "and the docs");
    lanes
        .say(SENDER, &format!("MSG #{historical}: shipped"))
        .expect("the reply publishes");
    lanes
        .say(SENDER, &format!("MSG #{theirs}: which docs?"))
        .expect("the concern publishes");

    let items = lanes.inbox_items(&[]);
    assert_eq!(
        items
            .iter()
            .map(|(_, verb)| verb.as_str())
            .collect::<Vec<_>>(),
        ["msg"],
        "the local inbox holds only the bootstrap principal's item"
    );
    let reply: RoomAuthorityStamp = items[0].0.parse().expect("item key");
    let concern = lanes
        .conversations()
        .into_iter()
        .find(|record| {
            record
                .routes
                .iter()
                .any(|route| route.direction.as_deref() == Some(theirs.as_str()))
        })
        .expect("the concern recorded its conversation")
        .publication;
    assert_eq!(lanes.recipient(reply), Some(lanes.bootstrap()));
    assert_eq!(lanes.recipient(concern), Some(Actor::Human(second)));
    lanes.finish();
}

/// Settling a room's anchor closes it to new posts only: an operator item
/// already open in it still settles.
#[test]
fn open_room_duties_still_settle_after_the_anchor_settles() {
    let lanes = Lanes::new("control-settled-anchor");
    let room = lanes.room("dVrD244Jd");
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key} #{room}: shipped"))
        .expect("the MSG publishes");
    let items = lanes.inbox_items(&[]);
    let told = RoomReceiptRef::new(items[0].0.parse().expect("item key"));
    lanes.close_room(&room);
    let born = room.born().expect("fixture handle carries its stamp");
    let anchor = lanes
        .bus
        .read(move |read| read.workflow_head(&born))
        .expect("anchor workflow reads")
        .expect("anchor exists");
    assert!(
        matches!(
            anchor.body().status.conclusion(),
            Some(wire::WorkflowConclusion::Negative(
                wire::WorkflowNegative::Closed
            ))
        ),
        "the anchor is settled before its open duties settle"
    );

    committed(&lanes.dispose(
        62,
        RoomReceiptDisposition::Ack,
        std::slice::from_ref(&told),
        "seen",
    ));
    assert!(matches!(
        lanes.item_state(&told),
        RoomReceiptState::Acked { .. }
    ));
    lanes.finish();
}

/// A lapsed operator item stays open but refuses settlement until the
/// operator recovers it to pending with a fresh expiry.
#[test]
fn lapsed_items_refuse_settlement_until_recovered() {
    let lanes = Lanes::new("control-lapse");
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key}: ship it?"))
        .expect("the MSG publishes");
    let [conversed] = lanes
        .conversations()
        .try_into()
        .expect("one conversation record");
    let item = RoomReceiptRef::new(conversed.publication);
    let recover = |id| lanes.operate(id, RoomControl::Recover { item: item.clone() });
    let mut timers = lanes.timers();
    let RoomReceiptState::Pending { expires } = lanes.item_state(&item) else {
        panic!("the operator item opens pending");
    };
    assert_eq!(reason(&recover(80)), "room::item-live");
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &RoomDue::Item(item.clone()), expires),
        Ok(Fired::Room)
    );
    assert_eq!(
        reason(&lanes.dispose(
            81,
            RoomReceiptDisposition::Ack,
            std::slice::from_ref(&item),
            "seen"
        )),
        "room::item-lapsed"
    );
    assert_eq!(
        lanes.inbox_items(&[]),
        [(item.to_string(), "msg".to_owned())],
        "a lapsed item still owes a disposition"
    );

    committed(&recover(82));
    let RoomReceiptState::Pending { expires: renewed } = lanes.item_state(&item) else {
        panic!("recovery returns the item to pending");
    };
    assert!(renewed > expires, "recovery re-arms a later expiry");
    assert_eq!(reason(&recover(83)), "room::item-live");
    committed(&lanes.dispose(
        84,
        RoomReceiptDisposition::Ack,
        std::slice::from_ref(&item),
        "seen",
    ));
    drop(timers);
    lanes.finish();
}
