//! One sender-to-destination card family for room MSG, ASK, and ANS.
//!
//! A card keeps its publication identity through every echo and move. Being
//! recorded in the room, each route's receipt, and a question's substantive
//! result are separate statuses; none of them is review or authorization.

use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, HtmlInputElement, HtmlTextAreaElement};
use wire::intent::{RoomClose, RoomControl, RoomControlAnswer, RoomConversationContent};
use wire::rooms::{
    QuestionClosure, QuestionResultRef, QuestionResultState, ROOM_PENDING_RESULT_LIMIT,
    RoomAuthorityStamp, RoomDelivery, RoomDestination, RoomPublicationContent, RoomPublicationKind,
    RoomReceiptDisposition, RoomReceiptRef, RoomReceiptState, RoomSource, RoomSourceUnavailable,
    TranscriptHunk,
};
use wire::{CanonicalUuid, CommandKind, Disposition};
use yew::prelude::*;

use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, RoomControlRequest};
use crate::runtime::{
    Conversation, ConversationText, Delivery, ItemConcern, OperatorItem, QuestionResult, RoomEvent,
    RoomEventKind,
};

/// What a card's controls act through.
#[derive(Clone, Copy)]
struct Act<'a> {
    runtime: &'a AppRuntime,
    report: &'a Callback<String>,
    sender: &'a SenderHandle,
}

/// One room publication card, or `None` for an event outside this family.
/// Its controls' outcomes and refusals go to `report`.
pub(super) fn render_card(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
) -> Option<Html> {
    let act = Act {
        runtime,
        report,
        sender,
    };
    let projection = runtime.room_projection().conversation();
    let (author, echoed, conversation, packet) = match &event.kind {
        RoomEventKind::Messaged {
            source,
            directed,
            conversation,
            ..
        } => (
            lane_label(runtime, source.head.lane),
            echo(runtime, source),
            conversation
                .as_ref()
                .and_then(|stamp| projection.conversation(stamp)),
            Some(directed.as_slice()),
        ),
        RoomEventKind::Posted => {
            let conversation = projection.conversation(publication)?;
            (
                "Operator".to_owned(),
                conversation.text.clone(),
                Some(conversation),
                None,
            )
        }
        // A plain post is recorded in the room and directs no one.
        RoomEventKind::Published(RoomPublicationContent::Authored { text }) => {
            ("Operator".to_owned(), Some(text.clone()), None, None)
        }
        _ => return None,
    };
    // A lane's bytes stay in its transcript; absent its echo, the text its
    // own deliveries carried, then the resident's read of that transcript,
    // stands in.
    let text = echoed.or_else(|| {
        packet
            .unwrap_or_default()
            .iter()
            .chain(conversation.map_or(&[][..], |conversation| &conversation.directed))
            .find_map(|directive| projection.delivery(directive))
            .map(|delivery| delivery.value.text.clone())
            .or_else(|| {
                conversation.and_then(|conversation| {
                    match runtime.conversation_text(conversation.publication.publication()) {
                        Some(ConversationText::Read {
                            content: RoomConversationContent::Text { text },
                            ..
                        }) => Some(text.clone()),
                        _ => None,
                    }
                })
            })
    });
    let (verb, routes, result) = match conversation {
        Some(conversation) => (
            Some(verb(conversation)),
            routes(act, conversation),
            result(act, conversation),
        ),
        // A packet that admitted no conversation still delivers its directed
        // messages under native steering receipts.
        None => (
            packet.map(|_| "MSG"),
            packet
                .unwrap_or_default()
                .iter()
                .map(|directive| directive_route(runtime, *directive, None, ""))
                .collect(),
            Html::default(),
        ),
    };
    let destinations = routes
        .iter()
        .map(|route| route.destination.clone())
        .collect::<Vec<_>>()
        .join(", ");
    Some(html! {
        <article
            class="room-card room-flow-card"
            key={publication.to_string()}
            data-publication={publication.to_string()}
            data-verb={verb}
            tabindex="0"
        >
            <header>
                <strong class="room-flow-route">
                    {author}
                    if !destinations.is_empty() {
                        {" → "}{destinations}
                    }
                </strong>
                if let Some(verb) = verb {
                    <span class="room-flow-verb">{verb}</span>
                }
                <code>{publication.to_string()}</code>
            </header>
            <p class="room-flow-text">
                {text.unwrap_or_else(|| "Source remains in the author's lane transcript.".to_owned())}
            </p>
            <ul class="room-flow-statuses" aria-label="Delivery status">
                <li class="room-chip" data-status="recorded">{"Recorded in room"}</li>
                {for routes.into_iter().map(|route| route.html)}
            </ul>
            {result}
        </article>
    })
}

