//! Lazy, page-owned Mermaid rendering and its image-only display boundary.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use yew::prelude::*;

mod frame;
mod service;

use super::image_viewer::use_button;
use service::{Service, Shared};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn read() -> Result<Self, AttrValue> {
        let theme = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element())
            .and_then(|root| root.get_attribute("data-canvas-effective-theme"));
        match theme.as_deref() {
            Some("light") => Ok(Self::Light),
            Some("dark") => Ok(Self::Dark),
            _ => Err("Canvas effective theme is unavailable".into()),
        }
    }
}

#[derive(Clone)]
struct Context {
    service: Rc<Result<Shared, AttrValue>>,
    theme: Result<Theme, AttrValue>,
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.service, &other.service) && self.theme == other.theme
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct ProviderProps {
    pub(super) children: Children,
}

#[function_component(Provider)]
pub(super) fn provider(props: &ProviderProps) -> Html {
    let service = use_memo((), |()| {
        super::connection::loaded_canvas_generation()
            .and_then(Service::new)
            .map_err(AttrValue::from)
    });
    let theme = use_state_eq(Theme::read);
    use_effect_with((), {
        let theme = theme.clone();
        move |()| {
            let observation = ThemeObservation::new(theme.clone());
            if observation.is_err() {
                theme.set(Err("Could not observe the Canvas theme".into()));
            }
            move || drop(observation)
        }
    });
    use_effect_with((), {
        let service = service.clone();
        let theme = theme.clone();
        move |()| {
            let registration = page_hide(service.clone());
            if registration.is_err() {
                theme.set(Err("Could not own the diagram renderer lifecycle".into()));
            }
            move || {
                if let Ok(cleanup) = registration {
                    cleanup();
                }
                if let Ok(service) = service.as_ref() {
                    Service::close(service, "Diagram renderer unmounted");
                }
            }
        }
    });
    let context = Context {
        service,
        theme: (*theme).clone(),
    };
    html! { <ContextProvider<Context> {context}>{props.children.clone()}</ContextProvider<Context>> }
}

struct ThemeObservation {
    observer: web_sys::MutationObserver,
    _callback: Closure<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
}

impl ThemeObservation {
    fn new(theme: UseStateHandle<Result<Theme, AttrValue>>) -> Result<Self, JsValue> {
        let root = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.document_element())
            .ok_or("No Canvas root")?;
        let callback = Closure::new(move |_: js_sys::Array, _: web_sys::MutationObserver| {
            theme.set(Theme::read());
        });
        let observer = web_sys::MutationObserver::new(callback.as_ref().unchecked_ref())?;
        let observation = Self {
            observer,
            _callback: callback,
        };
        let options = web_sys::MutationObserverInit::new();
        options.set_attributes(true);
        options.set_attribute_filter(&js_sys::Array::of1(&JsValue::from_str(
            "data-canvas-effective-theme",
        )));
        observation.observer.observe_with_options(&root, &options)?;
        Ok(observation)
    }
}

