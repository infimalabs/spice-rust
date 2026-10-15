//! One image-only top-layer viewer for Markdown, diagrams, and attachments.
//!
//! The viewer lives only while its opener could take focus back: it closes
//! when the opener leaves the document, stops rendering, or falls inside an
//! inert subtree, which is how a route change or a hidden owner ends it. Every
//! close path converges on one generation-checked clear, so stale close
//! events and repeated dismissals are no-ops.

use std::rc::Rc;

use wasm_bindgen::prelude::*;
use yew::prelude::*;

use super::overlay;

mod actions;
mod editor;

use super::watchband::{ComposerAction, ComposerDrafts};
use crate::image_navigation::{self, Direction};

#[derive(Clone, PartialEq)]
struct DraftTarget {
    lane: String,
    quote: Option<String>,
    id: u64,
}

#[derive(Clone, PartialEq)]
struct Selection {
    generation: u64,
    source: AttrValue,
    alt: AttrValue,
    name: AttrValue,
    opener: web_sys::Element,
    draft: Option<DraftTarget>,
}

#[derive(Default, PartialEq)]
struct Viewing {
    issued: u64,
    current: Option<Selection>,
}

enum Change {
    Open {
        source: AttrValue,
        alt: AttrValue,
        name: AttrValue,
        opener: web_sys::Element,
        draft: Option<DraftTarget>,
    },
    Close(u64),
}

impl Reducible for Viewing {
    type Action = Change;

    fn reduce(self: Rc<Self>, change: Change) -> Rc<Self> {
        match change {
            Change::Open {
                source,
                alt,
                name,
                opener,
                draft,
            } => {
                let generation = self.issued + 1;
                Rc::new(Self {
                    issued: generation,
                    current: Some(Selection {
                        generation,
                        source,
                        alt,
                        name,
                        opener,
                        draft,
                    }),
                })
            }
            Change::Close(generation)
                if self
                    .current
                    .as_ref()
                    .is_some_and(|current| current.generation == generation) =>
            {
                Rc::new(Self {
                    issued: self.issued,
                    current: None,
                })
            }
            Change::Close(_) => self,
        }
    }
}
const LANE_CARD_SELECTOR: &str = ".mosaic-message-card[data-agent-lane][data-mosaic-card-key]";
const IMAGE_BUTTON_SELECTOR: &str = "button[data-image-preview]:not([disabled])";

fn open_change(opener: web_sys::Element) -> Option<Change> {
    let image = opener
        .query_selector("img")
        .ok()
        .flatten()?
        .dyn_into::<web_sys::HtmlImageElement>()
        .ok()?;
    Some(Change::Open {
        source: image.src().into(),
        alt: image.alt().into(),
        name: opener
            .get_attribute("data-image-name")
            .unwrap_or_default()
            .into(),
        draft: opener.get_attribute("data-image-lane").and_then(|lane| {
            Some(DraftTarget {
                lane,
                quote: opener.get_attribute("data-image-quote"),
                id: opener.get_attribute("data-image-id")?.parse().ok()?,
            })
        }),
        opener,
    })
}

fn element_at(nodes: &web_sys::NodeList, index: usize) -> Option<web_sys::Element> {
    nodes
        .item(u32::try_from(index).ok()?)?
        .dyn_into::<web_sys::Element>()
        .ok()
}

fn lane_media_count(cards: &web_sys::NodeList, index: usize, lane: &str) -> usize {
    let Some(card) = element_at(cards, index)
        .filter(|card| card.get_attribute("data-agent-lane").as_deref() == Some(lane))
    else {
        return 0;
    };
    card.query_selector_all(IMAGE_BUTTON_SELECTOR)
        .map_or(0, |images| images.length() as usize)
}

