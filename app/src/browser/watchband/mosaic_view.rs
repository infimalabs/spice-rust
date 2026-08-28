//! Browser measurement and Rust-lattice rendering for lineage activity.

use super::*;

use yew::functional::UseReducerDispatcher;

use gloo_timers::callback::Timeout;

const MIN_MEASURABLE_WIDTH_PX: i32 = 24;
const RESIZE_DEBOUNCE_MILLIS: u32 = 200;

#[derive(Clone, PartialEq)]
pub(super) enum ActivityCard {
    Agent {
        member: crate::runtime::LaneMember,
        quote_lane: CanonicalUuid,
        message: wire::AgentMessage,
    },
    Directive {
        lane: CanonicalUuid,
        directive: wire::intent::PendingDirective,
    },
}

impl ActivityCard {
    fn key(&self) -> String {
        match self {
            Self::Agent { message, .. } => agent_message_source_id(message),
            Self::Directive { directive, .. } => directive_source_id(directive),
        }
    }

    fn measurement_fingerprint(&self) -> String {
        match self {
            Self::Agent {
                member, message, ..
            } => format!(
                "agent\0{}\0{}\0{}\0{}",
                member.worktree, message.author, message.locus.event, message.text
            ),
            Self::Directive { directive, .. } => serde_json::to_string(directive)
                .expect("an admitted pending directive serializes for measurement identity"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ActivityMeasurement {
    width: i32,
    key: String,
    fingerprint: String,
    image_revision: u64,
    candidates: Vec<crate::mosaic::Candidate>,
    latched: Option<crate::mosaic::Candidate>,
    heights: Vec<i32>,
}

pub(super) struct CardLayout {
    pub(super) committed: bool,
    pub(super) track: usize,
    pub(super) span: usize,
    pub(super) base: i32,
    pub(super) row: i32,
    pub(super) rows: i32,
    pub(super) lifecycle: crate::mosaic::Lifecycle,
    pub(super) measurement_index: usize,
    pub(super) measurement_reads: usize,
    pub(super) measured_heights: String,
    pub(super) candidate_spans: String,
    pub(super) measure_span: usize,
}

impl CardLayout {
    fn measuring(measurement_index: usize, base_span: usize) -> Self {
        Self {
            committed: false,
            track: 0,
            span: base_span,
            base: 0,
            row: 1,
            rows: 2,
            lifecycle: crate::mosaic::Lifecycle::Liquid,
            measurement_index,
            measurement_reads: 0,
            measured_heights: String::new(),
            candidate_spans: String::new(),
            measure_span: base_span,
        }
    }

    fn placed(
        measurement_index: usize,
        card: &crate::mosaic::Card,
        measurement: &ActivityMeasurement,
    ) -> Self {
        Self {
            committed: true,
            track: card.track,
            span: card.span,
            base: card.base,
            row: card.base,
            rows: card.rows,
            lifecycle: card.lifecycle,
            measurement_index,
            measurement_reads: measurement.heights.len(),
            measured_heights: measurement
                .heights
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            candidate_spans: measurement
                .candidates
                .iter()
                .map(|candidate| candidate.span.to_string())
                .collect::<Vec<_>>()
                .join(","),
            measure_span: measurement.candidates[0].span,
        }
    }
}

const PLANE_LAYOUT_PROPERTIES: [(&str, &str); 2] = [
    ("--mosaic-max-row", "data-max-row"),
    ("--mosaic-plane-rows", "data-plane-rows"),
];
const CARD_LAYOUT_PROPERTIES: [(&str, &str); 5] = [
    ("--mosaic-base", "data-base"),
    ("--mosaic-rows", "data-rows"),
    ("--mosaic-track", "data-track"),
    ("--mosaic-span", "data-span"),
    ("--mosaic-measure-span", "data-measure-span"),
];

fn copy_layout_property(element: &web_sys::HtmlElement, property: &str, attribute: &str) {
    let value = element
        .get_attribute(attribute)
        .unwrap_or_else(|| panic!("Mosaic layout element carries {attribute}"));
    value
        .parse::<i32>()
        .unwrap_or_else(|_| panic!("Mosaic layout attribute {attribute} is an integer: {value}"));
    element
        .style()
        .set_property(property, &value)
        .unwrap_or_else(|_| panic!("Mosaic layout writes {property}"));
}

fn copy_layout_properties(element: &web_sys::HtmlElement, properties: &[(&str, &str)]) {
    for (property, attribute) in properties {
        copy_layout_property(element, property, attribute);
    }
}

#[hook]
/// Copy admitted integer diagnostics into the one CSP-safe layout channel.
///
/// Firefox does not compute the typed `attr()` arithmetic this lattice once
/// used, and the Canvas CSP deliberately refuses serialized style attributes.
/// `CssStyleDeclaration` keeps the diagnostic attributes visible while every
/// browser executes the same custom-property geometry.
fn use_layout_properties(
    host: NodeRef,
    width: i32,
    keys: Vec<String>,
    plane: Option<crate::mosaic_plane::PersistentLanePlane<String>>,
) {
    use_effect_with((host, width, keys, plane), move |(host, _, _, _)| {
        let host = host
            .cast::<web_sys::HtmlElement>()
            .expect("a rendered Mosaic plane is an HTML element");
        copy_layout_properties(&host, &PLANE_LAYOUT_PROPERTIES);
        let cards = host
            .query_selector_all(".mosaic-message-card")
            .expect("the fixed Mosaic card selector is valid");
        for index in 0..cards.length() {
            let card = cards
                .item(index)
                .expect("the Mosaic card list retains every indexed node")
                .dyn_into::<web_sys::HtmlElement>()
                .expect("a Mosaic card is an HTML element");
            copy_layout_properties(&card, &CARD_LAYOUT_PROPERTIES);
        }
    });
}

fn activity_measurement<'a>(
    measurements: &'a [ActivityMeasurement],
    width: i32,
    activity: &ActivityCard,
    image_revision: u64,
) -> Option<&'a ActivityMeasurement> {
    let key = activity.key();
    let fingerprint = activity.measurement_fingerprint();
    measurements.iter().find(|measurement| {
        measurement.width == width
            && measurement.key == key
            && measurement.fingerprint == fingerprint
            && measurement.image_revision == image_revision
    })
}

#[derive(Properties, PartialEq)]
pub(super) struct MosaicPlaneProps {
    pub(super) lane: CanonicalUuid,
    pub(super) life: LineageLife,
    pub(super) members: usize,
    pub(super) cards: Vec<ActivityCard>,
    pub(super) quote_dispatcher: UseReducerDispatcher<ComposerDrafts>,
    pub(super) speak: Callback<SpeechRequest>,
    pub(super) narrate: Callback<NarrationBatch>,
    pub(super) speech: LineageSpeech,
}

struct MosaicPlaneView<'a> {
    properties: &'a MosaicPlaneProps,
    measurement_width: i32,
    image_revisions: &'a ImageRevisions,
    measurements: &'a [ActivityMeasurement],
    plane: Option<&'a crate::mosaic_plane::PersistentLanePlane<String>>,
    geometry: crate::mosaic::Geometry,
    max_row: Option<i32>,
}

impl MosaicPlaneView<'_> {
    fn render(self, host: NodeRef) -> Html {
        let rendered_cards = render_activity_cards(
            self.properties,
            self.measurements,
            &self.geometry,
            self.plane,
            self.image_revisions,
        );
        let measurement_phase = if self.plane.is_some() {
            "ready"
        } else {
            "base"
        };
        let plane_rows = self.plane.map_or(1, |plane| plane.rows());
        let pending = self
            .properties
            .cards
            .iter()
            .filter(|activity| {
                self.plane.is_none_or(|plane| {
                    plane
                        .cards()
                        .iter()
                        .all(|entry| entry.key != activity.key())
                })
            })
            .count();
        let commits = self.plane.map_or(0, |plane| plane.commits());
        let transition = self
            .plane
            .map_or("deferred", |plane| plane.transition().as_str());
        let committed_width = self
            .plane
            .map_or(0, crate::mosaic_plane::PersistentLanePlane::geometry);
        html! {
            <section
                ref={host}
                class="lane-mosaic-plane"
                data-agent-plane={self.properties.lane.to_string()}
                data-lineage-policy={self.properties.life.label()}
                data-lineage-members={self.properties.members.to_string()}
                data-lanes={self.geometry.lane_count.to_string()}
                data-plane-rows={plane_rows.to_string()}
                data-max-row={self.max_row.unwrap_or_default().to_string()}
                data-module={self.geometry.module.to_string()}
                data-track-count={crate::mosaic::TRACK_COUNT.to_string()}
                data-base-span={self.geometry.base_span.to_string()}
                data-min-lane={self.geometry.min_lane.to_string()}
                data-min-card-rows={crate::mosaic::MIN_CARD_ROWS.to_string()}
                data-tall-threshold={crate::mosaic::TALL_THRESHOLD_PX.to_string()}
                data-measurement-phase={measurement_phase}
                data-measurement-pending={pending.to_string()}
                data-plane-commits={commits.to_string()}
                data-plane-transition={transition}
                data-observed-width={self.measurement_width.to_string()}
                data-committed-width={committed_width.to_string()}
                aria-label={format!(
                    "Packed lineage plane: {} policy, {} {}",
                    self.properties.life.label(),
                    self.properties.members,
                    if self.properties.members == 1 { "member" } else { "members" },
                )}
            >
                {rendered_cards}
            </section>
        }
    }
}