/// Operator receipts and answer duties, each with the words it carries and
/// its exact controls, for the catalog. An ACKed ASK remains here until its
/// separate result terminates.
pub(super) fn render_inbox(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
) -> Html {
    let act = Act {
        runtime,
        report,
        sender,
    };
    let projection = runtime.room_projection().conversation();
    let open = projection.inbox_items();
    html! {
        <section class="room-inbox" data-room-inbox="" aria-label="Operator inbox" tabindex="-1">
            <header><strong>{"Operator inbox"}</strong><span>{open.len()}</span></header>
            if open.is_empty() {
                <p class="room-empty">{"No operator receipt or answer awaits action."}</p>
            }
            {for open.into_iter().map(|entry| {
                let publication = entry.item.value.record.publication();
                let conversation = projection.conversation(publication);
                let summary = conversation.map_or_else(String::new, |conversation| {
                    format!(
                        "{} from {}",
                        verb(conversation),
                        party(runtime, conversation.publication.author()),
                    )
                });
                let words = conversation.map_or_else(
                    || "The publication this item stands for is not loaded.".to_owned(),
                    |conversation| inbox_words(runtime, conversation),
                );
                let asked = entry.result.map(|result| {
                    party(runtime, result.value.record.requester())
                });
                let (concern, direction) = match runtime.item_concern(&entry.item.value.record) {
                    ItemConcern::Direction { key, text } => {
                        (format!("About your direction {key}"), text)
                    }
                    ItemConcern::Task(task) => {
                        (format!("About {}", super::anchor_label(runtime, &task)), None)
                    }
                    ItemConcern::Unnamed => ("No direction or task named".to_owned(), None),
                };
                let result = entry.result.map_or_else(Html::default, |result| {
                    question_result(
                        act,
                        result.value.record.result(),
                        &result.value,
                        true,
                    )
                });
                let guide = match &asked {
                    Some(requester) => format!(
                        "Answer sends your reply back to {requester} and settles the receipt; \
                         Decline refuses the question. ACK only marks it seen; NAK also declines it."
                    ),
                    None => "ACK marks this message seen; NAK declines it. Nothing is asked of you.".to_owned(),
                };
                html! {
                    <article
                        class="room-card room-flow-card"
                        key={entry.key.to_string()}
                        data-item={entry.key.to_string()}
                        data-admitted={entry.item.value.admitted.to_string()}
                        tabindex="0"
                    >
                        <header>
                            <strong class="room-flow-route">{summary}</strong>
                            <span>{concern}</span>
                            <code>{publication.to_string()}</code>
                        </header>
                        if let Some(direction) = direction {
                            <blockquote class="room-flow-concern">{direction}</blockquote>
                        }
                        <p class="room-flow-text">{words}</p>
                        {result}
                        <ul class="room-flow-statuses" aria-label="Receipt">
                            {item_status(act, entry.key, &entry.item.value, "You")}
                        </ul>
                        <p class="room-flow-guide">{guide}</p>
                    </article>
                }
            })}
        </section>
    }
}

/// What one inbox publication says: the operator's own text, else the
/// resident's read of the authoring lane's transcript, else that lane's echo
/// while the read runs.
fn inbox_words(runtime: &AppRuntime, conversation: &Conversation) -> String {
    if let Some(text) = &conversation.text {
        return text.clone();
    }
    match runtime.conversation_text(conversation.publication.publication()) {
        Some(ConversationText::Read {
            content: RoomConversationContent::Text { text },
            ..
        }) => text.clone(),
        Some(ConversationText::Read {
            content: RoomConversationContent::Linked { source },
            ..
        }) => format!("Linked source: {source:?}"),
        Some(ConversationText::Read {
            content: RoomConversationContent::Unavailable { unavailable },
            ..
        }) => format!("Text unavailable: {}", unavailable_reason(unavailable)),
        Some(ConversationText::Failed(reason)) => format!("Text could not be read: {reason}"),
        Some(ConversationText::Loading) | None => match conversation.publication.source() {
            RoomSource::Transcript(hunk) => echo(runtime, hunk),
            _ => None,
        }
        .unwrap_or_else(|| "Reading the author's transcript…".to_owned()),
    }
}

