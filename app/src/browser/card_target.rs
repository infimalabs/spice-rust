use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wire::CanonicalUuid;
use yew::prelude::*;

use crate::AppRuntime;

use super::SurfaceData;
use super::watchband::{TeamControlAction, TeamControls, TeamMode};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Status {
    pub(super) target: String,
    pub(super) available: bool,
}

/// History-state key naming where a reader left through a card link.
const RETURN_STATE: &str = "appReturn";

/// Work links whose Chat destination records the Work card they leave: task
/// message links and direction recipient badges.
const WORK_RETURN_LINKS: &str = "a.work-message-link[href], a.team-direction-recipient-link[href]";

/// The element holding a followed card link and its offset inside its scrolling
/// surface at that moment. Team modes and Chat lanes scroll independently, so
/// the browser cannot restore this position itself.
struct ReturnAnchor {
    card: String,
    offset: f64,
    team: Option<wire::CanonicalUuid>,
    mode: Option<TeamMode>,
    /// Distinguishes one recorded departure from a later one on the same entry.
    nonce: f64,
}

impl ReturnAnchor {
    fn read(history: &web_sys::History) -> Option<Self> {
        let anchor = js_sys::Reflect::get(&history.state().ok()?, &RETURN_STATE.into()).ok()?;
        let field = |name: &str| js_sys::Reflect::get(&anchor, &name.into()).ok();
        let team = field("team")
            .and_then(|team| team.as_string())
            .and_then(|team| team.parse().ok());
        let mode = field("mode")
            .and_then(|mode| mode.as_string())
            .and_then(|mode| match mode.as_str() {
                "work" => Some(TeamMode::Work),
                "chat" => Some(TeamMode::Chat),
                _ => None,
            });
        Some(Self {
            card: field("card")?.as_string()?,
            offset: field("offset")?.as_f64()?,
            team,
            mode,
            nonce: field("nonce")?.as_f64()?,
        })
    }

    /// Scroll the anchor card back to its recorded offset; false while absent.
    fn restore(&self, document: &web_sys::Document) -> bool {
        let Some(card) = document.get_element_by_id(&self.card) else {
            return false;
        };
        let Some((plane, offset)) = return_offset(&card) else {
            return false;
        };
        plane.scroll_by_with_x_and_y(0.0, offset - self.offset);
        if let Ok(card) = card.dyn_into::<web_sys::HtmlElement>() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = card.focus_with_options(&options);
        }
        true
    }
}

/// A card's scrolling lane plane and the card's top edge relative to it.
fn plane_offset(card: &web_sys::Element) -> Option<(web_sys::Element, f64)> {
    let plane = card.closest(".lane-mosaic-plane").ok()??;
    let offset = card.get_bounding_client_rect().top() - plane.get_bounding_client_rect().top();
    Some((plane, offset))
}

/// A return origin's scrolling surface and top edge relative to it.
fn return_offset(origin: &web_sys::Element) -> Option<(web_sys::Element, f64)> {
    let surface = origin
        .closest(".lane-mosaic-plane, .team-mode-pane, .work-lane-track")
        .ok()??;
    let offset = origin.get_bounding_client_rect().top() - surface.get_bounding_client_rect().top();
    Some((surface, offset))
}

/// Record where a plain quote link leaves its containing Chat card.
pub(super) fn remember_quote_return(event: &web_sys::MouseEvent) {
    let team = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".team-surface").ok().flatten())
        .and_then(|surface| surface.get_attribute("data-team"))
        .and_then(|team| team.parse().ok())
        .map(|team| (team, TeamMode::Chat));
    let _ = remember_return(
        event,
        "a.message-quote-age[href]",
        ".mosaic-message-card",
        team,
    );
}

/// Preserve the Chat card holding a task quick view before following its
/// creator-context link.
pub(super) fn remember_creator_return(event: &web_sys::MouseEvent) -> bool {
    remember_return(
        event,
        "a.creator-context-link[href]",
        ".mosaic-message-card",
        None,
    )
}

/// Preserve a global Work card before following one of its Chat links.
pub(super) fn remember_global_work_return(event: &web_sys::MouseEvent) {
    let _ = remember_return(event, WORK_RETURN_LINKS, "[data-card-return-origin]", None);
}