/// The next lane image in time. Cards are newest first in the DOM while each
/// card's images, including a crystallized run, are oldest first.
fn adjacent_lane_media(
    opener: &web_sys::Element,
    direction: Direction,
) -> Option<web_sys::Element> {
    let card = opener.closest(LANE_CARD_SELECTOR).ok().flatten()?;
    let lane = card.get_attribute("data-agent-lane")?;
    let plane = card.closest(".lane-mosaic-plane").ok().flatten()?;
    let cards = plane.query_selector_all(LANE_CARD_SELECTOR).ok()?;
    let card_count = cards.length() as usize;
    let current_card = (0..card_count)
        .find(|index| element_at(&cards, *index).is_some_and(|candidate| candidate == card))?;
    let images = card.query_selector_all(IMAGE_BUTTON_SELECTOR).ok()?;
    let image_count = images.length() as usize;
    let current_image = (0..image_count)
        .find(|index| element_at(&images, *index).is_some_and(|candidate| candidate == *opener))?;
    let (target_card, target_image) = image_navigation::adjacent_position(
        card_count,
        current_card,
        current_image,
        direction,
        |index| lane_media_count(&cards, index, &lane),
    )?;
    element_at(&cards, target_card)
        .and_then(|card| card.query_selector_all(IMAGE_BUTTON_SELECTOR).ok())
        .and_then(|images| element_at(&images, target_image))
}

fn lane_navigation_available(opener: &web_sys::Element) -> bool {
    opener.closest(LANE_CARD_SELECTOR).ok().flatten().is_some()
}

#[derive(Properties, PartialEq)]
pub(super) struct ProviderProps {
    pub children: Children,
    pub drafts: UseReducerHandle<ComposerDrafts>,
}

#[function_component(Provider)]
pub(super) fn provider(props: &ProviderProps) -> Html {
    let viewing = use_reducer_eq(Viewing::default);
    use_effect_with(viewing.dispatcher(), |viewing| {
        let document = web_sys::window()
            .expect("app window")
            .document()
            .expect("app document");
        let viewing = viewing.clone();
        let opened = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Some(opener) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                .and_then(|target| target.closest("button[data-image-preview]").ok().flatten())
            else {
                return;
            };
            let Some(change) = open_change(opener) else {
                return;
            };
            event.prevent_default();
            viewing.dispatch(change);
        });
        document
            .add_event_listener_with_callback("click", opened.as_ref().unchecked_ref())
            .expect("the app document accepts image activation");
        move || {
            let _ = document
                .remove_event_listener_with_callback("click", opened.as_ref().unchecked_ref());
        }
    });
    html! {
        <>
            {props.children.clone()}
            if let Some(selection) = viewing.current.as_ref() {
                <Viewer
                    key={selection.generation.to_string()}
                    selection={selection.clone()}
                    viewing={viewing.dispatcher()}
                    drafts={props.drafts.clone()}
                />
            }
        </>
    }
}

