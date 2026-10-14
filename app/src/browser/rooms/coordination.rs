//! A design menu's room coordination controls. A room post only records, so
//! these are how the operator asks a lane for a contribution: Start or Resume
//! names one occupied seat by address with the criterion, contribution, stop
//! condition, and optional auto-resume limit the resident's
//! `room::coordinate` admits, and Put aside cancels unanswered coordination
//! and asks a claim holder to checkpoint without freeing its claim. Outcomes
//! and refusals go to the design composer's report.

use std::rc::Rc;

use wire::intent::{
    DesignStanding, RoomCoordinate, RoomCoordinateAction, RoomCoordinationView, RoomPutAside,
};
use wire::{CommandKind, TaskHandle, TaskStamp, Witness};
use yew::prelude::*;

use super::{input_value, transition_witnesses};
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, RoomCoordinationRequest};
use crate::runtime::Seats;

/// The fields one Start or Resume reads.
#[derive(Clone, Default)]
struct Inputs {
    target: NodeRef,
    criterion: NodeRef,
    contribution: NodeRef,
    stop: NodeRef,
    auto_resume: NodeRef,
}

/// Everything one design's coordination buttons submit against: the room,
/// its anchor heads, and the seats at the board snapshot they were drawn from.
#[derive(Clone)]
struct Coordination {
    report: Callback<String>,
    sender: SenderHandle,
    room: TaskHandle,
    anchor: Vec<Witness>,
    seats: Seats,
    inputs: Inputs,
}

/// One active design's coordination controls. A passive design names how to
/// activate it instead; a concluded one, or one off the board, shows nothing.
pub(super) fn coordination_form(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
    room: &TaskStamp,
) -> Html {
    let Some(board) = runtime.board() else {
        return Html::default();
    };
    let Some(anchor) = board.row(room) else {
        return Html::default();
    };
    match anchor.design_standing() {
        Some(DesignStanding::Active) => {}
        Some(DesignStanding::Passive) => {
            return html! {
                <p class="room-notice" data-room-coordination-passive={room.to_string()}>
                    {"Passive: no lane can claim this Design or be coordinated onto it. Move it into a working group to activate it."}
                </p>
            };
        }
        _ => return Html::default(),
    }
    let coordination = Coordination {
        report: report.clone(),
        sender: sender.clone(),
        room: anchor.statement.body.handle.clone(),
        anchor: transition_witnesses(anchor),
        seats: board.seats.clone(),
        inputs: Inputs::default(),
    };
    let inputs = coordination.inputs.clone();
    let datalist = format!("room-coordination-seats-{room}");
    let coordinates = runtime.offers(CommandKind::RoomCoordinate);
    let lanes = runtime.lanes();
    html! {
        <form
            class="room-coordination"
            data-room-coordination={room.to_string()}
            onsubmit={Callback::from(|event: SubmitEvent| event.prevent_default())}
        >
            <strong>{"Coordinate"}</strong>
            <p class="room-notice">
                {"Ask one seated lane for a bounded contribution. It receives this as operator direction."}
            </p>
            <label>
                <span>{"Lane address"}</span>
                <input ref={inputs.target} type="text" list={datalist.clone()} placeholder="6JRT" />
            </label>
            <datalist id={datalist}>
                {for board.seats.occupied().map(|(lane, address)| {
                    let name = lanes
                        .and_then(|lanes| lanes.member(lane))
                        .and_then(|member| member.name.clone());
                    html! { <option value={address.to_string()}>{name.unwrap_or_default()}</option> }
                })}
            </datalist>
            <label><span>{"Criterion"}</span><input ref={inputs.criterion} type="text" /></label>
            <label><span>{"Contribution"}</span><input ref={inputs.contribution} type="text" /></label>
            <label><span>{"Stop when"}</span><input ref={inputs.stop} type="text" /></label>
            <label>
                <span>{"Auto-resume"}</span>
                <input
                    ref={inputs.auto_resume}
                    type="number"
                    min="1"
                    max={wire::rooms::ROOM_RESUME_LIMIT.to_string()}
                    placeholder="off"
                />
            </label>
            <div class="room-inline-actions">
                <button type="button" disabled={!coordinates} onclick={coordination.coordinate(RoomCoordinateAction::Start)}>
                    {"Start"}
                </button>
                <button type="button" disabled={!coordinates} onclick={coordination.coordinate(RoomCoordinateAction::Resume)}>
                    {"Resume"}
                </button>
                <button
                    type="button"
                    disabled={!runtime.offers(CommandKind::RoomPutAside)}
                    onclick={coordination.put_aside()}
                >
                    {"Put aside"}
                </button>
            </div>
        </form>
    }
}

