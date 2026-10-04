//! Lifts each quote's trailing images to the end of the prose they flow beside.
//!
//! The renderer leads a quote layer that ends in images with those images,
//! floated beneath a zero-width anchor float (`composer_markdown::image_flow`).
//! Where the images belong depends on how the prose wraps around them, which
//! depends on where they are, so no stylesheet can resolve it: the anchor's
//! height is measured here instead. The images' bottom meets the prose's and is
//! never lower; prose shorter than the images sits beside their top. A layer
//! too narrow to share a line with its images stacks them full width under the
//! prose. The layer's attribution leads it, so it never takes part.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use web_sys::{Element, HtmlElement, ResizeObserver};
use yew::prelude::*;

const FLOW: &str = "message-image-flow";
/// The narrowest prose column worth wrapping beside the images.
const PROSE_FLOOR: &str = "14ch";
/// Lift passes before settling on the highest lift whose images end no lower
/// than the prose; wrapping beside the images rarely needs more than two.
const SETTLE_PASSES: usize = 4;

#[hook]
pub(super) fn use_quote_image_flow(host: NodeRef, html: AttrValue) {
    use_effect_with((host, html), |(host, _)| {
        let observation = host
            .cast::<Element>()
            .and_then(|host| Observation::new(&host));
        move || drop(observation)
    });
}

struct Observation {
    observer: ResizeObserver,
    _settle: Closure<dyn FnMut(js_sys::Array, ResizeObserver)>,
}

impl Observation {
    fn new(host: &Element) -> Option<Self> {
        let flows = collect_flows(host);
        if flows.is_empty() {
            return None;
        }
        let host = host.clone();
        let settle = Closure::<dyn FnMut(js_sys::Array, ResizeObserver)>::new(move |_, _| {
            settle_flows(&host);
        });
        let observer = ResizeObserver::new(settle.as_ref().unchecked_ref()).ok()?;
        for flow in &flows {
            observer.observe(flow);
        }
        Some(Self {
            observer,
            _settle: settle,
        })
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

/// Settle every image flow under `host` at its current width, innermost first
/// because an outer flow measures prose holding inner ones.
pub(super) fn settle_flows(host: &Element) {
    let settled = collect_flows(host);
    for flow in settled.iter().rev() {
        settle_batch(std::iter::once(flow));
    }
}

/// Find image flows once when a measurement clone is created. Its descendants
/// are stable for the clone's lifetime even as the clone's span changes.
pub(super) fn collect_flows(host: &Element) -> Vec<HtmlElement> {
    let Ok(nodes) = host.query_selector_all(&format!(".{FLOW}")) else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index)?.dyn_into::<HtmlElement>().ok())
        .collect()
}
/// Card keys whose rendered content contains an image flow. Discovering these
/// once on the live host avoids a subtree query for every measurement clone.
pub(super) fn flow_host_keys(host: &Element) -> std::collections::HashSet<String> {
    let Ok(nodes) = host.query_selector_all(&format!(".{FLOW}")) else {
        return std::collections::HashSet::new();
    };
    (0..nodes.length())
        .filter_map(|index| {
            nodes
                .item(index)?
                .dyn_into::<Element>()
                .ok()?
                .closest("[data-mosaic-card-key]")
                .ok()??
                .get_attribute("data-mosaic-card-key")
        })
        .collect()
}

/// Settle previously collected measurement-clone flows without forcing a
/// separate document layout for every card. Each host is independent, so flows
/// at the same reverse document-order tier can share each read/write pass.
pub(super) fn settle_collected_flows<'a>(
    flow_sets: impl Iterator<Item = &'a [HtmlElement]> + Clone,
) {
    let max_flows = flow_sets
        .clone()
        .map(<[HtmlElement]>::len)
        .max()
        .unwrap_or(0);
    for index in (0..max_flows).rev() {
        settle_batch(flow_sets.clone().filter_map(|flows| flows.get(index)));
    }
}

fn has_class(element: &Element, class: &str) -> bool {
    element
        .get_attribute("class")
        .is_some_and(|classes| classes.split_whitespace().any(|name| name == class))
}

fn child(flow: &HtmlElement, class: &str) -> Option<HtmlElement> {
    let children = flow.children();
    (0..children.length())
        .filter_map(|index| children.item(index))
        .find(|element| has_class(element, class))?
        .dyn_into::<HtmlElement>()
        .ok()
}