fn committed_spans(
    plane: Option<&crate::mosaic_plane::PersistentLanePlane<String>>,
) -> Vec<(String, usize)> {
    plane
        .into_iter()
        .flat_map(crate::mosaic_plane::PersistentLanePlane::cards)
        .map(|entry| (entry.key.clone(), entry.card.span))
        .collect()
}

/// One component per lineage lane is deliberate: every lane owns one width,
/// candidate stream, engine call, and row floor for all of its worktree
/// members. Sibling lineages can never carry a skyline into one another.
#[function_component(MosaicPlane)]
pub(super) fn mosaic_plane(properties: &MosaicPlaneProps) -> Html {
    let host = use_node_ref();
    let width = use_element_width(host.clone());
    let measurement_width = width.round() as i32;
    let image_revisions = use_reducer(ImageRevisions::default);
    let committed = use_state_eq(|| None::<crate::mosaic_plane::PersistentLanePlane<String>>);
    let card_keys = properties
        .cards
        .iter()
        .map(ActivityCard::key)
        .collect::<Vec<_>>();
    use_layout_properties(
        host.clone(),
        measurement_width,
        card_keys,
        (*committed).clone(),
    );
    let committed_spans = committed_spans(committed.as_ref());
    let measurements = use_state_eq(Vec::<ActivityMeasurement>::new);
    use_image_resolution_events(
        host.clone(),
        measurement_width,
        properties.cards.clone(),
        image_revisions.clone(),
        measurements.clone(),
        committed.clone(),
    );
    use_activity_measurements(
        host.clone(),
        measurement_width,
        properties.cards.clone(),
        (*image_revisions).clone(),
        committed_spans,
        measurements.clone(),
    );
    use_persistent_plane(
        committed.clone(),
        measurement_width,
        properties.cards.clone(),
        (*measurements).clone(),
        (*image_revisions).clone(),
    );
    use_narration(
        properties.lane,
        properties.cards.clone(),
        properties.speech,
        properties.narrate.clone(),
    );
    let committed_plane = committed.as_ref();
    let geometry_width = committed_plane.map_or(
        measurement_width,
        crate::mosaic_plane::PersistentLanePlane::geometry,
    );
    let geometry = crate::mosaic::Geometry::new(16.0, f64::from(geometry_width));
    let max_row = committed_plane.map(crate::mosaic_plane::PersistentLanePlane::max_row);
    use_plane_scroll(
        host.clone(),
        max_row,
        geometry.module,
        committed_plane.map_or(0, |plane| plane.cards().len()),
    );
    MosaicPlaneView {
        properties,
        measurement_width,
        image_revisions: &image_revisions,
        measurements: &measurements,
        plane: committed_plane,
        geometry,
        max_row,
    }
    .render(host)
}

