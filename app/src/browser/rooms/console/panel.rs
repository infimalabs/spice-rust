//! A working group's control panel: the team panel's frame and geometry,
//! holding the group's TURBO, its arrangement menu, its speech and stance
//! sliders, and one send control for every drafted design composer.

use wasm_bindgen::JsCast;
use wire::TaskStamp;
use wire::rooms::RoomAuthorityStamp;
use yew::prelude::*;

use super::super::Controls;
use super::GroupRender;
use crate::browser::watchband::{ControlsFrame, GroupPanel, SendSlot, holds_text};
use crate::room_views::GroupView;

/// One open design composer as the group's post control sees it.
pub(super) struct Postable {
    pub(super) design: TaskStamp,
    pub(super) accent: usize,
    /// Its room takes a post now.
    pub(super) available: bool,
    /// A quote is drafted, which posts without any primary text.
    pub(super) attached: bool,
    pub(super) post: Callback<()>,
}

/// The selector naming one design's composer section within its form.
fn section(design: &TaskStamp) -> String {
    format!("[data-design=\"{design}\"]")
}

/// Post every drafted composer whose room takes a post, each to its own room.
pub(super) fn submit(postables: &[Postable]) -> Callback<SubmitEvent> {
    let targets = postables
        .iter()
        .filter(|postable| postable.available)
        .map(|postable| {
            (
                section(&postable.design),
                postable.attached,
                postable.post.clone(),
            )
        })
        .collect::<Vec<_>>();
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(form) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        for (section, attached, post) in &targets {
            let typed = form
                .query_selector(&format!(".team-agent{section}"))
                .ok()
                .flatten()
                .is_some_and(|section| holds_text(&section));
            if typed || *attached {
                post.emit(());
            }
        }
    })
}

/// The panel: TURBO, the arrangement menu where the team menu stands, the
/// speech slider, the Paused/Running brake in the allocation slot, and SEND. A group
/// is on while any lane claims its designs' work, as a team is while a member
/// runs; with no claimant, no quote drafted, and no post in flight it rests
/// exactly as an idle team's panel does.
pub(super) fn render(
    render: &GroupRender<'_, '_>,
    group: &RoomAuthorityStamp,
    postables: &[Postable],
    posting: bool,
) -> Html {
    let controls = render.controls;
    let projection = render.runtime.room_projection();
    let claimants = render
        .runtime
        .board()
        .and_then(|board| crate::roster::claimants(projection.partition(), board).remove(group))
        .unwrap_or_default();
    let names = claimants
        .iter()
        .map(|lane| super::super::conversation::lane_label(render.runtime, *lane))
        .collect::<Vec<_>>();
    let report = {
        let notice = controls.notice.clone();
        Callback::from(move |text| notice.set(Some(text)))
    };
    let group_controls = GroupPanel {
        runtime: render.runtime,
        controls: render.team_controls,
        set_automatic_speech: &render.agents.set_automatic_speech,
        sender: render.sender,
        report,
        group,
        claimants: &claimants,
        names: &names,
    }
    .render();
    let slots = postables
        .iter()
        .map(|postable| SendSlot {
            section: section(&postable.design),
            accent: postable.accent,
            reachable: postable.available,
            attached: postable.attached,
            broadcast: false,
        })
        .collect::<Vec<_>>();
    let ready = postables.iter().any(|postable| postable.available);
    let dormant =
        claimants.is_empty() && !posting && !postables.iter().any(|postable| postable.attached);
    let open = controls.arranging.as_ref() == Some(group);
    let menu = {
        let arranging = controls.arranging.clone();
        let group = group.clone();
        html! {
            <button
                class="team-menu"
                type="button"
                aria-label="Group menu"
                aria-haspopup="menu"
                aria-expanded={open.to_string()}
                title="Group menu"
                onclick={Callback::from(move |event: MouseEvent| {
                    event.stop_propagation();
                    // Safari does not focus a clicked button; closing from here keeps focus here.
                    if let Some(button) = event
                        .current_target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = button.focus();
                    }
                    arranging.set((!open).then(|| group.clone()));
                })}
            >
                <span class="composer-menu-icon" aria-hidden="true">
                    <span></span><span></span><span></span>
                </span>
            </button>
        }
    };
    html! {
        <ControlsFrame
            {dormant}
            {ready}
            topology_revision={projection.partition().revision}
            {slots}
            verb="Send"
            noun="design"
            speech={Some(group_controls.speech)}
            lifetime={Some(group_controls.stance)}
            lifetime_pending={false}
            turbo={group_controls.turbo}
            {menu}
            children={group_controls.sliders}
        />
    }
}

