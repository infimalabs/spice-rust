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
        let flows = flows(host);
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
/// because an outer flow measures prose holding inner ones. Mosaic measurement
/// clones call this at each width they measure, so a card's measured height
/// never depends on whether the live card's observer already ran, or at which
/// width.
pub(super) fn settle_flows(host: &Element) {
    for flow in flows(host).iter().rev() {
        settle(flow);
    }
}

fn flows(host: &Element) -> Vec<HtmlElement> {
    let Ok(nodes) = host.query_selector_all(&format!(".{FLOW}")) else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index)?.dyn_into::<HtmlElement>().ok())
        .collect()
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

fn settle(flow: &HtmlElement) -> Option<()> {
    let anchor = child(flow, "message-image-flow-anchor")?;
    let stack = child(flow, "message-image-stack")?;
    let prose = flow.last_element_child()?;
    let lift = |height: f64| {
        let _ = anchor
            .style()
            .set_property("height", &format!("{height}px"));
    };
    lift(0.0);
    let _ = flow.remove_attribute("data-image-flow");
    let width = f64::from(flow.client_width());
    if width == 0.0 {
        return None;
    }
    let end = |element: &Element| {
        element.get_bounding_client_rect().bottom() - flow.get_bounding_client_rect().top()
    };
    let images = stack.get_bounding_client_rect();
    let gap = web_sys::window()?
        .get_computed_style(&stack)
        .ok()??
        .get_property_value("margin-right")
        .ok()?
        .trim_end_matches("px")
        .parse::<f64>()
        .unwrap_or(0.0);
    let _ = anchor.style().set_property("width", PROSE_FLOOR);
    let floor = anchor.get_bounding_client_rect().width();
    let _ = anchor.style().remove_property("width");
    if width - images.width() - gap < floor {
        // Too narrow to share lines: the images follow the prose, measured
        // with the anchor holding them out of its way.
        let _ = flow.set_attribute("data-image-flow", "stacked");
        lift(f64::from(u16::MAX));
        lift(end(&prose).max(0.0).ceil());
        return Some(());
    }
    let height = images.height();
    let (mut current, mut best) = (0.0_f64, 0.0_f64);
    for _ in 0..SETTLE_PASSES {
        let closed = end(&prose);
        if current + height <= closed + 0.5 {
            best = best.max(current);
        }
        let next = (closed - height).max(0.0).floor();
        if (next - current).abs() < 0.5 {
            break;
        }
        current = next;
        lift(current);
    }
    lift(best);
    Some(())
}