/// Record where a plain Work link leaves, returning whether this tab should
/// switch the team to Chat. Modified clicks retain native new-tab behavior and
/// must not change this tab's team mode.
pub(super) fn remember_work_return(event: &web_sys::MouseEvent, team: wire::CanonicalUuid) -> bool {
    remember_return(
        event,
        WORK_RETURN_LINKS,
        "[data-card-return-origin]",
        Some((team, TeamMode::Work)),
    )
}
/// Whether a native anchor click will navigate this tab.
pub(super) fn follows_in_tab(event: &web_sys::MouseEvent) -> bool {
    !event.default_prevented()
        && event.button() == 0
        && !event.meta_key()
        && !event.ctrl_key()
        && !event.shift_key()
        && !event.alt_key()
}

fn remember_return(
    event: &web_sys::MouseEvent,
    link_selector: &str,
    origin_selector: &str,
    team_mode: Option<(wire::CanonicalUuid, TeamMode)>,
) -> bool {
    if !follows_in_tab(event) {
        return false;
    }
    let Some(link) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(link_selector).ok().flatten())
    else {
        return false;
    };
    let Some(origin) = link.closest(origin_selector).ok().flatten() else {
        return false;
    };
    let Some(window) = web_sys::window() else {
        return false;
    };
    // A link to the current entry navigates nowhere, so nothing is left behind.
    if origin.id().is_empty() || link.get_attribute("href") == window.location().hash().ok() {
        return false;
    }
    let (Ok(history), Some((_, offset))) = (window.history(), return_offset(&origin)) else {
        return false;
    };
    let state = js_sys::Object::new();
    let anchor = js_sys::Object::new();
    for (name, value) in [
        ("card", JsValue::from(origin.id())),
        ("offset", JsValue::from(offset)),
        ("nonce", JsValue::from(js_sys::Date::now())),
    ] {
        let _ = js_sys::Reflect::set(&anchor, &name.into(), &value);
    }
    if let Some((team, mode)) = team_mode {
        let mode = match mode {
            TeamMode::Work => "work",
            TeamMode::Chat => "chat",
        };
        let _ = js_sys::Reflect::set(&anchor, &"team".into(), &JsValue::from(team.to_string()));
        let _ = js_sys::Reflect::set(&anchor, &"mode".into(), &JsValue::from(mode));
    }
    let _ = js_sys::Reflect::set(&state, &RETURN_STATE.into(), &anchor);
    if history.replace_state(&state, "").is_err() {
        return false;
    }
    true
}

/// The departure recorded on the current history entry. Back from a route with
/// no card target, such as a direction, to an entry with none either changes
/// only this, so the route effect keys on it to restore the entry it left.
fn entry_departure() -> Option<f64> {
    let history = web_sys::window()?.history().ok()?;
    ReturnAnchor::read(&history).map(|anchor| anchor.nonce)
}

/// Restore a returned-to entry's anchor once per arrival at that entry.
fn returned_to(
    restored: &Rc<RefCell<Option<f64>>>,
    anchor: &ReturnAnchor,
    document: &web_sys::Document,
) -> bool {
    if *restored.borrow() == Some(anchor.nonce) {
        return true;
    }
    let returned = anchor.restore(document);
    if returned {
        *restored.borrow_mut() = Some(anchor.nonce);
    }
    returned
}

/// Keeps a routed card where its route brought it while staged history grows
/// its lane plane around it. The plane disables browser scroll anchoring, so
/// without this a cold permalink load drifts off its card as older cards land
/// above it. The first scroll the pin did not make belongs to the reader and
/// releases it.
struct Pin {
    observer: web_sys::ResizeObserver,
    _aligned: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
}

