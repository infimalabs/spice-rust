use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wire::CanonicalUuid;
use yew::prelude::*;

use crate::CanvasRuntime;

use super::SurfaceData;
use super::watchband::{TeamControlAction, TeamControls, TeamMode};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Status {
    pub(super) target: String,
    pub(super) available: bool,
}

/// History-state key naming where a reader left through a card link.
const RETURN_STATE: &str = "canvasReturn";

/// Preserve the exact Dash row and Flow position a Chat target leaves.
pub(super) fn remember_team_return(event: &web_sys::MouseEvent, team: CanonicalUuid) {
    let _ = remember_return(
        event,
        "a[data-dash-return][href]",
        "[data-dash-return-origin]",
        Some((team, TeamMode::Dash)),
    );
}

/// The element holding a followed card link and its offset inside its scrolling
/// surface at that moment. Team modes and Chat lanes scroll independently, so
/// the browser cannot restore this position itself.
struct ReturnAnchor {
    card: String,
    offset: f64,
    team: Option<wire::CanonicalUuid>,
    mode: Option<TeamMode>,
    flow_scroll: Option<i32>,
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
                "dash" => Some(TeamMode::Dash),
                "work" => Some(TeamMode::Work),
                "chat" => Some(TeamMode::Chat),
                _ => None,
            });
        Some(Self {
            card: field("card")?.as_string()?,
            offset: field("offset")?.as_f64()?,
            team,
            mode,
            flow_scroll: field("flowScroll")
                .and_then(|scroll| scroll.as_f64())
                .map(|scroll| scroll as i32),
            nonce: field("nonce")?.as_f64()?,
        })
    }

    /// Scroll the anchor card back to its recorded offset; false while absent.
    fn restore(&self, document: &web_sys::Document) -> bool {
        let Some(card) = document.get_element_by_id(&self.card) else {
            return false;
        };
        if let Some(scroll) = self.flow_scroll
            && let Some(flow) = card
                .closest(".team-surface")
                .ok()
                .flatten()
                .and_then(|team| team.query_selector(".team-dash-flow-body").ok().flatten())
                .and_then(|flow| flow.dyn_into::<web_sys::HtmlElement>().ok())
        {
            flow.set_scroll_top(scroll);
        }
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
        .closest(".lane-mosaic-plane, .team-mode-pane")
        .ok()??;
    let offset = origin.get_bounding_client_rect().top() - surface.get_bounding_client_rect().top();
    Some((surface, offset))
}

/// Record where a plain quote link leaves its containing Chat card.
pub(super) fn remember_quote_return(event: &web_sys::MouseEvent) {
    let _ = remember_return(
        event,
        "a.message-quote-age[href]",
        ".mosaic-message-card",
        None,
    );
}

/// Record where a plain Work link leaves, returning whether this tab should
/// switch the team to Chat. Modified clicks retain native new-tab behavior and
/// must not change this tab's team mode.
pub(super) fn remember_work_return(event: &web_sys::MouseEvent, team: wire::CanonicalUuid) -> bool {
    remember_return(
        event,
        "a.work-message-link[href]",
        "[data-card-return-origin]",
        Some((team, TeamMode::Work)),
    )
}