fn unavailable_reason(unavailable: &RoomSourceUnavailable) -> String {
    match unavailable {
        RoomSourceUnavailable::Missing {} => "the source is missing".to_owned(),
        RoomSourceUnavailable::Hidden {} => "the source is hidden".to_owned(),
        RoomSourceUnavailable::Changed {} => "the source no longer holds these bytes".to_owned(),
        RoomSourceUnavailable::Unreadable {} => "the source cannot be read now".to_owned(),
        RoomSourceUnavailable::UnsupportedRetained { release } => {
            format!("retained by release {release}, which this release cannot decode")
        }
    }
}

/// One route's destination and its receipt status.
struct Route {
    destination: String,
    html: Html,
}

fn verb(conversation: &Conversation) -> &'static str {
    match conversation.publication.kind() {
        RoomPublicationKind::Msg {} => "MSG",
        RoomPublicationKind::Ask { .. } => "ASK",
        RoomPublicationKind::Ans { .. } => "ANS",
    }
}

fn routes(act: Act<'_>, conversation: &Conversation) -> Vec<Route> {
    let projection = act.runtime.room_projection().conversation();
    if let RoomPublicationKind::Ans { result, .. } = conversation.publication.kind() {
        // An answer's only receipt is its return to the stored requester.
        return match projection
            .result(result)
            .map(|question| &question.value.state)
        {
            Some(QuestionResultState::Answered {
                return_delivery, ..
            }) => vec![delivery_route(
                act,
                return_delivery.destination(),
                return_delivery.delivery(),
            )],
            _ => Vec::new(),
        };
    }
    let mut directed = conversation.directed.iter();
    conversation
        .publication
        .routes()
        .iter()
        .map(|route| match &route.destination {
            RoomDestination::Lane { lane } => match directed.next() {
                Some(directive) => directive_route(act.runtime, *directive, Some(*lane), ""),
                None => unavailable_route(lane_label(act.runtime, *lane)),
            },
            RoomDestination::Operator {} => match conversation.publication.operator_item() {
                Some((item, _)) => item_route(act, &item, "Operator", ""),
                None => unavailable_route("Operator".to_owned()),
            },
            RoomDestination::Task { task } => {
                let destination = format!("#{}", super::anchor_label(act.runtime, task));
                Route {
                    html: html! {
                        <li class="room-chip" data-status="room">
                            {format!("{destination}: recorded there; no directed recipient")}
                        </li>
                    },
                    destination,
                }
            }
        })
        .collect()
}

/// An answer's return, receipted like any delivery but named as the return.
fn delivery_route(act: Act<'_>, destination: &RoomDestination, delivery: &RoomDelivery) -> Route {
    const LEAD: &str = "Return to ";
    match delivery {
        RoomDelivery::Directive { directive } => {
            let lane = match destination {
                RoomDestination::Lane { lane } => Some(*lane),
                _ => None,
            };
            directive_route(act.runtime, *directive, lane, LEAD)
        }
        RoomDelivery::Item { item } => item_route(act, item, "Operator", LEAD),
    }
}

fn unavailable_route(destination: String) -> Route {
    Route {
        html: html! {
            <li class="room-chip" data-status="unavailable">
                {format!("{destination}: receipt unavailable")}
            </li>
        },
        destination,
    }
}

/// A lane receipt is the native steering directive delivering it.
fn directive_route(
    runtime: &AppRuntime,
    directive: CanonicalUuid,
    lane: Option<CanonicalUuid>,
    lead: &str,
) -> Route {
    let delivery = runtime
        .room_projection()
        .conversation()
        .delivery(&directive)
        .map(|delivery| &delivery.value);
    let Some(lane) = lane.or_else(|| delivery.map(|delivery| delivery.lane)) else {
        return unavailable_route("Lane".to_owned());
    };
    let destination = lane_label(runtime, lane);
    let Some(delivery) = delivery else {
        return unavailable_route(destination);
    };
    let (status, label) = delivery_status(runtime, delivery);
    Route {
        html: {
            let text = format!("{lead}{destination}: {label}");
            html! {
                <li class="room-chip" data-status={status} data-receipt-key={delivery.key.clone()} title={text.clone()}>
                    {text}
                </li>
            }
        },
        destination,
    }
}

