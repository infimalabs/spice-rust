//! Image lifecycle, full-size data-image opening, and host width observation.

use super::*;

use gloo_timers::callback::Timeout;

const RESIZE_DEBOUNCE_MILLIS: u32 = 200;

#[derive(Clone, Default, PartialEq)]
pub(super) struct ImageRevisions(BTreeMap<String, u64>);

impl ImageRevisions {
    pub(super) fn get(&self, key: &str) -> u64 {
        self.0.get(key).copied().unwrap_or_default()
    }
}

impl Reducible for ImageRevisions {
    type Action = String;

    fn reduce(self: Rc<Self>, key: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        let revision = next.0.entry(key).or_default();
        *revision = revision.saturating_add(1);
        Rc::new(next)
    }
}

#[derive(Clone, Default)]
struct ImageResolutionSnapshot {
    width: i32,
    cards: Vec<ActivityCard>,
    revisions: ImageRevisions,
    measurements: Vec<ActivityMeasurement>,
    committed: Option<crate::mosaic_plane::PersistentLanePlane<String>>,
}

#[derive(Clone)]
struct ImageResolutionCoordinator {
    revisions: UseReducerHandle<ImageRevisions>,
    measurements: UseStateHandle<Vec<ActivityMeasurement>>,
    host: web_sys::Element,
    live: Rc<RefCell<ImageResolutionSnapshot>>,
}

impl ImageResolutionCoordinator {
    fn new(
        revisions: UseReducerHandle<ImageRevisions>,
        measurements: UseStateHandle<Vec<ActivityMeasurement>>,
        host: web_sys::Element,
        live: Rc<RefCell<ImageResolutionSnapshot>>,
    ) -> Self {
        Self {
            revisions,
            measurements,
            host,
            live,
        }
    }

    fn resolve_card(&self, key: &str) {
        let mut snapshot = self.live.borrow_mut();
        let width = element_content_width(&self.host)
            .map(|measured| measured.round() as i32)
            .unwrap_or(snapshot.width);
        let measured = self.resolve_key(&mut snapshot, key, width);
        let next_measurements = measured
            .is_some_and(|changed| changed)
            .then(|| snapshot.measurements.clone());
        drop(snapshot);
        if let Some(next_measurements) = next_measurements {
            self.measurements.set(next_measurements);
        }
        if measured.is_some() {
            self.revisions.dispatch(key.to_owned());
        }
    }

    fn resolve_key(
        &self,
        snapshot: &mut ImageResolutionSnapshot,
        key: &str,
        width: i32,
    ) -> Option<bool> {
        let activity = snapshot
            .cards
            .iter()
            .find(|activity| activity.key() == key)
            .cloned()?;
        let selector = format!("[data-mosaic-card-key=\"{key}\"]");
        let card = self
            .host
            .query_selector(&selector)
            .ok()
            .flatten()?
            .dyn_into::<web_sys::HtmlElement>()
            .ok()?;
        let revision = snapshot.revisions.get(key).saturating_add(1);
        snapshot.revisions.0.insert(key.to_owned(), revision);
        mark_card_image_revision(&card, width, revision);
        if width < MIN_MEASURABLE_WIDTH_PX {
            return Some(false);
        }
        let latched_span = snapshot.committed.as_ref().and_then(|plane| {
            plane
                .cards()
                .iter()
                .find_map(|entry| (entry.key == key).then_some(entry.card.span))
        });
        let Some(measurement) = measure_element(&card, width, &activity, revision, latched_span)
        else {
            return Some(false);
        };
        snapshot.measurements.retain(|previous| {
            previous.width != measurement.width || previous.key != measurement.key
        });
        snapshot.measurements.push(measurement);
        Some(true)
    }

    fn resolve_image(&self, image: web_sys::HtmlImageElement) {
        let outcome = if image.natural_width() > 0 {
            "loaded"
        } else {
            "error"
        };
        let changed = set_image_resolution_state(&image, outcome);
        if !changed && image.has_attribute("data-mosaic-image-revision") {
            return;
        }
        let Some(card) = image.closest(".mosaic-message-card").ok().flatten() else {
            return;
        };
        if card.has_attribute("data-mosaic-image-revision") || !card_images_are_terminal(&card) {
            return;
        }
        let Some(key) = card.get_attribute("data-mosaic-card-key") else {
            return;
        };
        self.resolve_card(&key);
    }