/// The group's menu: this browser's console placement and the shared action
/// that ends the group. Membership changes otherwise stay with each design's
/// menu and the working-group roster.
pub(super) fn render_arrangement(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    index: usize,
    name: &str,
) -> Html {
    let controls = render.controls;
    if controls.arranging.as_ref() != Some(&view.group) {
        return Html::default();
    }
    let count = controls.views.groups().len();
    let action = |kind: &'static str,
                  label: &'static str,
                  detail: &'static str,
                  enabled: bool,
                  run: Callback<()>| {
        let arranging = controls.arranging.clone();
        html! {
            <button
                type="button"
                class="team-action team-arrangement-action"
                role="menuitem"
                disabled={!enabled}
                data-group-arrangement={kind}
                onclick={Callback::from(move |_| {
                    arranging.set(None);
                    run.emit(());
                })}
            >
                <span class="team-action-label">{label}</span>
                <span class="team-action-detail">{detail}</span>
            </button>
        }
    };
    let move_group = |delta| {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |()| controls.move_group(group.clone(), delta))
    };
    let close = {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |()| controls.close_group(group.clone()))
    };
    let disband = {
        let sender = render.sender.clone();
        let controls = controls.clone();
        let group = view.group.clone();
        let revision = render.runtime.room_projection().partition().revision;
        Callback::from(move |()| {
            super::super::submit_group_change(
                &sender,
                &controls,
                crate::client_action::GroupChange::Disband(wire::rooms::WorkingGroupDisband {
                    group: group.clone(),
                }),
                revision,
            );
        })
    };
    let can_disband = render
        .runtime
        .offers(wire::CommandKind::WorkingGroupDisband);
    html! {
        <div
            class="team-action-menu team-arrangement-menu app-overlay-panel app-overlay-panel--popover"
            role="menu"
            aria-label={format!("Arrange group {name}")}
        >
            {action("left", "Move group left", "this browser's console order", index > 0, move_group(-1))}
            {action("right", "Move group right", "this browser's console order", index + 1 < count, move_group(1))}
            {action("close", "Close group console", "the group stays in Working groups", true, close)}
            {action("disband", "Disband working group", "leave every Design ungrouped", can_disband, disband)}
        </div>
    }
}

/// A click outside the open arrangement menu and its opener closes it.
pub(super) fn dismissal(controls: &Controls, group: &RoomAuthorityStamp) -> Callback<MouseEvent> {
    let arranging = controls.arranging.clone();
    let open = controls.arranging.as_ref() == Some(group);
    Callback::from(move |event: MouseEvent| {
        let inside = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| {
                target
                    .closest(".team-action-menu, .team-menu")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if open && !inside {
            arranging.set(None);
        }
    })
}

/// Opening a group's arrangement moves focus to its first available choice;
/// Escape closes it and returns focus to the group menu that opened it.
#[hook]
pub(in crate::browser::rooms) fn use_arrangement_focus(
    arranging: &UseStateHandle<Option<RoomAuthorityStamp>>,
) {
    let open = (**arranging).clone();
    use_effect_with(open.clone(), |open| {
        if let Some(group) = open
            && let Some(choice) = group_element(group, ".team-arrangement-action:not([disabled])")
        {
            let _ = choice.focus();
        }
    });
    let dismiss = {
        let arranging = arranging.clone();
        let open = open.clone();
        Callback::from(move |()| {
            if let Some(group) = &open
                && let Some(trigger) = group_element(group, ".team-menu")
            {
                crate::browser::overlay::return_focus(&trigger);
            }
            arranging.set(None);
        })
    };
    crate::browser::overlay::use_escape_owner(open.is_some(), dismiss);
}

fn group_element(group: &RoomAuthorityStamp, selector: &str) -> Option<web_sys::HtmlElement> {
    web_sys::window()?
        .document()?
        .query_selector(&format!(
            ".group-surface[data-group=\"{group}\"] {selector}"
        ))
        .ok()
        .flatten()?
        .dyn_into()
        .ok()
}
