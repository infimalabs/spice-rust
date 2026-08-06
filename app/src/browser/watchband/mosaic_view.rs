//! Browser measurement and Rust-lattice rendering for lineage activity.

use super::*;

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

#[derive(Clone, PartialEq)]
struct ActivityMeasurement {
    width: i32,
    activity: ActivityCard,
    candidates: Vec<crate::mosaic::Candidate>,
    heights: Vec<i32>,
}

pub(super) struct CardLayout {
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
        max_row: i32,
        measurement: &ActivityMeasurement,
    ) -> Self {
        Self {
            track: card.track,
            span: card.span,
            base: card.base,
            row: crate::mosaic::css_row_start(card, max_row),
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

fn activity_measurement<'a>(
    measurements: &'a [ActivityMeasurement],
    width: i32,
    activity: &ActivityCard,
) -> Option<&'a ActivityMeasurement> {
    measurements
        .iter()
        .find(|measurement| measurement.width == width && measurement.activity == *activity)
}

#[derive(Properties, PartialEq)]
pub(super) struct MosaicPlaneProps {
    pub(super) lane: CanonicalUuid,
    pub(super) life: LineageLife,
    pub(super) members: usize,
    pub(super) cards: Vec<ActivityCard>,
    pub(super) drafts: UseReducerHandle<ComposerDrafts>,
    pub(super) speak: Callback<String>,
    pub(super) speech: LineageSpeech,
}

/// One component per lineage lane is deliberate: every lane owns one width,
/// candidate stream, engine call, and row floor for all of its worktree
/// members. Sibling lineages can never carry a skyline into one another.
#[function_component(MosaicPlane)]
pub(super) fn mosaic_plane(properties: &MosaicPlaneProps) -> Html {
    let host = use_node_ref();
    let width = use_element_width(host.clone());
    let measurement_width = width.round() as i32;
    let measurements =
        use_activity_measurements(host.clone(), measurement_width, properties.cards.clone());
    use_narration(
        properties.cards.clone(),
        properties.speech,
        properties.speak.clone(),
    );
    let geometry = crate::mosaic::Geometry::new(16.0, f64::from(measurement_width));
    let plane = arrange_activity(&properties.cards, &measurements, measurement_width);
    let rendered_cards = render_activity_cards(
        properties,
        &measurements,
        measurement_width,
        &geometry,
        plane.as_ref(),
    );
    let measurement_phase = if plane.is_some() { "ready" } else { "base" };
    let plane_rows = plane
        .as_ref()
        .map_or(1, crate::mosaic_plane::LanePlane::rows);
    html! {
        <section
            ref={host}
            class="lane-mosaic-plane"
            data-agent-plane={properties.lane.to_string()}
            data-lineage-policy={properties.life.label()}
            data-lineage-members={properties.members.to_string()}
            data-lanes={geometry.lane_count.to_string()}
            data-plane-rows={plane_rows.to_string()}
            data-track-count={crate::mosaic::TRACK_COUNT.to_string()}
            data-base-span={geometry.base_span.to_string()}
            data-tall-threshold={crate::mosaic::TALL_THRESHOLD_PX.to_string()}
            data-measurement-phase={measurement_phase}
            aria-label={format!(
                "Packed lineage plane: {} policy, {} {}",
                properties.life.label(),
                properties.members,
                if properties.members == 1 { "member" } else { "members" },
            )}
        >
            {rendered_cards}
        </section>
    }
}

#[hook]
fn use_narration(cards: Vec<ActivityCard>, speech: LineageSpeech, speak: Callback<String>) {
    let narration = use_mut_ref(|| (false, BTreeMap::<String, ()>::new()));
    let messages = cards
        .iter()
        .filter_map(|card| match card {
            ActivityCard::Agent { message, .. } => Some((
                format!(
                    "{}:{}:{}:{}:{}",
                    message.lane,
                    message.locus.member,
                    message.locus.offset,
                    message.locus.length,
                    message.locus.event,
                ),
                message.text.clone(),
            )),
            ActivityCard::Directive { .. } => None,
        })
        .collect::<Vec<_>>();
    use_effect_with((speech, messages), move |(speech, messages)| {
        let mut narration = narration.borrow_mut();
        if !narration.0 {
            narration.0 = true;
            for (key, _) in messages {
                narration.1.insert(key.clone(), ());
            }
            return;
        }
        let newest = messages
            .iter()
            .rev()
            .find_map(|(key, text)| (!narration.1.contains_key(key)).then_some(text.clone()));
        for (key, _) in messages {
            narration.1.insert(key.clone(), ());
        }
        if *speech == LineageSpeech::Narrate
            && let Some(text) = newest
        {
            speak.emit(text);
        }
    });
}