impl Coordination {
    fn coordinate(&self, action: RoomCoordinateAction) -> Callback<MouseEvent> {
        let coordination = self.clone();
        Callback::from(move |_: MouseEvent| match coordination.request(action) {
            Ok(request) => coordination.send(request),
            Err(refusal) => coordination.report.emit(refusal),
        })
    }

    fn put_aside(&self) -> Callback<MouseEvent> {
        let coordination = self.clone();
        Callback::from(move |_: MouseEvent| {
            coordination.send(RoomCoordinationRequest::PutAside {
                request: RoomPutAside {
                    room: coordination.room.clone(),
                },
                expected: coordination.anchor.clone(),
            });
        })
    }

    /// The Start or Resume the fields describe, or the refusal the resident
    /// would answer for an address naming no occupied seat or an
    /// out-of-range auto-resume limit, so nothing is submitted.
    fn request(&self, action: RoomCoordinateAction) -> Result<RoomCoordinationRequest, String> {
        let field = |node: &NodeRef| {
            input_value(node).map_or_else(String::new, |value| value.trim().to_owned())
        };
        let target = field(&self.inputs.target)
            .parse::<wire::OccupantAddress>()
            .map_err(|error| format!("room::address-invalid: {error}"))?;
        let seat = self
            .seats
            .witness(target)
            .map_err(|refusal| format!("{}: {}", refusal.reason(), refusal.explanation(target)))?;
        let auto_resume = Some(field(&self.inputs.auto_resume))
            .filter(|written| !written.is_empty())
            .map(|written| written.parse::<u8>())
            .transpose()
            .map_err(|_| {
                format!(
                    "room::auto-resume-out-of-range: authorize from 1 through {} automated resumes",
                    wire::rooms::ROOM_RESUME_LIMIT
                )
            })?;
        let mut expected = self.anchor.clone();
        expected.push(seat);
        Ok(RoomCoordinationRequest::Coordinate {
            request: RoomCoordinate {
                room: self.room.clone(),
                target,
                action,
                criterion: field(&self.inputs.criterion),
                contribution: field(&self.inputs.contribution),
                stop: field(&self.inputs.stop),
                auto_resume,
            },
            expected,
        })
    }

    fn send(&self, request: RoomCoordinationRequest) {
        let report = self.report.clone();
        let respond = Rc::new(move |answer: Result<RoomCoordinationView, String>| {
            report.emit(answer.map_or_else(|error| error, |view| coordination_notice(&view)));
        });
        let _ = admit_action(
            &self.sender,
            ClientAction::RoomCoordination { request, respond },
        );
    }
}

/// What one coordination answer leaves standing in the room.
fn coordination_notice(view: &RoomCoordinationView) -> String {
    let mut notice = match (view.requests.first(), &view.stop) {
        (Some(request), _) => format!(
            "Asked {} for the next contribution as request {}.",
            request.target, request.key
        ),
        (None, Some(stop)) => format!(
            "Put aside. Asked {} to checkpoint and free claim {} as request {}; the claim stays held until it does.",
            stop.target, stop.claim, stop.key
        ),
        (None, None) => "Put aside. No coordination remains unanswered.".to_owned(),
    };
    if let Some(rule) = view.rule {
        notice.push_str(&format!(
            " Automation may resume it {} more time(s).",
            rule.limit.saturating_sub(rule.spent)
        ));
    }
    notice
}
