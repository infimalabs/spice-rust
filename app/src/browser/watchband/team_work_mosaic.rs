//! Measured packing for the team-local Work card collections.
//!
//! CSS owns the responsive column policy. This bridge reads those resolved
//! columns, measures every rendered card, and feeds the heights through the
//! same pure Mosaic packer used by team activity. ResizeObserver covers width,
//! text, task-detail, and media changes without coupling sibling row heights.

use super::*;
const PACK_SUBPIXELS: f64 = 1024.0;

#[derive(Properties, PartialEq)]
pub(super) struct Props {
    #[prop_or_default]
    pub(super) children: Children,
    #[prop_or_default]
    pub(super) labelledby: Option<AttrValue>,
}

#[function_component(DirectionMosaic)]
pub(super) fn direction_mosaic(props: &Props) -> Html {
    let host = use_node_ref();
    use_work_mosaic(host.clone());
    html! {
        <ol ref={host} class="team-direction-ledger" role="feed" aria-labelledby={props.labelledby.clone()}>
            {for props.children.iter()}
        </ol>
    }
}

#[function_component(SettledMosaic)]
pub(super) fn settled_mosaic(props: &Props) -> Html {
    let host = use_node_ref();
    use_work_mosaic(host.clone());
    html! {
        <div ref={host} class="team-task-card-list work-history-list">
            {for props.children.iter()}
        </div>
    }
}

#[hook]
fn use_work_mosaic(host: NodeRef) {
    use_effect(move || {
        let registration = host.cast::<web_sys::HtmlElement>().and_then(|host| {
            pack(&host);
            let observed = host.clone();
            let callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(move |_, _| {
                    pack(&observed)
                });
            let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
            observer.observe(&host);
            for card in cards(&host) {
                observer.observe(&card);
            }
            Some((observer, callback))
        });
        move || {
            if let Some((observer, _callback)) = registration {
                observer.disconnect();
            }
        }
    });
}

fn cards(host: &web_sys::HtmlElement) -> Vec<web_sys::HtmlElement> {
    let Ok(nodes) = host.query_selector_all(":scope > *") else {
        return Vec::new();
    };
    (0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

fn css_pixels(value: &str) -> Option<f64> {
    value.trim().strip_suffix("px")?.parse().ok()
}

fn resolved_columns(host: &web_sys::HtmlElement) -> Option<(usize, f64)> {
    let style = web_sys::window()?.get_computed_style(host).ok()??;
    let tracks = style.get_property_value("grid-template-columns").ok()?;
    let columns = tracks
        .split_ascii_whitespace()
        .filter_map(css_pixels)
        .filter(|width| *width > 0.0)
        .count()
        .max(1);
    let gap = style
        .get_property_value("column-gap")
        .ok()
        .as_deref()
        .and_then(css_pixels)
        .unwrap_or(0.0);
    Some((columns, gap))
}

fn set_pixels(
    style: &web_sys::CssStyleDeclaration,
    property: &str,
    value: f64,
) -> Result<(), wasm_bindgen::JsValue> {
    style.set_property(property, &format!("{value:.3}px"))
}

fn pack(host: &web_sys::HtmlElement) {
    let items = cards(host);
    if items.is_empty() {
        let _ = host.style().remove_property("height");
        let _ = host.set_attribute("data-mosaic-packing", "empty");
        return;
    }
    let Some((columns, gap)) = resolved_columns(host) else {
        return;
    };
    let width = f64::from(host.client_width());
    if width <= 0.0 {
        return;
    }
    let column_width =
        ((width - gap * (columns.saturating_sub(1) as f64)) / columns as f64).max(1.0);

    for item in &items {
        let style = item.style();
        let _ = style.set_property("position", "absolute");
        let _ = set_pixels(&style, "width", column_width);
    }

    let heights = items
        .iter()
        .map(|item| item.get_bounding_client_rect().height())
        .collect::<Vec<_>>();
    let candidates = heights
        .iter()
        .map(|height| {
            vec![crate::mosaic::Candidate {
                span: 1,
                rows: ((height + gap) * PACK_SUBPIXELS).ceil().max(1.0) as i32,
            }]
        })
        .collect::<Vec<_>>();
    let placements = crate::mosaic::pack(&candidates, columns);
    let mut extent = 0.0_f64;
    for ((item, height), placement) in items.iter().zip(heights).zip(placements) {
        let left = placement.track as f64 * (column_width + gap);
        let top = f64::from(placement.base) / PACK_SUBPIXELS;
        let style = item.style();
        let _ = set_pixels(&style, "left", left);
        let _ = set_pixels(&style, "top", top);
        extent = extent.max(top + height);
    }
    let _ = set_pixels(&host.style(), "height", extent);
    let _ = host.set_attribute("data-mosaic-packing", "ready");
    let _ = host.set_attribute("data-mosaic-columns", &columns.to_string());
}