    fn scan_complete(&self) {
        let Ok(images) = self.host.query_selector_all("img") else {
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
                self.resolve_image(image);
            }
        }
    }
}

fn card_images_are_terminal(card: &web_sys::Element) -> bool {
    let Ok(images) = card.query_selector_all("img") else {
        return false;
    };
    images.length() > 0
        && (0..images.length()).all(|index| {
            images.item(index).is_some_and(|image| {
                matches!(
                    image
                        .dyn_into::<web_sys::HtmlImageElement>()
                        .ok()
                        .and_then(|image| image.get_attribute("data-mosaic-image-state"))
                        .as_deref(),
                    Some("loaded" | "error")
                )
            })
        })
}

fn mark_card_image_revision(card: &web_sys::HtmlElement, width: i32, revision: u64) {
    let _ = card.set_attribute("data-mosaic-image-revision", &revision.to_string());
    let Ok(images) = card.query_selector_all("img") else {
        return;
    };
    for index in 0..images.length() {
        let Some(image) = images
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::HtmlImageElement>().ok())
        else {
            continue;
        };
        let _ = image.set_attribute("data-mosaic-image-revision", &revision.to_string());
        let _ = image.set_attribute("data-mosaic-image-measured-width", &width.to_string());
    }
}

fn set_image_resolution_state(image: &web_sys::HtmlImageElement, outcome: &str) -> bool {
    let changed = image.get_attribute("data-mosaic-image-state").as_deref() != Some(outcome);
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
    changed
}

#[hook]
pub(super) fn use_image_resolution_events(
    host: NodeRef,
    width: i32,
    cards: Vec<ActivityCard>,
    revisions: UseReducerHandle<ImageRevisions>,
    measurements: UseStateHandle<Vec<ActivityMeasurement>>,
    committed: UseStateHandle<Option<crate::mosaic_plane::PersistentLanePlane<String>>>,
) {
    let live = use_mut_ref(ImageResolutionSnapshot::default);
    let image_scan = cards
        .iter()
        .map(|activity| (activity.key(), activity.measurement_fingerprint()))
        .collect::<Vec<_>>();
    *live.borrow_mut() = ImageResolutionSnapshot {
        width,
        cards,
        revisions: (*revisions).clone(),
        measurements: (*measurements).clone(),
        committed: (*committed).clone(),
    };
    use_effect_with((host, image_scan), move |(host, _)| {
        let registration = host.cast::<web_sys::Element>().and_then(|host| {
            let coordinator = ImageResolutionCoordinator::new(
                revisions.clone(),
                measurements.clone(),
                host.clone(),
                live.clone(),
            );
            let event_coordinator = coordinator.clone();
            let resolved =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let Some(image) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlImageElement>().ok())
                    else {
                        return;
                    };
                    event_coordinator.resolve_image(image);
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
            coordinator.scan_complete();
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
                let observed_width = Rc::new(Cell::new(initial));
                let pending = Rc::new(RefCell::new(None::<Timeout>));
                let observed = element.clone();
                let width = width.clone();
                let callback_width = observed_width.clone();
                let callback_pending = pending.clone();
                let callback = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
                    move |_, _| {
                        if let Some(measured) = element_content_width(&observed) {
                            let previous = callback_width.replace(measured);
                            if measured < f64::from(MIN_MEASURABLE_WIDTH_PX)
                                || previous < f64::from(MIN_MEASURABLE_WIDTH_PX)
                            {
                                callback_pending.borrow_mut().take();
                                width.set(measured);
                            } else if (measured - previous).abs() > 0.5 {
                                let settled_width = width.clone();
                                callback_pending
                                    .borrow_mut()
                                    .replace(Timeout::new(RESIZE_DEBOUNCE_MILLIS, move || {
                                        settled_width.set(measured)
                                    }));
                            }
                        }
                    },
                );
                let observer =
                    web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
                observer.observe(&element);
                Some((observer, callback, pending))
            });
            move || {
                if let Some((observer, _callback, pending)) = registration {
                    observer.disconnect();
                    pending.borrow_mut().take();
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