fn delivery_status(runtime: &AppRuntime, delivery: &Delivery) -> (&'static str, &'static str) {
    let member = runtime
        .lanes()
        .and_then(|lanes| lanes.member(delivery.lane));
    match delivery.disposition {
        Disposition::Live if delivery.queued => ("unheard", "queued for vacant lane · unheard"),
        Disposition::Live if member.is_none() => {
            ("unavailable", "lane unavailable · receipt pending")
        }
        Disposition::Live if member.is_some_and(|member| member.occupant.is_none()) => {
            ("vacant", "lane vacant · receipt pending")
        }
        Disposition::Live => ("pending", "receipt pending"),
        Disposition::Acked => ("acked", "acked"),
        Disposition::Declined => ("declined", "declined"),
        Disposition::Lapsed => ("expired", "receipt expired"),
        Disposition::DeadLettered => ("unavailable", "undeliverable · unavailable"),
        Disposition::Canceled => ("cancelled", "cancelled"),
    }
}

fn item_route(act: Act<'_>, key: &RoomReceiptRef, destination: &str, lead: &str) -> Route {
    let html = match act.runtime.room_projection().conversation().item(key) {
        Some(item) => item_status(act, key, &item.value, &format!("{lead}{destination}")),
        None => return unavailable_route(destination.to_owned()),
    };
    Route {
        destination: destination.to_owned(),
        html,
    }
}

/// The operator's own receipt, with ACK/NAK while pending and recovery once
/// lapsed. Its control slot keeps one height in every state, so a receipt
/// settling never moves the cards after it.
fn item_status(act: Act<'_>, key: &RoomReceiptRef, item: &OperatorItem, destination: &str) -> Html {
    let (status, label) = match &item.state {
        RoomReceiptState::Pending { expires } => {
            ("pending", format!("receipt pending · expires {expires}"))
        }
        RoomReceiptState::Acked { .. } => ("acked", "acked".to_owned()),
        RoomReceiptState::Declined { .. } => ("declined", "declined".to_owned()),
        RoomReceiptState::Expired { at } => ("expired", format!("receipt expired {at}")),
        RoomReceiptState::Cancelled { .. } => ("cancelled", "cancelled".to_owned()),
    };
    let control = match &item.state {
        RoomReceiptState::Pending { .. } => dispose_controls(act, key),
        RoomReceiptState::Expired { .. } => recover_control(act, key),
        _ => Html::default(),
    };
    let text = format!("{destination}: {label}");
    html! {
        <li class="room-chip" data-status={status} data-item-key={key.to_string()}>
            <span title={text.clone()}>{text}</span>
            <div class="room-flow-slot">{control}</div>
        </li>
    }
}

/// A question's substantive result, separate from every receipt.
fn result(act: Act<'_>, conversation: &Conversation) -> Html {
    let projection = act.runtime.room_projection().conversation();
    let (result, lead) = match conversation.publication.kind() {
        RoomPublicationKind::Ask { result, .. } => (result, "Result"),
        RoomPublicationKind::Ans { result, .. } => (result, "Answers"),
        RoomPublicationKind::Msg {} => return Html::default(),
    };
    let Some(question) = projection.result(result) else {
        return html! {
            <p class="room-flow-result" data-result-state="unavailable">
                {format!("{lead} {result}: unavailable")}
            </p>
        };
    };
    let question = &question.value;
    if lead == "Answers" {
        return html! {
            <p class="room-flow-result" data-result-state="answer">
                {format!(
                    "Answers {result} from {}",
                    party(act.runtime, question.record.requester()),
                )}
            </p>
        };
    }
    question_result(act, result, question, false)
}