impl Pin {
    fn hold(card: &web_sys::Element) -> Option<Self> {
        let (plane, offset) = plane_offset(card)?;
        let track = card.closest(".lane-mosaic-track").ok()??;
        let expected = Cell::new(plane.scroll_top());
        let card = card.clone();
        let aligned = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
            move |_, observer: web_sys::ResizeObserver| {
                if plane.scroll_top() != expected.get() || !card.is_connected() {
                    observer.disconnect();
                    return;
                }
                if let Some((_, now)) = plane_offset(&card) {
                    plane.scroll_by_with_x_and_y(0.0, now - offset);
                }
                expected.set(plane.scroll_top());
            },
        );
        let observer = web_sys::ResizeObserver::new(aligned.as_ref().unchecked_ref()).ok()?;
        observer.observe(&track);
        Some(Self {
            observer,
            _aligned: aligned,
        })
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

/// Bring a route's element into view and focus, report it reached, and pin it.
fn reach(
    element: &web_sys::Element,
    target: &str,
    status: &UseStateHandle<Option<Status>>,
) -> Option<Pin> {
    element.scroll_into_view();
    if let Some(element) = element.dyn_ref::<web_sys::HtmlElement>() {
        let _ = element.focus();
    }
    status.set(Some(Status {
        target: target.to_owned(),
        available: true,
    }));
    Pin::hold(element)
}

/// Layout phase attribute; a `measuring` card is hidden until its lattice commits.
const LAYOUT_PHASE: &str = "data-layout-phase";

/// The rendered element a route names. A card counts only once its layout
/// commits: while measuring it is hidden, so it can neither be scrolled to
/// meaningfully nor take focus, and its node may be replaced as the layout commits.
/// Route targets are ASCII alphanumerics and `-`, so they quote verbatim.
fn routed_element(document: &web_sys::Document, target: &str) -> Option<web_sys::Element> {
    let nodes = document
        .query_selector_all(&format!("[id=\"{target}\"]"))
        .ok()?;
    (0..nodes.length())
        .filter_map(|index| nodes.item(index)?.dyn_into::<web_sys::Element>().ok())
        .find(|element| element.get_attribute(LAYOUT_PHASE).as_deref() != Some("measuring"))
}

fn routed_team(element: &web_sys::Element) -> Option<wire::CanonicalUuid> {
    element
        .closest(".team-surface")
        .ok()??
        .get_attribute("data-team")?
        .parse()
        .ok()
}

/// Watches the document for a routed element that has not rendered yet.
/// Staged hydration renders cards without advancing the stream cursor, so a
/// cold permalink otherwise waits on an unrelated live event to find its card.
struct Seek {
    observer: web_sys::MutationObserver,
    _found: Closure<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
    /// The card this seek marked selected, outside the route effect's care.
    marked: Rc<RefCell<Option<web_sys::Element>>>,
}

impl Seek {
    fn start(
        document: &web_sys::Document,
        target: &str,
        status_target: &str,
        status: &UseStateHandle<Option<Status>>,
        pin: &Rc<RefCell<Option<Pin>>>,
    ) -> Option<Self> {
        let body = document.body()?;
        let marked = Rc::new(RefCell::new(None));
        let (document, target, status_target, status, pin, mark) = (
            document.clone(),
            target.to_owned(),
            status_target.to_owned(),
            status.clone(),
            Rc::clone(pin),
            Rc::clone(&marked),
        );
        let found = Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
            move |_, observer: web_sys::MutationObserver| {
                let Some(element) = routed_element(&document, &target) else {
                    return;
                };
                observer.disconnect();
                if route_selection(&element) {
                    let _ = element.set_attribute("data-card-route-selected", "true");
                    *mark.borrow_mut() = Some(element.clone());
                }
                *pin.borrow_mut() = reach(&element, &status_target, &status);
            },
        );
        let observer = web_sys::MutationObserver::new(found.as_ref().unchecked_ref()).ok()?;
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        // A rendered card becomes reachable when its layout commits.
        options.set_attribute_filter(&js_sys::Array::of1(&LAYOUT_PHASE.into()));
        observer.observe_with_options(&body, &options).ok()?;
        Some(Self {
            observer,
            _found: found,
            marked,
        })
    }
}

impl Drop for Seek {
    fn drop(&mut self) {
        self.observer.disconnect();
        if let Some(element) = self.marked.borrow_mut().take() {
            let _ = element.remove_attribute("data-card-route-selected");
        }
    }
}

/// Restore a returned-to entry's team mode once per arrival, then its anchor.
/// Mode changes the reader makes on that entry afterward stand.
fn restore_team_entry(
    anchor: Option<&ReturnAnchor>,
    team_controls: &UseReducerHandle<TeamControls>,
    restored: &Rc<RefCell<Option<f64>>>,
    moved: &Rc<RefCell<Option<f64>>>,
    document: Option<&web_sys::Document>,
) -> bool {
    let arrived = anchor.filter(|anchor| *moved.borrow() != Some(anchor.nonce));
    if let Some(anchor) = arrived {
        *moved.borrow_mut() = Some(anchor.nonce);
        if let Some((team, mode)) = anchor
            .team
            .zip(anchor.mode)
            .filter(|(team, mode)| team_controls.mode(*team) != *mode)
        {
            team_controls.dispatch(TeamControlAction::SetMode {
                team: Some(team),
                mode,
            });
            return false;
        }
    }
    match (anchor, document) {
        (Some(anchor), Some(document)) => returned_to(restored, anchor, document),
        _ => {
            // Leaving the entry lets a later Back restore it again.
            restored.borrow_mut().take();
            moved.borrow_mut().take();
            false
        }
    }
}

