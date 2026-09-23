//! Trusted controls aligned to validated inert Mermaid geometry.

use std::rc::Rc;

use wasm_bindgen::{JsCast, closure::Closure};
use yew::prelude::*;

use wire::TaskStamp;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MapNodeTarget {
    pub(super) id: String,
    pub(super) task: TaskStamp,
    pub(super) label: String,
    pub(super) current: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ViewportSize {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ViewportTransform {
    pub(super) zoom: u16,
    pub(super) pan_x: i16,
    pub(super) pan_y: i16,
}

#[derive(Properties, PartialEq)]
struct GeometryOverlayProps {
    geometry: Rc<crate::mermaid_geometry::Geometry>,
    targets: Rc<Vec<MapNodeTarget>>,
    select: Callback<TaskStamp>,
    viewport: ViewportTransform,
}

struct SizeObservation {
    observer: web_sys::ResizeObserver,
    _callback: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
}

impl Drop for SizeObservation {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

#[function_component(GeometryOverlay)]
fn geometry_overlay(props: &GeometryOverlayProps) -> Html {
    let host = use_node_ref();
    let size = use_state(ViewportSize::default);
    use_effect_with(host.clone(), {
        let size = size.clone();
        move |host| {
            let registration = observe_size(host, size);
            move || drop(registration)
        }
    });
    use_effect_with(
        (host.clone(), *size, props.geometry.clone(), props.viewport),
        move |(host, _, _, viewport)| apply_overlay_layout(host, *viewport),
    );
    let view = props.geometry.view_box;
    let scale = (f64::from(size.width) / view.width)
        .min(f64::from(size.height) / view.height)
        .max(0.0);
    let drawn_width = view.width * scale;
    let drawn_height = view.height * scale;
    let offset_x = (f64::from(size.width) - drawn_width) / 2.0;
    let offset_y = (f64::from(size.height) - drawn_height) / 2.0;
    let nodes = if size.width > 0 && size.height > 0 {
        props.geometry.nodes.as_slice()
    } else {
        &[]
    };
    html! {
        <div
            ref={host}
            class="work-map-node-layer"
            role="group"
            aria-label="Interactive task nodes"
            data-map-overlay-width={size.width.to_string()}
            data-map-overlay-height={size.height.to_string()}
        >
            {for nodes.iter().filter_map(|node| {
                let target = props.targets.iter().find(|target| target.id == node.id)?;
                let activate = {
                    let select = props.select.clone();
                    let task = target.task.clone();
                    Callback::from(move |_| select.emit(task.clone()))
                };
                let label = format!("Open {} in Work detail", target.label);
                let left = offset_x + (node.rect.x - view.x) * scale;
                let top = offset_y + (node.rect.y - view.y) * scale;
                let width = node.rect.width * scale;
                let height = node.rect.height * scale;
                Some(html! {
                    <button
                        key={node.id.clone()}
                        type="button"
                        class={classes!("work-map-node-button", target.current.then_some("current"))}
                        data-map-left={left.to_string()}
                        data-map-top={top.to_string()}
                        data-map-width={width.to_string()}
                        data-map-height={height.to_string()}
                        data-map-node-id={node.id.clone()}
                        data-task-stamp={target.task.to_string()}
                        aria-label={label.clone()}
                        title={label}
                        aria-current={target.current.then_some("true")}
                        onclick={activate}
                    ></button>
                })
            })}
        </div>
    }
}

fn observe_size(host: &NodeRef, size: UseStateHandle<ViewportSize>) -> Option<SizeObservation> {
    let element = host.cast::<web_sys::Element>()?;
    publish_size(&element, &size);
    let observed = element.clone();
    let callback = Closure::new(move |_: js_sys::Array, _: web_sys::ResizeObserver| {
        publish_size(&observed, &size);
    });
    let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
    observer.observe(&element);
    Some(SizeObservation {
        observer,
        _callback: callback,
    })
}

fn publish_size(element: &web_sys::Element, size: &UseStateHandle<ViewportSize>) {
    let next = ViewportSize {
        width: element.client_width(),
        height: element.client_height(),
    };
    if **size != next {
        size.set(next);
    }
}

fn apply_overlay_layout(host: &NodeRef, viewport: ViewportTransform) {
    let Some(layer) = host.cast::<web_sys::HtmlElement>() else {
        return;
    };
    let transform = format!(
        "translate({}px, {}px) scale({})",
        viewport.pan_x,
        viewport.pan_y,
        f64::from(viewport.zoom) / 100.0,
    );
    layer
        .style()
        .set_property("transform", &transform)
        .expect("Work map transform property");
    if let Some(image) = layer
        .parent_element()
        .and_then(|parent| {
            parent
                .query_selector(":scope > .message-image")
                .ok()
                .flatten()
        })
        .and_then(|image| image.dyn_into::<web_sys::HtmlElement>().ok())
    {
        image
            .style()
            .set_property("transform", &transform)
            .expect("Work map image transform property");
    }
    let buttons = layer
        .query_selector_all(".work-map-node-button")
        .expect("the fixed Work map node selector is valid");
    for index in 0..buttons.length() {
        let button = buttons
            .item(index)
            .expect("the Work map node list retains every indexed node")
            .dyn_into::<web_sys::HtmlElement>()
            .expect("a Work map node control is an HTML element");
        for (property, attribute) in [
            ("left", "data-map-left"),
            ("top", "data-map-top"),
            ("width", "data-map-width"),
            ("height", "data-map-height"),
        ] {
            let value = format!(
                "{}px",
                button
                    .get_attribute(attribute)
                    .expect("Work map geometry provides every layout attribute"),
            );
            button
                .style()
                .set_property(property, &value)
                .unwrap_or_else(|_| panic!("Work map layout writes {property}"));
        }
    }
    layer
        .set_attribute("data-map-overlay-ready", "true")
        .expect("the fixed Work map readiness attribute is valid");
}

pub(super) fn renderer(
    targets: Rc<Vec<MapNodeTarget>>,
    select: Callback<TaskStamp>,
    viewport: ViewportTransform,
) -> Callback<Rc<crate::mermaid_geometry::Geometry>, Html> {
    Callback::from(move |geometry: Rc<crate::mermaid_geometry::Geometry>| {
        html! {
            <GeometryOverlay
                {geometry}
                targets={targets.clone()}
                select={select.clone()}
                {viewport}
            />
        }
    })
}