/// A question's result line and controls. The inbox leads with the answer
/// owed; a room card also shows the requester's pending-result load.
fn question_result(
    act: Act<'_>,
    result: &QuestionResultRef,
    question: &QuestionResult,
    inbox: bool,
) -> Html {
    let projection = act.runtime.room_projection().conversation();
    let state = question.state.name();
    let requester = party(act.runtime, question.record.requester());
    let label = match &question.state {
        QuestionResultState::Pending {} if inbox => format!(
            "{requester} awaits your answer until {}",
            question.record.deadline(),
        ),
        QuestionResultState::Pending {} => format!(
            "pending until {} · {requester} holds {} of {ROOM_PENDING_RESULT_LIMIT} pending results",
            question.record.deadline(),
            projection.pending_results(question.record.requester()),
        ),
        QuestionResultState::Answered { answer, .. } => format!("answered by {answer}"),
        QuestionResultState::Declined { .. } => "declined".to_owned(),
        QuestionResultState::Cancelled { .. } => "cancelled".to_owned(),
        QuestionResultState::TimedOut {} => "timed out".to_owned(),
    };
    let controls = if question.state.is_terminal() {
        Html::default()
    } else {
        result_controls(act, result, question)
    };
    html! {
        <div class="room-flow-result" data-result-state={state}>
            <p>{format!("Result {result}: {label}")}</p>
            <div class="room-flow-slot room-flow-slot--result">{controls}</div>
        </div>
    }
}

fn dispose_controls(act: Act<'_>, key: &RoomReceiptRef) -> Html {
    let clause = NodeRef::default();
    let enabled = act.runtime.offers(CommandKind::RoomControl);
    let dispose = |disposition| {
        let act = Owned::from(act);
        let clause = clause.clone();
        let key = key.clone();
        move |event: &Event| {
            act.send(
                event,
                RoomControlRequest::Control(RoomControl::Dispose {
                    disposition,
                    keys: vec![key.clone()],
                    clause: input_value(&clause),
                }),
            );
        }
    };
    let ack = dispose(RoomReceiptDisposition::Ack);
    let nak = dispose(RoomReceiptDisposition::Nak);
    html! {
        <form class="room-flow-control" onsubmit={Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            ack(&event);
        })}>
            <input ref={clause} aria-label="Receipt clause" placeholder="Clause" disabled={!enabled} />
            <button type="submit" disabled={!enabled}>{"ACK"}</button>
            <button type="button" disabled={!enabled} onclick={Callback::from(move |event: MouseEvent| nak(&event))}>{"NAK"}</button>
        </form>
    }
}

fn recover_control(act: Act<'_>, key: &RoomReceiptRef) -> Html {
    let enabled = act.runtime.offers(CommandKind::RoomControl);
    let act = Owned::from(act);
    let key = key.clone();
    let onclick = Callback::from(move |event: MouseEvent| {
        act.send(
            &event,
            RoomControlRequest::Control(RoomControl::Recover { item: key.clone() }),
        );
    });
    html! {
        <button type="button" class="room-flow-control" disabled={!enabled} {onclick}>{"Recover"}</button>
    }
}

/// The operator answers or declines what it was asked, and may close a lane
/// party's question only explicitly on that party's behalf.
fn result_controls(act: Act<'_>, result: &QuestionResultRef, question: &QuestionResult) -> Html {
    let operator = RoomDestination::Operator {};
    let respondent = *question.record.respondent() == operator;
    let requester = *question.record.requester() == operator;
    let answerable = respondent && act.runtime.offers(CommandKind::RoomControl);
    let closable = act.runtime.offers(CommandKind::RoomClose);
    let text = NodeRef::default();
    let close = |closure, override_party| {
        let act = Owned::from(act);
        let text = text.clone();
        let result = result.clone();
        Callback::from(move |event: MouseEvent| {
            act.send(
                &event,
                RoomControlRequest::Close(RoomClose {
                    result: result.clone(),
                    closure,
                    clause: input_value(&text),
                    override_party,
                }),
            );
        })
    };
    let decline = close(QuestionClosure::Declined, !respondent);
    let cancel = close(QuestionClosure::Cancelled, !requester);
    let onsubmit = {
        let act = Owned::from(act);
        let text = text.clone();
        let result = result.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            act.send(
                &event,
                RoomControlRequest::Control(RoomControl::Answer {
                    result: result.clone(),
                    context: None,
                    text: input_value(&text),
                }),
            );
        })
    };
    let decline_label = if respondent {
        "Decline"
    } else {
        "Decline for respondent"
    };
    let cancel_label = if requester {
        "Cancel"
    } else {
        "Cancel for requester"
    };
    let prompt = if respondent {
        format!(
            "Answer to {}, or a decline clause",
            party(act.runtime, question.record.requester())
        )
    } else {
        "Clause".to_owned()
    };
    html! {
        <form class="room-flow-control" {onsubmit}>
            <textarea
                ref={text}
                aria-label={prompt.clone()}
                placeholder={prompt}
                disabled={!(answerable || closable)}
            />
            if respondent {
                <button type="submit" disabled={!answerable}>{"Answer"}</button>
            }
            <button type="button" disabled={!closable} onclick={decline}>{decline_label}</button>
            <button type="button" disabled={!closable} onclick={cancel}>{cancel_label}</button>
        </form>
    }
}