fn remember_return(
    event: &web_sys::MouseEvent,
    link_selector: &str,
    origin_selector: &str,
    team_mode: Option<(wire::CanonicalUuid, TeamMode)>,
) -> bool {
    if event.default_prevented()
        || event.button() != 0
        || event.meta_key()
        || event.ctrl_key()
        || event.shift_key()
        || event.alt_key()
    {
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
    // Keep the history library's own entry identity beside the anchor.
    if let Ok(current) = history.state()
        && let Some(current) = current.dyn_ref::<js_sys::Object>()
    {
        js_sys::Object::assign(&state, current);
    }
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
            TeamMode::Dash => "dash",
            TeamMode::Work => "work",
            TeamMode::Chat => "chat",
        };
        let _ = js_sys::Reflect::set(&anchor, &"team".into(), &JsValue::from(team.to_string()));
        let _ = js_sys::Reflect::set(&anchor, &"mode".into(), &JsValue::from(mode));
        if mode == "dash"
            && let Some(scroll) = origin
                .closest(".team-surface")
                .ok()
                .flatten()
                .and_then(|team| team.query_selector(".team-dash-flow-body").ok().flatten())
                .and_then(|flow| flow.dyn_into::<web_sys::HtmlElement>().ok())
                .map(|flow| flow.scroll_top())
        {
            let _ = js_sys::Reflect::set(&anchor, &"flowScroll".into(), &JsValue::from(scroll));
        }
    }
    let _ = js_sys::Reflect::set(&state, &RETURN_STATE.into(), &anchor);
    if history.replace_state(&state, "").is_err() {
        return false;
    }
    true
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
        status: &UseStateHandle<Option<Status>>,
        pin: &Rc<RefCell<Option<Pin>>>,
    ) -> Option<Self> {
        let body = document.body()?;
        let marked = Rc::new(RefCell::new(None));
        let (document, target, status, pin, mark) = (
            document.clone(),
            target.to_owned(),
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
                if element.matches(".mosaic-message-card").unwrap_or(false) {
                    let _ = element.set_attribute("data-card-route-selected", "true");
                    *mark.borrow_mut() = Some(element.clone());
                }
                *pin.borrow_mut() = reach(&element, &target, &status);
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

fn restore_team_entry(
    anchor: Option<&ReturnAnchor>,
    team_controls: &UseReducerHandle<TeamControls>,
    restored: &Rc<RefCell<Option<f64>>>,
    document: Option<&web_sys::Document>,
) -> bool {
    let restore_team = anchor
        .and_then(|anchor| anchor.team.zip(anchor.mode))
        .filter(|(team, mode)| team_controls.mode(*team) != *mode);
    if let Some((team, mode)) = restore_team {
        team_controls.dispatch(TeamControlAction::SetMode {
            team: Some(team),
            mode,
        });
        return false;
    }
    match (anchor, document) {
        (Some(anchor), Some(document)) => returned_to(restored, anchor, document),
        _ => {
            // Leaving the entry lets a later Back restore it again.
            restored.borrow_mut().take();
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

/// Focus a card named by the current route once retained Chat state is ready,
/// unless history returned to an entry a quote link left: then the card that
/// held the link returns to where the reader saw it.
///
/// A target not yet rendered is sought as staged hydration renders it, and a
/// reached card stays pinned while that hydration grows its plane, until the
/// reader scrolls it.
#[hook]
pub(super) fn use_target(
    target: Option<String>,
    surface: &SurfaceData,
    status: UseStateHandle<Option<Status>>,
    team_controls: UseReducerHandle<TeamControls>,
    visible: bool,
) {
    let restored = use_mut_ref(|| None::<f64>);
    let held = use_mut_ref(|| None::<Hold>);
    let (ready, revision) = {
        let surface = surface.read();
        let runtime = surface.as_ref();
        (
            runtime.and_then(CanvasRuntime::lanes).is_some(),
            runtime
                .and_then(CanvasRuntime::resume)
                .map(|cursor| cursor.seq.to_string()),
        )
    };
    use_effect_with(
        (target, visible, ready, revision, team_controls),
        move |(target, visible, ready, _, team_controls)| {
            let mut selected = None;
            let window = web_sys::window();
            let document = window.as_ref().and_then(web_sys::Window::document);
            let anchor = window
                .as_ref()
                .and_then(|window| window.history().ok())
                .and_then(|history| ReturnAnchor::read(&history))
                .filter(|_| target.is_none() && *visible && *ready);
            let returned =
                restore_team_entry(anchor.as_ref(), team_controls, &restored, document.as_ref());
            if returned
                || held.borrow().as_ref().is_some_and(|hold| {
                    Some(hold.target.as_str()) != target.as_deref() || !*visible
                })
            {
                held.borrow_mut().take();
            }
            if let Some(target) = target.as_deref().filter(|_| *visible) {
                if *ready {
                    let settled = returned
                        || status
                            .as_ref()
                            .is_some_and(|settled| settled.target == target && settled.available);
                    let element = document
                        .as_ref()
                        .and_then(|document| routed_element(document, target));
                    let open_team = element
                        .as_ref()
                        .and_then(routed_team)
                        .filter(|team| team_controls.mode(*team) != TeamMode::Chat);
                    if let Some(team) = open_team {
                        team_controls.dispatch(TeamControlAction::ShowChat { team });
                    } else {
                        if let Some(element) = element.as_ref().filter(|element| {
                            element.matches(".mosaic-message-card").unwrap_or(false)
                        }) {
                            let _ = element.set_attribute("data-card-route-selected", "true");
                            selected = Some(element.clone());
                        }
                        if !settled {
                            if let Some(element) = element {
                                let pin = Rc::new(RefCell::new(reach(&element, target, &status)));
                                *held.borrow_mut() = Some(Hold {
                                    target: target.to_owned(),
                                    _seek: None,
                                    _pin: pin,
                                });
                            } else {
                                status.set(Some(Status {
                                    target: target.to_owned(),
                                    available: false,
                                }));
                                if held.borrow().is_none()
                                    && let Some(document) = &document
                                {
                                    let pin = Rc::new(RefCell::new(None));
                                    let seek = Seek::start(document, target, &status, &pin);
                                    *held.borrow_mut() = Some(Hold {
                                        target: target.to_owned(),
                                        _seek: seek,
                                        _pin: pin,
                                    });
                                }
                            }
                        } else if returned
                            && status.as_ref().is_none_or(|known| known.target != target)
                        {
                            // The route's own card counts as reached, so a later
                            // stream advance does not yank the reader to it.
                            status.set(Some(Status {
                                target: target.to_owned(),
                                available: element.is_some(),
                            }));
                        }
                    }
                }
            } else if status.is_some() {
                status.set(None);
            }
            move || {
                if let Some(element) = selected {
                    let _ = element.remove_attribute("data-card-route-selected");
                }
            }
        },
    );
}
