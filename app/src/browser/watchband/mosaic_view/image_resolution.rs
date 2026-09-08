//! Image lifecycle, full-size data-image opening, and host width observation.

use super::*;

use gloo_timers::callback::Timeout;

fn resolve_image(image: web_sys::HtmlImageElement) {
    let outcome = if image.natural_width() > 0 {
        "loaded"
    } else {
        "error"
    };
    set_image_resolution_state(&image, outcome);
}

fn scan_complete_images(host: &web_sys::Element) {
    let Ok(images) = host.query_selector_all("img") else {
        return;
    };
    for index in 0..images.length() {
        let Some(image) = images
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::HtmlImageElement>().ok())
        else {
            continue;
        };
        if image.complete() {
            resolve_image(image);
        }
    }
}

fn set_image_resolution_state(image: &web_sys::HtmlImageElement, outcome: &str) {
    let _ = image.set_attribute("data-mosaic-image-state", outcome);
    if let Some(status) = image
        .closest(".message-image")
        .ok()
        .flatten()
        .and_then(|preview| {
            preview
                .query_selector("[data-mosaic-image-status]")
                .ok()
                .flatten()
        })
    {
        let text = match outcome {
            "loaded" => "Image loaded",
            "error" => "Image unavailable",
            _ => "Loading image",
        };
        let _ = status.set_attribute("data-mosaic-image-status", outcome);
        status.set_text_content(Some(text));
    }
}

#[hook]
pub(super) fn use_image_resolution_events(host: NodeRef, cards: Vec<ActivityCard>) {
    let image_scan = cards
        .iter()
        .map(|activity| (activity.key(), activity.measurement_fingerprint()))
        .collect::<Vec<_>>();
    use_effect_with((host, image_scan), move |(host, _)| {
        let registration = host.cast::<web_sys::Element>().and_then(|host| {
            let resolved =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let Some(image) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlImageElement>().ok())
                    else {
                        return;
                    };
                    resolve_image(image);
                });
            let callback = resolved.as_ref().unchecked_ref();
            host.add_event_listener_with_callback_and_bool("load", callback, true)
                .ok()?;
            if host
                .add_event_listener_with_callback_and_bool("error", callback, true)
                .is_err()
            {
                let _ = host.remove_event_listener_with_callback_and_bool("load", callback, true);
                return None;
            }
            scan_complete_images(&host);
            Some((host, resolved))
        });
        move || {
            if let Some((host, resolved)) = registration {
                let callback = resolved.as_ref().unchecked_ref();
                let _ = host.remove_event_listener_with_callback_and_bool("load", callback, true);
                let _ = host.remove_event_listener_with_callback_and_bool("error", callback, true);
            }
        }
    });
}

#[hook]
pub(super) fn use_data_image_open_events(host: NodeRef) {
    use_effect_with(host, move |host| {
        let registration = host.cast::<web_sys::Element>().and_then(|host| {
            let opened = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                let Some(target) = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                else {
                    return;
                };
                let Some(anchor) = target
                    .closest("a[data-full-size-data-image]")
                    .ok()
                    .flatten()
                else {
                    return;
                };
                let Some(source) = anchor
                    .query_selector("img")
                    .ok()
                    .flatten()
                    .and_then(|image| image.get_attribute("src"))
                else {
                    return;
                };
                event.prevent_default();
                let Some(url) = data_image_blob_url(&source) else {
                    return;
                };
                let opened = web_sys::window().is_some_and(|window| {
                    window
                        .open_with_url_and_target_and_features(&url, "_blank", "noopener")
                        .is_ok()
                });
                if opened {
                    let retired_url = url.clone();
                    Timeout::new(60_000, move || {
                        let _ = web_sys::Url::revoke_object_url(&retired_url);
                    })
                    .forget();
                } else {
                    let _ = web_sys::Url::revoke_object_url(&url);
                }
            });
            host.add_event_listener_with_callback("click", opened.as_ref().unchecked_ref())
                .ok()?;
            Some((host, opened))
        });
        move || {
            if let Some((host, opened)) = registration {
                let _ = host
                    .remove_event_listener_with_callback("click", opened.as_ref().unchecked_ref());
            }
        }
    });
}

fn data_image_blob_url(source: &str) -> Option<String> {
    let (metadata, payload) = source.split_once(',')?;
    let normalized = metadata.to_ascii_lowercase();
    if !matches!(
        normalized.as_str(),
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/gif;base64"
            | "data:image/webp;base64"
    ) {
        return None;
    }
    let content_type = &metadata[5..metadata.find(';')?];
    let decoded = web_sys::window()?.atob(payload).ok()?;
    let bytes = decoded
        .encode_utf16()
        .map(|byte| u8::try_from(byte).ok())
        .collect::<Option<Vec<_>>>()?;
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes.as_slice()));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(content_type);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

#[hook]
pub(super) fn use_element_width(host: NodeRef) -> UseStateHandle<f64> {
    let width = use_state_eq(|| 0.0);
    {
        let width = width.clone();
        use_effect_with(host, move |host| {
            let registration = host.cast::<web_sys::Element>().and_then(|element| {
                let initial = element_content_width(&element)?;
                width.set(initial);
                let observed = element.clone();
                let width = width.clone();
                let callback = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
                    move |_, _| {
                        if let Some(measured) = element_content_width(&observed) {
                            width.set(measured);
                        }
                    },
                );
                let observer =
                    web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
                observer.observe(&element);
                Some((observer, callback))
            });
            move || {
                if let Some((observer, _callback)) = registration {
                    observer.disconnect();
                }
            }
        });
    }
    width
}

fn element_content_width(element: &web_sys::Element) -> Option<f64> {
    let style = web_sys::window()?.get_computed_style(element).ok()??;
    let padding = |property| {
        style
            .get_property_value(property)
            .ok()?
            .trim()
            .strip_suffix("px")?
            .parse::<f64>()
            .ok()
    };
    Some(crate::mosaic::content_box_width(
        f64::from(element.client_width()),
        padding("padding-left")?,
        padding("padding-right")?,
    ))
}