/// Control context a callback can own.
#[derive(Clone)]
struct Owned {
    report: Callback<String>,
    sender: SenderHandle,
}

impl From<Act<'_>> for Owned {
    fn from(act: Act<'_>) -> Self {
        Self {
            report: act.report.clone(),
            sender: act.sender.clone(),
        }
    }
}

impl Owned {
    /// Send one control and report its outcome where its surface reports.
    /// Keyboard focus moves to the acting card first, so the control
    /// retiring never strands it.
    fn send(&self, event: &Event, request: RoomControlRequest) {
        if let Some(card) = acting_card(event) {
            let _ = card.focus();
        }
        let report = self.report.clone();
        let respond = Rc::new(move |answer: Result<RoomControlAnswer, String>| {
            report.emit(match answer {
                Ok(answer) => control_notice(&answer),
                Err(error) => error,
            });
        });
        let _ = admit_action(&self.sender, ClientAction::RoomControl { request, respond });
    }
}

fn control_notice(answer: &RoomControlAnswer) -> String {
    let mut moves = answer
        .items
        .iter()
        .map(|moved| format!("item {} {}", moved.item, moved.state))
        .chain(
            answer
                .results
                .iter()
                .map(|moved| format!("result {} {}", moved.result, moved.state)),
        )
        .collect::<Vec<_>>();
    if let Some(answer) = &answer.answer {
        moves.push(format!("answer {answer} recorded"));
    }
    if moves.is_empty() {
        "Room control recorded.".to_owned()
    } else {
        format!("Room control recorded: {}.", moves.join("; "))
    }
}

/// The card a control acts on, or the inbox holding it, since a settled
/// item leaves the inbox.
fn acting_card(event: &Event) -> Option<HtmlElement> {
    let target = event.target()?.dyn_into::<Element>().ok()?;
    let inbox = target.closest("[data-room-inbox]").ok().flatten();
    inbox
        .or_else(|| target.closest(".room-flow-card").ok().flatten())?
        .dyn_into::<HtmlElement>()
        .ok()
}

fn input_value(node: &NodeRef) -> String {
    node.cast::<HtmlInputElement>()
        .map(|input| input.value())
        .or_else(|| {
            node.cast::<HtmlTextAreaElement>()
                .map(|input| input.value())
        })
        .unwrap_or_default()
}

fn party(runtime: &AppRuntime, destination: &RoomDestination) -> String {
    match destination {
        RoomDestination::Operator {} => "Operator".to_owned(),
        RoomDestination::Lane { lane } => lane_label(runtime, *lane),
        RoomDestination::Task { task } => format!("#{}", super::anchor_label(runtime, task)),
    }
}

fn lane_label(runtime: &AppRuntime, lane: CanonicalUuid) -> String {
    runtime
        .lanes()
        .and_then(|lanes| lanes.member(lane))
        .map_or_else(
            || format!("lane {}", &lane.to_string()[..8]),
            |member| {
                crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
                    .to_owned()
            },
        )
}

/// The exact contributed bytes when the author's transcript echo is loaded.
fn echo(runtime: &AppRuntime, hunk: &TranscriptHunk) -> Option<String> {
    let start = usize::try_from(hunk.text.start).ok()?;
    let end = usize::try_from(hunk.text.end).ok()?;
    runtime
        .messages()
        .iter()
        .find(|message| message.lane == hunk.head.lane && message.locus == hunk.locus)
        .and_then(|message| message.text.get(start..end))
        .map(str::to_owned)
}