struct FlowContext {
    flow: HtmlElement,
    anchor: HtmlElement,
    stack: HtmlElement,
    prose: Element,
}

struct PreparedFlow {
    context: FlowContext,
    width: f64,
    image_width: f64,
    image_height: f64,
    gap: f64,
    floor: f64,
    narrow: bool,
    stacked_height: Option<f64>,
    current: f64,
    best: f64,
    closed: f64,
    settled: bool,
}

fn prepared_flow(flow: &HtmlElement) -> Option<PreparedFlow> {
    Some(PreparedFlow {
        context: FlowContext {
            flow: flow.clone(),
            anchor: child(flow, "message-image-flow-anchor")?,
            stack: child(flow, "message-image-stack")?,
            prose: flow.last_element_child()?,
        },
        width: 0.0,
        image_width: 0.0,
        image_height: 0.0,
        gap: 0.0,
        floor: 0.0,
        narrow: false,
        stacked_height: None,
        current: 0.0,
        best: 0.0,
        closed: 0.0,
        settled: false,
    })
}

fn lift(context: &FlowContext, height: f64) {
    let _ = context
        .anchor
        .style()
        .set_property("height", &format!("{height}px"));
}

fn end(context: &FlowContext) -> f64 {
    context.prose.get_bounding_client_rect().bottom()
        - context.flow.get_bounding_client_rect().top()
}

fn image_gap(stack: &HtmlElement) -> Option<f64> {
    Some(
        web_sys::window()?
            .get_computed_style(stack)
            .ok()??
            .get_property_value("margin-right")
            .ok()?
            .trim_end_matches("px")
            .parse::<f64>()
            .unwrap_or(0.0),
    )
}

fn settle_batch<'a>(flows: impl IntoIterator<Item = &'a HtmlElement>) {
    let mut prepared = flows
        .into_iter()
        .filter_map(prepared_flow)
        .collect::<Vec<_>>();
    for flow in &prepared {
        lift(&flow.context, 0.0);
        let _ = flow.context.flow.remove_attribute("data-image-flow");
    }
    prepared.retain_mut(|flow| {
        let width = f64::from(flow.context.flow.client_width());
        if width == 0.0 {
            return false;
        }
        let images = flow.context.stack.get_bounding_client_rect();
        let Some(gap) = image_gap(&flow.context.stack) else {
            return false;
        };
        flow.width = width;
        flow.image_width = images.width();
        flow.image_height = images.height();
        flow.gap = gap;
        true
    });

    for flow in &prepared {
        let _ = flow
            .context
            .anchor
            .style()
            .set_property("width", PROSE_FLOOR);
    }
    for flow in &mut prepared {
        flow.floor = flow.context.anchor.get_bounding_client_rect().width();
    }
    for flow in &prepared {
        let _ = flow.context.anchor.style().remove_property("width");
    }

    for flow in &mut prepared {
        flow.narrow = flow.width - flow.image_width - flow.gap < flow.floor;
        if flow.narrow {
            let _ = flow
                .context
                .flow
                .set_attribute("data-image-flow", "stacked");
            lift(&flow.context, f64::from(u16::MAX));
        }
    }
    for flow in &mut prepared {
        if flow.narrow {
            flow.stacked_height = Some(end(&flow.context).max(0.0).ceil());
        }
    }
    for flow in &prepared {
        if let Some(height) = flow.stacked_height {
            lift(&flow.context, height);
        }
    }

    for _ in 0..SETTLE_PASSES {
        for flow in &mut prepared {
            if !flow.narrow && !flow.settled {
                flow.closed = end(&flow.context);
            }
        }
        for flow in &mut prepared {
            if flow.narrow || flow.settled {
                continue;
            }
            if flow.current + flow.image_height <= flow.closed + 0.5 {
                flow.best = flow.best.max(flow.current);
            }
            let next = (flow.closed - flow.image_height).max(0.0).floor();
            if (next - flow.current).abs() < 0.5 {
                flow.settled = true;
            } else {
                flow.current = next;
                lift(&flow.context, flow.current);
            }
        }
    }
    for flow in &prepared {
        if !flow.narrow {
            lift(&flow.context, flow.best);
        }
    }
}