#[hook]
/// Preserve the viewport only after live measurement produces a ready plane.
///
/// A temporary `None` during incremental measurement deliberately retains the
/// prior ready-plane state, so the eventual row delta has one authoritative
/// baseline and one scroll owner.
fn use_plane_scroll(host: NodeRef, max_row: Option<i32>, module: i32, card_count: usize) {
    let scroll_state = use_mut_ref(|| None::<(i32, i32, usize)>);
    use_effect_with(
        (host, max_row, module, card_count),
        move |(host, max_row, module, card_count)| {
            let Some(max_row) = *max_row else {
                return;
            };
            let previous = *scroll_state.borrow();
            if let Some((previous_max_row, previous_module, previous_card_count)) = previous
                && let Some(scroller) = host.cast::<web_sys::Element>()
            {
                let scroll_top = f64::from(scroller.scroll_top());
                let replaying = *module != previous_module || *card_count <= previous_card_count;
                if let Some(delta) = crate::mosaic::scroll_compensation(
                    previous_max_row,
                    max_row,
                    *module,
                    crate::mosaic::pane_is_scrolled(scroll_top, None),
                    replaying,
                ) {
                    scroller.set_scroll_top((scroll_top + delta).round() as i32);
                }
            }
            *scroll_state.borrow_mut() = Some((max_row, *module, *card_count));
        },
    );
}