/// Raw-Markdown portals need listeners on physical nodes rather than delegated Yew events.
#[hook]
pub(super) fn use_button(action: Callback<()>) -> NodeRef {
    let button = use_node_ref();
    use_effect_with((button.clone(), action), move |(button, action)| {
        let registration = button.cast::<web_sys::HtmlElement>().map(|button| {
            let action = action.clone();
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| action.emit(()));
            button
                .add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())
                .expect("the owned button accepts its click listener");
            (button, callback)
        });
        move || {
            if let Some((button, callback)) = registration {
                let _ = button.remove_event_listener_with_callback(
                    "click",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
    button
}

#[derive(Properties, PartialEq)]
struct ViewerProps {
    selection: Selection,
    viewing: UseReducerDispatcher<Viewing>,
    drafts: UseReducerHandle<ComposerDrafts>,
}

const DISCARD_ANNOTATIONS: &str = "Discard annotation changes?";

fn allow_annotation_discard(dirty: bool) -> bool {
    !dirty
        || web_sys::window()
            .and_then(|window| window.confirm_with_message(DISCARD_ANNOTATIONS).ok())
            .unwrap_or(false)
}

#[hook]
fn use_annotation_apply(
    target: Option<DraftTarget>,
    drafts: UseReducerHandle<ComposerDrafts>,
    editing: UseStateHandle<bool>,
    dirty: UseStateHandle<bool>,
    status: UseStateHandle<AttrValue>,
) -> Callback<editor::Applied> {
    let latest = use_mut_ref(|| drafts.clone());
    *latest.borrow_mut() = drafts;
    use_callback(
        (target, latest, editing, dirty, status),
        |applied: editor::Applied, (target, latest, editing, dirty, status)| {
            let Some(target) = target else {
                return;
            };
            let drafts = latest.borrow();
            if !drafts
                .image(&target.lane, target.quote.as_deref(), target.id)
                .is_some_and(|image| image.revision == applied.revision)
            {
                status
                    .set("This draft image changed or was removed. Reopen it to annotate.".into());
                dirty.set(false);
                editing.set(false);
                return;
            }
            drafts.dispatch(ComposerAction::ApplyImage {
                lane: target.lane.clone(),
                quote: target.quote.clone(),
                id: target.id,
                revision: applied.revision,
                media: applied.media,
                shapes: applied.shapes,
            });
            dirty.set(false);
            editing.set(false);
        },
    )
}
#[hook]
fn use_lane_navigation(
    props: &ViewerProps,
    editing: UseStateHandle<bool>,
) -> (bool, Callback<KeyboardEvent>) {
    let opener = props.selection.opener.clone();
    let available = lane_navigation_available(&opener);
    let navigate = use_callback(
        (opener, props.viewing.clone(), editing),
        |event: KeyboardEvent, (opener, viewing, editing)| {
            if **editing
                || event.is_composing()
                || event.alt_key()
                || event.ctrl_key()
                || event.meta_key()
            {
                return;
            }
            let direction = match event.key().as_str() {
                "ArrowLeft" => Direction::Older,
                "ArrowRight" => Direction::Newer,
                _ => return,
            };
            event.prevent_default();
            event.stop_propagation();
            if let Some(change) = adjacent_lane_media(opener, direction).and_then(open_change) {
                viewing.dispatch(change);
            }
        },
    );
    (available, navigate)
}

/// The lightbox's single-letter shortcuts: A annotates and D downloads by
/// activating the matching button, so each key does exactly what clicking
/// does and nothing when that button is absent or disabled. The editor keeps
/// its own letters, and typing in a field or with a modifier never triggers.
#[hook]
fn use_action_keys(host: NodeRef, editing: UseStateHandle<bool>) -> Callback<KeyboardEvent> {
    use_callback((host, editing), |event: KeyboardEvent, (host, editing)| {
        if **editing
            || event.repeat()
            || event.is_composing()
            || event.alt_key()
            || event.ctrl_key()
            || event.meta_key()
            || event.shift_key()
            || typing(&event)
        {
            return;
        }
        let selector = match event.key().as_str() {
            "a" | "A" => "[data-image-edit]",
            "d" | "D" => "[data-image-download]",
            _ => return,
        };
        let Some(button) = host
            .cast::<web_sys::HtmlDialogElement>()
            .and_then(|dialog| dialog.query_selector(selector).ok().flatten())
            .and_then(|button| button.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        button.click();
    })
}

/// The dialog's keyboard: lane navigation by arrows and the action letters.
/// Says whether lane navigation is offered, for its description.
#[hook]
fn use_viewer_keys(
    props: &ViewerProps,
    host: NodeRef,
    editing: UseStateHandle<bool>,
) -> (bool, Callback<KeyboardEvent>) {
    let (lane_navigation, navigate) = use_lane_navigation(props, editing.clone());
    let action_keys = use_action_keys(host, editing);
    let keydown = Callback::from(move |event: KeyboardEvent| {
        navigate.emit(event.clone());
        action_keys.emit(event);
    });
    (lane_navigation, keydown)
}

/// Whether the key lands in a field that takes typed text.
fn typing(event: &KeyboardEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| {
            target
                .closest(
                    "input, textarea, select, [contenteditable]:not([contenteditable=\"false\"])",
                )
                .ok()
                .flatten()
        })
        .is_some()
}

#[function_component(Viewer)]
fn viewer(props: &ViewerProps) -> Html {
    let host = use_node_ref();
    let image_status = use_state_eq(|| AttrValue::from("Loading image"));
    let editing = use_state_eq(|| false);
    let dirty = use_state_eq(|| false);
    let latest_dirty = use_mut_ref(|| false);
    *latest_dirty.borrow_mut() = *dirty;
    let image = props
        .selection
        .draft
        .as_ref()
        .and_then(|target| {
            props
                .drafts
                .image(&target.lane, target.quote.as_deref(), target.id)
        })
        .cloned();
    let source = use_memo(
        (
            props.selection.source.clone(),
            image.as_ref().map(|image| image.media.clone()),
        ),
        |(fallback, media)| {
            media.as_ref().map_or_else(
                || fallback.clone(),
                |media| AttrValue::from(media.data_url.clone()),
            )
        },
    );
    let ondirty = use_callback(dirty.clone(), |value: bool, dirty| dirty.set(value));
    let edit = use_callback((editing.clone(), dirty.clone()), |(), (editing, dirty)| {
        dirty.set(false);
        editing.set(true);
    });
    let cancel = use_callback(
        (editing.clone(), dirty.clone(), latest_dirty.clone()),
        |(), (editing, dirty, latest_dirty)| {
            if allow_annotation_discard(*latest_dirty.borrow()) {
                dirty.set(false);
                editing.set(false);
            }
        },
    );
    let apply = use_annotation_apply(
        props.selection.draft.clone(),
        props.drafts.clone(),
        editing.clone(),
        dirty.clone(),
        image_status.clone(),
    );
    let loaded = use_callback(image_status.clone(), |_: Event, status| {
        status.set(AttrValue::default())
    });
    let failed = use_callback(image_status.clone(), |_: Event, status| {
        status.set("Image unavailable".into())
    });
    let close = use_callback((host.clone(), latest_dirty), |(), (host, latest_dirty)| {
        if allow_annotation_discard(*latest_dirty.borrow())
            && let Some(dialog) = host.cast::<web_sys::HtmlDialogElement>()
        {
            dialog.close();
        }
    });
    let (lane_navigation, keydown) = use_viewer_keys(props, host.clone(), editing.clone());
    use_effect_with(
        (
            host.clone(),
            props.selection.clone(),
            props.viewing.clone(),
            close.clone(),
        ),
        |(host, selection, viewing, close)| {
            let dialog = host
                .cast::<web_sys::HtmlDialogElement>()
                .expect("the mounted viewer owns its dialog");
            let generation = selection.generation;
            let viewing = viewing.clone();
            let presentation = Presentation::show(
                dialog,
                selection.opener.clone(),
                Callback::from(move |()| viewing.dispatch(Change::Close(generation))),
                close.clone(),
            );
            move || drop(presentation)
        },
    );
    use_effect_with((host.clone(), *editing), |(host, editing)| {
        if !editing
            && let Some(dialog) = host.cast::<web_sys::HtmlDialogElement>()
            && dialog.open()
            && let Ok(Some(button)) = dialog.query_selector("[data-image-edit]")
            && let Some(button) = button.dyn_ref::<web_sys::HtmlElement>()
        {
            let _ = button.focus();
        }
    });
    html! {
        <dialog
            ref={host}
            class="image-lightbox app-modal"
            aria-label="Full-size image"
            aria-describedby={lane_navigation.then_some("image-lightbox-navigation-help")}
            onkeydown={keydown}
        >
            if lane_navigation {
                <span id="image-lightbox-navigation-help" class="visually-hidden">
                    {"Use Left Arrow for the previous older image in this lane and Right Arrow for the next newer image."}
                </span>
            }
            if *editing && let Some(image) = image {
                <editor::Editor {image} onapply={apply} oncancel={cancel} {ondirty} />
            } else {
                <actions::Actions source={(*source).clone()} name={props.selection.name.clone()} onclose={close} image_status={(*image_status).clone()} onedit={image.is_some().then_some(edit)} />
                <img class="image-lightbox-image" src={(*source).clone()} alt={props.selection.alt.clone()} onload={loaded} onerror={failed} />
            }
        </dialog>
    }
}

/// One modal showing and the listeners that request or finish dismissal.
/// Native cancel and backdrop requests pass through the current edit guard;
/// an opener that lapses must close immediately. The single `close` event
/// returns focus and clears the selection. Dropping the presentation detaches
/// every listener first, so teardown closes silently.
struct Presentation {
    dialog: web_sys::HtmlDialogElement,
    closed: Closure<dyn FnMut(web_sys::Event)>,
    cancelled: Closure<dyn FnMut(web_sys::Event)>,
    backdrop: Closure<dyn FnMut(web_sys::Event)>,
    _ownership: Ownership,
}

/// Closes the dialog as soon as its opener can no longer take focus back.
/// The opener's own box reports detachment and every kind of hiding; `inert`
/// changes nothing about layout, so ancestors' `inert` is watched separately.
struct Ownership {
    resized: web_sys::ResizeObserver,
    inerted: web_sys::MutationObserver,
    _lapsed: Closure<dyn FnMut()>,
}

impl Ownership {
    fn watch(dialog: web_sys::HtmlDialogElement, opener: web_sys::Element) -> Self {
        let document = opener.owner_document().expect("preview document");
        let observed = opener.clone();
        let lapsed = Closure::<dyn FnMut()>::new(move || {
            if !overlay::returnable(&observed) {
                dialog.close();
            }
        });
        let resized = web_sys::ResizeObserver::new(lapsed.as_ref().unchecked_ref())
            .expect("viewer opener box observer");
        resized.observe(&opener);
        let inerted = web_sys::MutationObserver::new(lapsed.as_ref().unchecked_ref())
            .expect("viewer opener inertness observer");
        let options = web_sys::MutationObserverInit::new();
        options.set_subtree(true);
        options.set_attribute_filter(&js_sys::Array::of1(&JsValue::from_str("inert")));
        inerted
            .observe_with_options(&document, &options)
            .expect("observe opener inertness");
        Self {
            resized,
            inerted,
            _lapsed: lapsed,
        }
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        self.resized.disconnect();
        self.inerted.disconnect();
    }
}

impl Presentation {
    fn show(
        dialog: web_sys::HtmlDialogElement,
        opener: web_sys::Element,
        dismissed: Callback<()>,
        requested: Callback<()>,
    ) -> Self {
        let returned = opener.clone();
        let closed = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            overlay::return_focus(&returned);
            dismissed.emit(());
        });
        let cancel_request = requested.clone();
        let cancelled = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            event.prevent_default();
            cancel_request.emit(());
        });
        let backdrop = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            if event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlDialogElement>().ok())
                .is_some()
            {
                requested.emit(());
            }
        });
        dialog
            .add_event_listener_with_callback("close", closed.as_ref().unchecked_ref())
            .expect("dialog close listener");
        dialog
            .add_event_listener_with_callback("cancel", cancelled.as_ref().unchecked_ref())
            .expect("dialog cancel listener");
        dialog
            .add_event_listener_with_callback("click", backdrop.as_ref().unchecked_ref())
            .expect("dialog backdrop listener");
        let ownership = Ownership::watch(dialog.clone(), opener.clone());
        // Focus returns only to an eligible opener, never to whatever the
        // browser remembers as focused before the modal opened.
        if let Some(active) = opener
            .owner_document()
            .and_then(|document| document.active_element())
            .and_then(|active| active.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = active.blur();
        }
        dialog
            .show_modal()
            .expect("the connected viewer opens in the top layer");
        Self {
            dialog,
            closed,
            cancelled,
            backdrop,
            _ownership: ownership,
        }
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        let _ = self
            .dialog
            .remove_event_listener_with_callback("close", self.closed.as_ref().unchecked_ref());
        let _ = self
            .dialog
            .remove_event_listener_with_callback("cancel", self.cancelled.as_ref().unchecked_ref());
        let _ = self
            .dialog
            .remove_event_listener_with_callback("click", self.backdrop.as_ref().unchecked_ref());
        self.dialog.close();
    }
}