/// One route target being brought into view: sought until it renders, then
/// pinned until the reader scrolls. Dropped when the route moves on.
struct Hold {
    target: String,
    _seek: Option<Seek>,
    _pin: Rc<RefCell<Option<Pin>>>,
}

/// The latest visible card before one task-creation boundary.
fn creator_message_target(
    creator: &crate::view::CreatorContextTarget,
    runtime: &AppRuntime,
) -> Option<String> {
    runtime
        .messages()
        .iter()
        .filter(|message| creator.contains(message))
        .max_by_key(|message| {
            (
                message.locus.member,
                message.locus.offset,
                message.locus.event,
            )
        })
        .map(|message| super::watchband::agent_message_source_id(message))
}

#[derive(PartialEq)]
struct TargetResolution {
    ready: bool,
    routed: Option<String>,
    history: Option<wire::MessageHistoryCursor>,
    creator: bool,
    /// Both generations make another completed page re-run the route effect.
    _revision: u64,
    _completion: u64,
}

fn target_resolution(target: Option<&str>, surface: &SurfaceData) -> TargetResolution {
    let surface = surface.read();
    let runtime = surface.as_ref();
    let creator = target.and_then(crate::view::CreatorContextTarget::parse);
    let routed = target.and_then(|target| {
        creator.as_ref().map_or_else(
            || Some(target.to_owned()),
            |creator| runtime.and_then(|runtime| creator_message_target(creator, runtime)),
        )
    });
    let history = creator
        .as_ref()
        .filter(|_| routed.is_none())
        .and_then(|creator| {
            runtime.and_then(|runtime| runtime.message_history_target(creator.lane))
        });
    TargetResolution {
        ready: runtime.and_then(AppRuntime::lanes).is_some(),
        routed,
        history,
        creator: creator.is_some(),
        _revision: runtime.map_or(0, AppRuntime::message_history_revision),
        _completion: creator.as_ref().map_or(0, |creator| {
            runtime.map_or(0, |runtime| {
                runtime.message_history_completion(creator.lane)
            })
        }),
    }
}

/// Whether retained Chat state holds the card a route names, once the lane
/// snapshot and a transcript generation are admitted. Chat renders only
/// teamed lanes, so a card elsewhere is not retained. A creator-context
/// target stays unsettled while its lane can still page older history.
fn retained(target: &str, runtime: &AppRuntime) -> Option<bool> {
    let lanes = runtime.lanes()?;
    runtime.message_generation()?;
    if let Some(creator) = crate::view::CreatorContextTarget::parse(target) {
        if creator_message_target(&creator, runtime).is_some() {
            return Some(true);
        }
        return runtime
            .message_history_target(creator.lane)
            .is_none()
            .then_some(false);
    }
    let chat_member = |lane: CanonicalUuid| {
        lanes
            .lanes
            .iter()
            .filter(|team| team.teamed())
            .any(|team| team.members.iter().any(|member| member.lane == lane))
    };
    if let Some(team) = crate::view::team_stream_card_team(target) {
        return Some(lanes.chat_target_for_team(team).is_some());
    }
    if let Some(lane) = target.strip_prefix(super::watchband::PRIMARY_TEXTAREA_PREFIX) {
        return Some(lane.parse().is_ok_and(chat_member));
    }
    if let Some(seq) = target.strip_prefix(super::watchband::LIFECYCLE_CARD_PREFIX) {
        return Some(
            runtime
                .lifecycle_boundaries()
                .iter()
                .any(|boundary| chat_member(boundary.lane()) && boundary.seq().to_string() == seq),
        );
    }
    Some(runtime.messages().iter().any(|message| {
        chat_member(message.lane) && super::watchband::agent_message_source_id(message) == target
    }))
}