#[hook]
fn use_narration(
    lineage: CanonicalUuid,
    cards: Vec<ActivityCard>,
    speech: LineageSpeech,
    narrate: Callback<NarrationBatch>,
) {
    let materialized = use_mut_ref(|| false);
    let messages = cards
        .iter()
        .filter_map(|card| match card {
            ActivityCard::Agent { message, .. } => Some(NarrationMessage {
                lane: message.lane.to_string(),
                key: format!(
                    "{}:{}:{}:{}:{}",
                    message.lane,
                    message.locus.member,
                    message.locus.offset,
                    message.locus.length,
                    message.locus.event,
                ),
                text: message.text.clone(),
            }),
            ActivityCard::Directive { .. } => None,
        })
        .collect::<Vec<_>>();
    use_effect_with((speech, messages), move |(speech, messages)| {
        let first = !*materialized.borrow();
        *materialized.borrow_mut() = true;
        narrate.emit(NarrationBatch {
            lineage: lineage.to_string(),
            materialization: first,
            enabled: *speech == LineageSpeech::Narrate,
            messages: messages.clone(),
        });
    });
}

#[derive(Clone, Default, PartialEq)]
struct ImageRevisions(BTreeMap<String, u64>);

impl ImageRevisions {
    fn get(&self, key: &str) -> u64 {
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

#[hook]
fn use_image_resolution_events(
    host: NodeRef,
    width: i32,
    cards: Vec<ActivityCard>,
    revisions: UseReducerHandle<ImageRevisions>,
    measurements: UseStateHandle<Vec<ActivityMeasurement>>,
    committed: UseStateHandle<Option<crate::mosaic_plane::PersistentLanePlane<String>>>,
) {
    let live = use_mut_ref(ImageResolutionSnapshot::default);
    *live.borrow_mut() = ImageResolutionSnapshot {
        width,
        cards,
        revisions: (*revisions).clone(),
        measurements: (*measurements).clone(),
        committed: (*committed).clone(),
    };
    use_effect_with(host, move |host| {
        let registration = host.cast::<web_sys::Element>().and_then(|host| {
            let resolved_revisions = revisions.clone();
            let resolved_measurements = measurements.clone();
            let resolved_host = host.clone();
            let live = live.clone();
            let resolved =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let Some(image) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlImageElement>().ok())
                    else {
                        return;
                    };
                    let outcome = if image.natural_width() > 0 {
                        "loaded"
                    } else {
                        "error"
                    };
                    if image.get_attribute("data-mosaic-image-state").as_deref() == Some(outcome) {
                        return;
                    }
                    let _ = image.set_attribute("data-mosaic-image-state", outcome);
                    let Some(card) = image.closest(".mosaic-message-card").ok().flatten() else {
                        return;
                    };
                    let Some(key) = card.get_attribute("data-mosaic-card-key") else {
                        return;
                    };
                    let mut snapshot = live.borrow_mut();
                    let Some(activity) = snapshot
                        .cards
                        .iter()
                        .find(|activity| activity.key() == key)
                        .cloned()
                    else {
                        return;
                    };
                    let measurement_width = element_content_width(&resolved_host)
                        .map(|measured| measured.round() as i32)
                        .unwrap_or(snapshot.width);
                    let image_revision = snapshot.revisions.get(&key).saturating_add(1);
                    snapshot.revisions.0.insert(key.clone(), image_revision);
                    let _ = image
                        .set_attribute("data-mosaic-image-revision", &image_revision.to_string());
                    let _ = image.set_attribute(
                        "data-mosaic-image-measured-width",
                        &measurement_width.to_string(),
                    );
                    if measurement_width < MIN_MEASURABLE_WIDTH_PX {
                        resolved_revisions.dispatch(key);
                        return;
                    }
                    let latched_span = snapshot.committed.as_ref().and_then(|plane| {
                        plane
                            .cards()
                            .iter()
                            .find_map(|entry| (entry.key == key).then_some(entry.card.span))
                    });
                    let Some(measurement) = measure_element(
                        &card.unchecked_into::<web_sys::HtmlElement>(),
                        measurement_width,
                        &activity,
                        image_revision,
                        latched_span,
                    ) else {
                        resolved_revisions.dispatch(key);
                        return;
                    };
                    snapshot.measurements.retain(|previous| {
                        previous.width != measurement.width || previous.key != measurement.key
                    });
                    snapshot.measurements.push(measurement);
                    resolved_measurements.set(snapshot.measurements.clone());
                    resolved_revisions.dispatch(key);
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
fn use_activity_measurements(
    host: NodeRef,
    width: i32,
    cards: Vec<ActivityCard>,
    image_revisions: ImageRevisions,
    committed_spans: Vec<(String, usize)>,
    measurements: UseStateHandle<Vec<ActivityMeasurement>>,
) {
    {
        let measurements = measurements.clone();
        let prior = (*measurements).clone();
        use_effect_with(
            (width, cards, image_revisions, committed_spans),
            move |(width, cards, image_revisions, committed_spans)| {
                let canceled = Rc::new(Cell::new(false));
                let unknown = cards
                    .iter()
                    .enumerate()
                    .filter_map(|(index, activity)| {
                        let key = activity.key();
                        let image_revision = image_revisions.get(&key);
                        activity_measurement(&prior, *width, activity, image_revision)
                            .is_none()
                            .then(|| {
                                let latched_span =
                                    committed_spans.iter().find_map(|(candidate, span)| {
                                        (candidate == &key).then_some(*span)
                                    });
                                (index, activity.clone(), image_revision, latched_span)
                            })
                    })
                    .collect::<Vec<_>>();
                if *width >= MIN_MEASURABLE_WIDTH_PX && !unknown.is_empty() {
                    let canceled_task = canceled.clone();
                    let host = host.clone();
                    let measurements = measurements.clone();
                    let prior = prior.clone();
                    let width = *width;
                    spawn_local(async move {
                        let measured =
                            measure_unknown(host, width, unknown, canceled_task.clone()).await;
                        if let Some(measured) = measured
                            && !canceled_task.get()
                        {
                            let mut next = prior;
                            let previous_width = next.iter().rev().find_map(|measurement| {
                                (measurement.width != width).then_some(measurement.width)
                            });
                            next.retain(|measurement| {
                                measurement.width == width
                                    || Some(measurement.width) == previous_width
                            });
                            for fresh in &measured {
                                next.retain(|measurement| {
                                    measurement.width != fresh.width || measurement.key != fresh.key
                                });
                            }
                            next.extend(measured);
                            measurements.set(next);
                        }
                    });
                }
                move || canceled.set(true)
            },
        );
    }
}

async fn measure_unknown(
    host: NodeRef,
    width: i32,
    unknown: Vec<(usize, ActivityCard, u64, Option<usize>)>,
    canceled: Rc<Cell<bool>>,
) -> Option<Vec<ActivityMeasurement>> {
    TimeoutFuture::new(0).await;
    if canceled.get() {
        return None;
    }
    let host = host.cast::<web_sys::Element>()?;
    let mut measured = Vec::with_capacity(unknown.len());
    for (_index, activity, image_revision, latched_span) in unknown {
        let selector = format!("[data-mosaic-card-key=\"{}\"]", activity.key());
        let Some(element) = host.query_selector(&selector).ok().flatten() else {
            continue;
        };
        let Some(element) = element.dyn_into::<web_sys::HtmlElement>().ok() else {
            continue;
        };
        if let Some(measurement) =
            measure_element(&element, width, &activity, image_revision, latched_span)
        {
            measured.push(measurement);
        }
    }
    Some(measured)
}

fn measure_element(
    element: &web_sys::HtmlElement,
    width: i32,
    activity: &ActivityCard,
    image_revision: u64,
    latched_span: Option<usize>,
) -> Option<ActivityMeasurement> {
    if !prepare_images(element) {
        return None;
    }
    let geometry = crate::mosaic::Geometry::new(16.0, f64::from(width));
    let mut heights = Vec::with_capacity(3);
    let candidates = crate::mosaic::span_candidates(&geometry, |span| {
        let _ = element.set_attribute("data-measure-span", &span.to_string());
        let height = element.offset_height();
        heights.push(height);
        f64::from(height)
    });
    let latched = latched_span
        .filter(|span| candidates.iter().all(|candidate| candidate.span != *span))
        .map(|span| {
            let _ = element.set_attribute("data-measure-span", &span.to_string());
            let height = element.offset_height();
            heights.push(height);
            crate::mosaic::Candidate {
                span,
                rows: crate::mosaic::rows_for(height.into(), geometry.gap, geometry.module),
            }
        });
    let _ = element.set_attribute("data-measure-span", &geometry.base_span.to_string());
    Some(ActivityMeasurement {
        width,
        key: activity.key(),
        fingerprint: activity.measurement_fingerprint(),
        image_revision,
        candidates,
        latched,
        heights,
    })
}

fn prepare_images(element: &web_sys::HtmlElement) -> bool {
    let Ok(images) = element.query_selector_all("img") else {
        return false;
    };
    let mut ready = true;
    for index in 0..images.length() {
        let Some(image) = images
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::HtmlImageElement>().ok())
        else {
            ready = false;
            continue;
        };
        if image.complete() {
            let outcome = if image.natural_width() > 0 {
                "loaded"
            } else {
                "error"
            };
            let _ = image.set_attribute("data-mosaic-image-state", outcome);
        } else {
            let _ = image.set_attribute("data-mosaic-image-state", "pending");
        }
    }
    ready
}

fn measured_activity_at_revisions(
    cards: &[ActivityCard],
    measurements: &[ActivityMeasurement],
    width: i32,
    image_revisions: &ImageRevisions,
) -> Option<Vec<crate::mosaic_plane::MeasuredCard<String>>> {
    (width >= MIN_MEASURABLE_WIDTH_PX)
        .then(|| {
            cards
                .iter()
                .map(|activity| {
                    activity_measurement(
                        measurements,
                        width,
                        activity,
                        image_revisions.get(&activity.key()),
                    )
                    .map(|measurement| crate::mosaic_plane::MeasuredCard {
                        key: measurement.key.clone(),
                        candidates: measurement.candidates.clone(),
                        latched: measurement.latched,
                        content_revision: measurement.image_revision,
                    })
                })
                .collect::<Option<Vec<_>>>()
        })
        .flatten()
}

#[hook]
fn use_persistent_plane(
    committed: UseStateHandle<Option<crate::mosaic_plane::PersistentLanePlane<String>>>,
    width: i32,
    cards: Vec<ActivityCard>,
    measurements: Vec<ActivityMeasurement>,
    image_revisions: ImageRevisions,
) {
    use_effect_with(
        (width, cards, measurements, image_revisions),
        move |(width, cards, measurements, image_revisions)| {
            let Some(measured) =
                measured_activity_at_revisions(cards, measurements, *width, image_revisions)
            else {
                return;
            };
            let next = committed.as_ref().map_or_else(
                || crate::mosaic_plane::PersistentLanePlane::new(*width, &measured),
                |plane| plane.reconcile(*width, &measured),
            );
            if committed.as_ref() != Some(&next) {
                committed.set(Some(next));
            }
        },
    );
}

fn render_activity_cards(
    properties: &MosaicPlaneProps,
    measurements: &[ActivityMeasurement],
    geometry: &crate::mosaic::Geometry,
    plane: Option<&crate::mosaic_plane::PersistentLanePlane<String>>,
    image_revisions: &ImageRevisions,
) -> Html {
    let mut ordered = plane
        .into_iter()
        .flat_map(|plane| plane.cards())
        .filter_map(|entry| {
            properties
                .cards
                .iter()
                .find(|activity| activity.key() == entry.key)
        })
        .collect::<Vec<_>>();
    let unknown = properties
        .cards
        .iter()
        .filter(|activity| {
            let key = activity.key();
            ordered.iter().all(|known| known.key() != key)
        })
        .collect::<Vec<_>>();
    ordered.extend(unknown);
    ordered
        .into_iter()
        .enumerate()
        .map(|(order, activity)| {
            let speak = if properties.speech == LineageSpeech::Quiet {
                Callback::from(|_: String| {})
            } else {
                let lane = properties.lane.to_string();
                let speech_queue = properties.speak.clone();
                Callback::from(move |text: String| {
                    speech_queue.emit(SpeechRequest::manual(Some(lane.clone()), text));
                })
            };
            let key = activity.key();
            let image_revision = image_revisions.get(&key);
            let placement = plane.and_then(|plane| {
                plane
                    .cards()
                    .iter()
                    .find(|entry| entry.key == key)
                    .map(|entry| (plane, entry))
            });
            let measurement = placement
                .and_then(|(plane, _)| {
                    activity_measurement(measurements, plane.geometry(), activity, image_revision)
                })
                .or_else(|| {
                    measurements.iter().rev().find(|measurement| {
                        measurement.key == key
                            && measurement.fingerprint == activity.measurement_fingerprint()
                    })
                });
            let layout = match (placement, measurement) {
                (Some((_plane, entry)), Some(measurement)) => {
                    CardLayout::placed(entry.card.creation_index, &entry.card, measurement)
                }
                _ => CardLayout::measuring(order, geometry.base_span),
            };
            let quote_order = u64::try_from(layout.measurement_index)
                .expect("Mosaic creation index fits the quote ordering field");
            match activity {
                ActivityCard::Agent {
                    member,
                    quote_lane,
                    message,
                } => render_agent_message_card(
                    member,
                    *quote_lane,
                    message,
                    &properties.quote_dispatcher,
                    &speak,
                    quote_order,
                    &layout,
                ),
                ActivityCard::Directive { lane, directive } => render_directive_card(
                    *lane,
                    directive,
                    &lane.to_string(),
                    &properties.quote_dispatcher,
                    &speak,
                    quote_order,
                    &layout,
                ),
            }
        })
        .collect()
}

#[hook]
fn use_element_width(host: NodeRef) -> UseStateHandle<f64> {
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