impl Drop for ThemeObservation {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

fn page_hide(service: Rc<Result<Shared, AttrValue>>) -> Result<impl FnOnce(), JsValue> {
    let window = web_sys::window().ok_or("No browser window")?;
    let callback: Closure<dyn FnMut(web_sys::Event)> = Closure::new(move |_: web_sys::Event| {
        if let Ok(service) = service.as_ref() {
            Service::close(service, "Diagram renderer interrupted; retry to resume");
        }
    });
    window.add_event_listener_with_callback("pagehide", callback.as_ref().unchecked_ref())?;
    Ok(move || {
        let _ = window
            .remove_event_listener_with_callback("pagehide", callback.as_ref().unchecked_ref());
    })
}

#[derive(Clone, Default)]
struct Display {
    revision: AttrValue,
    theme: Option<Theme>,
    image: Option<AttrValue>,
    error: Option<AttrValue>,
    pending: bool,
}

#[derive(Properties, PartialEq)]
pub(super) struct MermaidImageProps {
    pub(super) source: AttrValue,
    #[prop_or_default]
    pub(super) node_ids: Rc<Vec<String>>,
    #[prop_or_default]
    pub(super) on_geometry: Callback<Option<Rc<crate::mermaid_geometry::Geometry>>>,
    #[prop_or(true)]
    pub(super) final_answer: bool,
}

struct RenderSubscription {
    context: Context,
    host: NodeRef,
    source: AttrValue,
    node_ids: Rc<Vec<String>>,
    on_geometry: Callback<Option<Rc<crate::mermaid_geometry::Geometry>>>,
    revision: Rc<AttrValue>,
    current_theme: Option<Theme>,
    previous: Option<AttrValue>,
    display: UseStateHandle<Display>,
}

#[hook]
fn use_render(subscription: RenderSubscription) {
    let RenderSubscription {
        context,
        host,
        source,
        node_ids,
        on_geometry,
        revision,
        current_theme,
        previous,
        display,
    } = subscription;
    use_effect_with((revision, context, on_geometry), {
        move |(revision, context, on_geometry)| {
            let pending = Display {
                revision: revision.as_ref().clone(),
                theme: current_theme,
                image: previous.clone(),
                error: None,
                pending: true,
            };
            display.set(pending.clone());
            on_geometry.emit(None);
            let on_result = on_geometry.clone();
            let callback = Callback::from(move |outcome: service::Outcome| {
                let mut next = pending.clone();
                next.pending = false;
                match outcome {
                    Ok(render) => {
                        next.image = Some(render.image);
                        on_result.emit(Some(render.geometry));
                    }
                    Err(error) => next.error = Some(error),
                }
                display.set(next);
            });
            let request = (|| {
                let service = context.service.as_ref().as_ref().map_err(Clone::clone)?;
                let theme = context.theme.clone()?;
                let host = host
                    .cast::<web_sys::Element>()
                    .ok_or_else(|| AttrValue::from("Diagram card is detached"))?;
                Service::request(
                    service,
                    source,
                    theme,
                    node_ids,
                    host,
                    revision.as_ref().clone(),
                    callback.clone(),
                )
            })();
            if let Err(error) = &request {
                callback.emit(Err(error.clone()));
            }
            move || drop(request)
        }
    });
}

/// Owned consumer: dropping/changing this component cancels its subscription.
/// The original source remains text, including after parse/load/resource errors.
#[function_component(MermaidImage)]
pub(super) fn mermaid_image(props: &MermaidImageProps) -> Html {
    let context = use_context::<Context>().expect("Canvas diagram provider");
    let host = use_node_ref();
    let display = use_state(Display::default);
    let retry = use_state(|| 0_u32);
    let show_source = use_state_eq(|| false);
    let counter = use_memo((), |()| Cell::new(0_u64));
    let revision = use_memo(
        (
            props.source.clone(),
            context.theme.clone(),
            props.node_ids.clone(),
            *retry,
        ),
        move |_| {
            let next = counter.get().wrapping_add(1);
            counter.set(next);
            AttrValue::from(next.to_string())
        },
    );
    let current_theme = context.theme.as_ref().ok().copied();
    let previous = (display.theme == current_theme)
        .then(|| display.image.clone())
        .flatten();
    use_render(RenderSubscription {
        context: context.clone(),
        host: host.clone(),
        source: props.source.clone(),
        node_ids: props.node_ids.clone(),
        on_geometry: props.on_geometry.clone(),
        revision: revision.clone(),
        current_theme,
        previous: previous.clone(),
        display: display.clone(),
    });
    let pending = display.revision != *revision || display.pending;
    let error = (!pending).then(|| display.error.clone()).flatten();
    let state = if pending {
        "pending"
    } else if error.is_some() {
        "error"
    } else {
        "ready"
    };
    let status = if pending {
        AttrValue::from("Rendering diagram")
    } else if let Some(error) = &error {
        if props.final_answer {
            error.clone()
        } else {
            "Waiting for a complete diagram".into()
        }
    } else {
        "Diagram rendered".into()
    };
    let retry_action = use_callback(retry, |(), retry| {
        retry.set((**retry).wrapping_add(1));
    });
    let retry_button = use_button(retry_action);
    let toggle_source = use_callback(show_source.clone(), |(), shown| {
        shown.set(!**shown);
    });
    let source_button = use_button(toggle_source);
    let source_action = if *show_source {
        "Show diagram preview"
    } else {
        "Show diagram source"
    };
    html! {
        <div ref={host} class="mermaid-render" data-mermaid-revision={revision.as_ref().clone()} data-mermaid-state={state} data-source-visible={show_source.to_string()}>
            <button type="button" class="message-image message-image--openable" data-image-preview="" data-image-name="diagram.svg" disabled={previous.is_none()} aria-label="View full-size diagram" title="View full-size diagram">
                if let Some(image) = previous {
                    <img src={image} alt="Mermaid diagram" data-mosaic-image-state="loaded" />
                }
                <span class="message-image-status" role="status" data-mermaid-status={if error.is_some() && !props.final_answer { "pending" } else { state }}>{status}</span>
            </button>
            <button ref={source_button} type="button" class="mermaid-source-toggle" aria-label={source_action} title={source_action} aria-pressed={show_source.to_string()}>
                {if *show_source { "×" } else { "</>" }}
            </button>
            <div class="mermaid-source-body" hidden={!*show_source}>
                if let Some(error) = error { <p>{error}</p> }
                <pre><code>{props.source.clone()}</code></pre>
                <button ref={retry_button} type="button">{"Retry diagram"}</button>
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct MarkdownBodyProps {
    pub(super) html: AttrValue,
    pub(super) sources: Rc<Vec<AttrValue>>,
    pub(super) final_answer: bool,
}

struct Slot {
    index: usize,
    target: web_sys::Element,
    key: yew::virtual_dom::Key,
}

/// Only producer-authored slots inside this card are portal targets. Raw user
/// HTML still passes through the shared Markdown sanitizer before this point.
#[function_component(MarkdownBody)]
pub(super) fn markdown_body(props: &MarkdownBodyProps) -> Html {
    let host = use_node_ref();
    let slots = use_state(Vec::<Slot>::new);
    let counter = use_memo((), |()| Cell::new(0_u64));
    use_effect_with((props.html.clone(), props.sources.clone()), {
        let host = host.clone();
        let slots = slots.clone();
        let counter = counter.clone();
        move |(_, sources)| {
            let mut targets = Vec::new();
            if !sources.is_empty()
                && let Some(host) = host.cast::<web_sys::Element>()
                && let Ok(nodes) = host.query_selector_all("[data-mermaid-slot]")
            {
                for index in 0..nodes.length() {
                    let Some(element) = nodes
                        .item(index)
                        .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                    else {
                        continue;
                    };
                    let Some(index) = element
                        .get_attribute("data-mermaid-slot")
                        .and_then(|index| index.parse::<usize>().ok())
                    else {
                        continue;
                    };
                    if index < sources.len() {
                        // A changed host needs a new portal event root;
                        // unchanged hosts retain streaming preview state.
                        let key = slots
                            .iter()
                            .find(|slot| slot.target == element)
                            .map(|slot| slot.key.clone())
                            .unwrap_or_else(|| {
                                counter.set(counter.get().wrapping_add(1));
                                counter.get().into()
                            });
                        targets.push(Slot {
                            index,
                            target: element,
                            key,
                        });
                    }
                }
            }
            slots.set(targets);
        }
    });
    // Keep the portal list under a stable VDOM parent. Mixing keyed portals
    // directly with VRaw replaces the raw nodes when the first portal mounts.
    html! {
        <div ref={host} class="message-body markdown-body">
            {Html::from_html_unchecked(props.html.clone())}
            if !props.sources.is_empty() {
            <span hidden=true>
            {for slots.iter().filter_map(|slot| {
                let source = props.sources.get(slot.index)?.clone();
                Some(yew::create_portal(html! {
                    <MermaidImage key={slot.key.clone()} {source} final_answer={props.final_answer} />
                }, slot.target.clone()))
            })}
            </span>
            }
        </div>
    }
}