/// Settled retention of the route's card target, recomputed only when the
/// retained Chat state it reads changes.
#[hook]
pub(super) fn use_retention(target: Option<&str>, surface: &SurfaceData) -> Option<bool> {
    let revision = {
        let surface = surface.read();
        surface.as_ref().map(|runtime| {
            let creator = target
                .and_then(crate::view::CreatorContextTarget::parse)
                .map_or(0, |creator| {
                    runtime.message_history_completion(creator.lane)
                });
            (
                runtime
                    .lanes()
                    .map(|lanes| (lanes.epoch, lanes.topology_revision)),
                runtime.message_generation(),
                runtime.message_history_revision(),
                runtime.messages().len(),
                runtime.lifecycle_boundaries().len(),
                creator,
            )
        })
    };
    let surface = surface.clone();
    *use_memo((target.map(str::to_owned), revision), move |(target, _)| {
        let surface = surface.read();
        target
            .as_deref()
            .zip(surface.as_ref())
            .and_then(|(target, runtime)| retained(target, runtime))
    })
}

/// Focus a card named by the current route once retained Chat state is ready,
/// unless history returned to an entry a quote link left: then the card that
/// held the link returns to where the reader saw it.
///
/// Creator-context targets resolve to the latest card before their captured
/// transcript boundary. If that card predates the loaded window, request the
/// owning lane's history until a page crosses the boundary.
#[hook]
pub(super) fn use_target(
    target: Option<String>,
    surface: &SurfaceData,
    sender: &super::SenderHandle,
    status: UseStateHandle<Option<Status>>,
    team_controls: UseReducerHandle<TeamControls>,
    visible: bool,
) {
    let restored = use_mut_ref(|| None::<f64>);
    let moved = use_mut_ref(|| None::<f64>);
    let opened = use_mut_ref(|| None::<String>);
    let held = use_mut_ref(|| None::<Hold>);
    let resolution = target_resolution(target.as_deref(), surface);
    let departure = entry_departure();
    let sender = sender.clone();
    use_effect_with(
        (target, resolution, visible, team_controls, departure),
        move |(target, resolution, visible, team_controls, _)| {
            let mut selected = None;
            let window = web_sys::window();
            let document = window.as_ref().and_then(web_sys::Window::document);
            let anchor = window
                .as_ref()
                .and_then(|window| window.history().ok())
                .and_then(|history| ReturnAnchor::read(&history))
                .filter(|_| target.is_none() && resolution.ready);
            let returned = restore_team_entry(
                anchor.as_ref(),
                team_controls,
                &restored,
                &moved,
                document.as_ref(),
            );
            if returned
                || held.borrow().as_ref().is_some_and(|hold| {
                    Some(hold.target.as_str()) != target.as_deref() || !*visible
                })
            {
                held.borrow_mut().take();
            }
            if let Some(requested) = target.as_deref().filter(|_| *visible) {
                if resolution.ready {
                    if let Some(target) = resolution.routed.as_deref() {
                        let route = Route {
                            target,
                            status_target: requested,
                            returned,
                            document: document.as_ref(),
                            status: &status,
                            team_controls,
                            team: None,
                            mode: TeamMode::Chat,
                        };
                        selected = route.reach(&held, &opened);
                    } else if resolution.creator {
                        held.borrow_mut().take();
                        opened.borrow_mut().take();
                        if let Some(cursor) = &resolution.history {
                            if status
                                .as_ref()
                                .is_some_and(|known| known.target == requested)
                            {
                                status.set(None);
                            }
                            let _ = super::admit_action(
                                &sender,
                                super::ClientAction::MessageHistory {
                                    generation: cursor.generation,
                                    lane: cursor.lane,
                                    before: cursor.before,
                                },
                            );
                        } else if status
                            .as_ref()
                            .is_none_or(|known| known.target != requested || known.available)
                        {
                            status.set(Some(Status {
                                target: requested.to_owned(),
                                available: false,
                            }));
                        }
                    }
                }
            } else {
                // A later visit to the same card opens its team's Chat again.
                opened.borrow_mut().take();
                if status.is_some() {
                    status.set(None);
                }
            }
            move || {
                if let Some(element) = selected {
                    let _ = element.remove_attribute("data-card-route-selected");
                }
            }
        },
    );
}