#[hook]
fn use_activity_measurements(
    host: NodeRef,
    width: i32,
    cards: Vec<ActivityCard>,
) -> UseStateHandle<Vec<ActivityMeasurement>> {
    let measurements = use_state_eq(Vec::<ActivityMeasurement>::new);
    {
        let measurements = measurements.clone();
        let prior = (*measurements).clone();
        use_effect_with((width, cards), move |(width, cards)| {
            let canceled = Rc::new(Cell::new(false));
            let unknown = cards
                .iter()
                .enumerate()
                .filter(|(_, activity)| activity_measurement(&prior, *width, activity).is_none())
                .map(|(index, activity)| (index, activity.clone()))
                .collect::<Vec<_>>();
            if *width > 0 && !unknown.is_empty() {
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
                        let mut next = prior
                            .into_iter()
                            .filter(|measurement| measurement.width == width)
                            .collect::<Vec<_>>();
                        next.extend(measured);
                        measurements.set(next);
                    }
                });
            }
            move || canceled.set(true)
        });
    }
    measurements
}

async fn measure_unknown(
    host: NodeRef,
    width: i32,
    unknown: Vec<(usize, ActivityCard)>,
    canceled: Rc<Cell<bool>>,
) -> Option<Vec<ActivityMeasurement>> {
    TimeoutFuture::new(0).await;
    if canceled.get() {
        return None;
    }
    let host = host.cast::<web_sys::Element>()?;
    let geometry = crate::mosaic::Geometry::new(16.0, f64::from(width));
    let mut measured = Vec::with_capacity(unknown.len());
    for (index, activity) in unknown {
        let selector = format!("[data-mosaic-measure-index=\"{index}\"]");
        let element = host
            .query_selector(&selector)
            .ok()
            .flatten()?
            .dyn_into::<web_sys::HtmlElement>()
            .ok()?;
        let mut heights = Vec::with_capacity(2);
        let candidates = crate::mosaic::span_candidates(&geometry, |span| {
            let _ = element.set_attribute("data-measure-span", &span.to_string());
            let height = element.offset_height();
            heights.push(height);
            f64::from(height)
        });
        let _ = element.set_attribute("data-measure-span", &geometry.base_span.to_string());
        measured.push(ActivityMeasurement {
            width,
            activity,
            candidates,
            heights,
        });
    }
    Some(measured)
}

fn arrange_activity(
    cards: &[ActivityCard],
    measurements: &[ActivityMeasurement],
    width: i32,
) -> Option<crate::mosaic_plane::LanePlane> {
    (width > 0)
        .then(|| {
            cards
                .iter()
                .map(|activity| {
                    activity_measurement(measurements, width, activity)
                        .map(|measurement| measurement.candidates.clone())
                })
                .collect::<Option<Vec<_>>>()
        })
        .flatten()
        .as_deref()
        .map(crate::mosaic_plane::arrange)
}

fn render_activity_cards(
    properties: &MosaicPlaneProps,
    measurements: &[ActivityMeasurement],
    width: i32,
    geometry: &crate::mosaic::Geometry,
    plane: Option<&crate::mosaic_plane::LanePlane>,
) -> Html {
    properties
        .cards
        .iter()
        .enumerate()
        .map(|(order, activity)| {
            let measurement = activity_measurement(measurements, width, activity);
            let layout = match (plane, measurement) {
                (Some(plane), Some(measurement)) => {
                    CardLayout::placed(order, &plane.cards[order], plane.max_row, measurement)
                }
                _ => CardLayout::measuring(order, geometry.base_span),
            };
            match activity {
                ActivityCard::Agent {
                    member,
                    quote_lane,
                    message,
                } => render_agent_message_card(
                    member,
                    *quote_lane,
                    message,
                    &properties.drafts,
                    &properties.speak,
                    order as u64,
                    &layout,
                ),
                ActivityCard::Directive { lane, directive } => render_directive_card(
                    *lane,
                    directive,
                    &lane.to_string(),
                    &properties.drafts,
                    &properties.speak,
                    order as u64,
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
                width.set(element_content_width(&element)?);
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