/// Focus and mark the global Work direction card a durable route names, once
/// its anchored page renders it.
#[hook]
pub(super) fn use_global_direction_target(
    target: Option<String>,
    team_controls: UseReducerHandle<TeamControls>,
    visible: bool,
) {
    let status = use_state(|| None::<Status>);
    let opened = use_mut_ref(|| None::<String>);
    let held = use_mut_ref(|| None::<Hold>);
    use_effect_with((target, visible), move |(target, visible)| {
        let mut selected = None;
        if let Some(requested) = target.as_deref().filter(|_| *visible) {
            if held
                .borrow()
                .as_ref()
                .is_some_and(|hold| hold.target != requested)
            {
                held.borrow_mut().take();
            }
            let document = web_sys::window().and_then(|window| window.document());
            let route = Route {
                target: requested,
                status_target: requested,
                returned: false,
                document: document.as_ref(),
                status: &status,
                team_controls: &team_controls,
                team: None,
                mode: TeamMode::Work,
            };
            selected = route.reach(&held, &opened);
        } else {
            held.borrow_mut().take();
            opened.borrow_mut().take();
            if status.is_some() {
                status.set(None);
            }
        }
        move || {
            if let Some(element) = selected {
                let _ = element.remove_attribute("data-card-route-selected");
            }
        }
    });
}

/// One visible route target once Chat state is ready.
struct Route<'a> {
    /// Concrete rendered card ID.
    target: &'a str,
    /// Stable URL target, including a creator-context boundary.
    status_target: &'a str,
    returned: bool,
    document: Option<&'a web_sys::Document>,
    status: &'a UseStateHandle<Option<Status>>,
    team_controls: &'a UseReducerHandle<TeamControls>,
    team: Option<wire::CanonicalUuid>,
    mode: TeamMode,
}

impl Route<'_> {
    /// Open the routed element's team in its required mode once per route,
    /// since any later mode change is the reader's, then reach, seek or settle
    /// the element. Returns the element marked as the route's selection.
    fn reach(
        &self,
        held: &Rc<RefCell<Option<Hold>>>,
        opened: &Rc<RefCell<Option<String>>>,
    ) -> Option<web_sys::Element> {
        let (target, status_target, status) = (self.target, self.status_target, self.status);
        let element = self
            .document
            .and_then(|document| routed_element(document, target));
        if element.is_some() && opened.borrow().as_deref() != Some(status_target) {
            *opened.borrow_mut() = Some(status_target.to_owned());
            if let Some(team) = self
                .team
                .or_else(|| element.as_ref().and_then(routed_team))
                .filter(|team| self.team_controls.mode(*team) != self.mode)
            {
                self.team_controls.dispatch(TeamControlAction::SetMode {
                    team: Some(team),
                    mode: self.mode,
                });
                return None;
            }
        }
        let selected = element
            .as_ref()
            .filter(|element| route_selection(element))
            .cloned();
        if let Some(selected) = &selected {
            let _ = selected.set_attribute("data-card-route-selected", "true");
        }
        let settled = self.returned
            || status
                .as_ref()
                .is_some_and(|settled| settled.target == status_target && settled.available);
        if !settled {
            if let Some(element) = element {
                let pin = Rc::new(RefCell::new(reach(&element, status_target, status)));
                *held.borrow_mut() = Some(Hold {
                    target: status_target.to_owned(),
                    _seek: None,
                    _pin: pin,
                });
            } else {
                status.set(Some(Status {
                    target: status_target.to_owned(),
                    available: false,
                }));
                if held.borrow().is_none()
                    && let Some(document) = self.document
                {
                    let pin = Rc::new(RefCell::new(None));
                    let seek = Seek::start(document, target, status_target, status, &pin);
                    *held.borrow_mut() = Some(Hold {
                        target: status_target.to_owned(),
                        _seek: seek,
                        _pin: pin,
                    });
                }
            }
        } else if self.returned
            && status
                .as_ref()
                .is_none_or(|known| known.target != status_target)
        {
            // The route's own card counts as reached, so a later stream
            // advance does not yank the reader to it.
            status.set(Some(Status {
                target: status_target.to_owned(),
                available: element.is_some(),
            }));
        }
        selected
    }
}

fn route_selection(element: &web_sys::Element) -> bool {
    element
        .matches(".mosaic-message-card, .team-direction-ledger-row")
        .unwrap_or(false)
}
