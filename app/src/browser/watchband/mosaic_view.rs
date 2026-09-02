//! Browser measurement and Rust-lattice rendering for lineage activity.

use super::*;

use std::collections::HashSet;

use yew::functional::UseReducerDispatcher;

use gloo_timers::callback::Timeout;

const MIN_MEASURABLE_WIDTH_PX: i32 = 24;
const RESIZE_DEBOUNCE_MILLIS: u32 = 200;

#[derive(Clone)]
pub(super) struct HistorySender(SenderHandle);

impl HistorySender {
    pub(super) fn new(sender: SenderHandle) -> Self {
        Self(sender)
    }

    fn request(&self, lane: CanonicalUuid, before: wire::MessageLocus) {
        send_action(&self.0, ClientAction::MessageHistory { lane, before });
    }
}

impl PartialEq for HistorySender {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub(super) enum ActivityCard {
    Agent {
        member: crate::runtime::LaneMember,
        quote_lane: CanonicalUuid,
        message: Rc<wire::AgentMessage>,
    },
}

impl PartialEq for ActivityCard {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Agent {
                    member: left_member,
                    quote_lane: left_quote_lane,
                    message: left_message,
                },
                Self::Agent {
                    member: right_member,
                    quote_lane: right_quote_lane,
                    message: right_message,
                },
            ) => {
                left_member.lane == right_member.lane
                    && left_member.name == right_member.name
                    && left_member.worktree == right_member.worktree
                    && left_quote_lane == right_quote_lane
                    && (Rc::ptr_eq(left_message, right_message) || left_message == right_message)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct ActivityFingerprint {
    worktree: String,
    message: Rc<wire::AgentMessage>,
}

impl PartialEq for ActivityFingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.worktree == other.worktree
            && (Rc::ptr_eq(&self.message, &other.message) || self.message == other.message)
    }
}

impl ActivityCard {
    fn key(&self) -> String {
        match self {
            Self::Agent { message, .. } => agent_message_source_id(message),
        }
    }

    fn measurement_fingerprint(&self) -> ActivityFingerprint {
        match self {
            Self::Agent {
                member, message, ..
            } => ActivityFingerprint {
                worktree: member.worktree.clone(),
                message: Rc::clone(message),
            },
        }
    }

    fn base_span(&self, geometry: &crate::mosaic::Geometry) -> usize {
        match self {
            Self::Agent { message, .. }
                if message.activity == wire::AgentActivityKind::Compaction =>
            {
                crate::mosaic::TRACK_COUNT
            }
            Self::Agent { .. } => geometry.base_span,
        }
    }

    fn is_boundary(&self) -> bool {
        matches!(
            self,
            Self::Agent { message, .. }
                if message.activity == wire::AgentActivityKind::Compaction
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ActivityMeasurement {
    width: i32,
    key: String,
    fingerprint: ActivityFingerprint,
    image_revision: u64,
    candidates: Vec<crate::mosaic::Candidate>,
    latched: Option<crate::mosaic::Candidate>,
    spans: Vec<usize>,
    widths: Vec<i32>,
    heights: Vec<i32>,
}

#[derive(Clone, PartialEq)]
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
    pub(super) measured_spans: String,
    pub(super) measured_widths: String,
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
            measured_spans: String::new(),
            measured_widths: String::new(),
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
            measured_spans: measurement
                .spans
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(","),
            measured_widths: measurement
                .widths
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
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
    let style = element.style();
    if style
        .get_property_value(property)
        .is_ok_and(|current| current == value)
    {
        return;
    }
    style
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
    use_effect_with((host, width, keys, plane), move |(host, width, _, _)| {
        if *width < MIN_MEASURABLE_WIDTH_PX {
            return;
        }
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

struct MeasurementIndex<'a> {
    by_key: BTreeMap<&'a str, Vec<&'a ActivityMeasurement>>,
}

impl<'a> MeasurementIndex<'a> {
    fn new(measurements: &'a [ActivityMeasurement]) -> Self {
        let mut by_key = BTreeMap::<&str, Vec<&ActivityMeasurement>>::new();
        for measurement in measurements {
            by_key
                .entry(measurement.key.as_str())
                .or_default()
                .push(measurement);
        }
        Self { by_key }
    }

    fn get(
        &self,
        key: &str,
        width: i32,
        activity: &ActivityCard,
        image_revision: u64,
    ) -> Option<&'a ActivityMeasurement> {
        let fingerprint = activity.measurement_fingerprint();
        self.by_key.get(key)?.iter().copied().find(|measurement| {
            measurement.width == width
                && measurement.fingerprint == fingerprint
                && measurement.image_revision == image_revision
        })
    }

    fn latest(&self, key: &str, activity: &ActivityCard) -> Option<&'a ActivityMeasurement> {
        let fingerprint = activity.measurement_fingerprint();
        self.by_key
            .get(key)?
            .iter()
            .rev()
            .copied()
            .find(|measurement| measurement.fingerprint == fingerprint)
    }
}

fn activity_measurement<'a>(
    index: &MeasurementIndex<'a>,
    width: i32,
    activity: &ActivityCard,
    image_revision: u64,
) -> Option<&'a ActivityMeasurement> {
    let key = activity.key();
    index.get(&key, width, activity, image_revision)
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
    pub(super) history_more: BTreeSet<CanonicalUuid>,
    pub(super) history: HistorySender,
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
                tabindex="0"
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
    let history_targets = history_targets(&properties.cards, &properties.history_more);
    use_history_paging(host.clone(), history_targets, properties.history.clone());
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
    use_data_image_open_events(host.clone());
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
    use_plane_scroll(host.clone(), (*committed).clone());
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

#[derive(Clone, PartialEq)]
struct HistoryTarget {
    lane: CanonicalUuid,
    before: wire::MessageLocus,
    key: String,
}

fn history_targets(cards: &[ActivityCard], more: &BTreeSet<CanonicalUuid>) -> Vec<HistoryTarget> {
    let mut seen = BTreeSet::new();
    cards
        .iter()
        .filter_map(|card| match card {
            ActivityCard::Agent { message, .. }
                if more.contains(&message.lane) && seen.insert(message.lane) =>
            {
                Some(HistoryTarget {
                    lane: message.lane,
                    before: message.locus,
                    key: card.key(),
                })
            }
            ActivityCard::Agent { .. } => None,
        })
        .collect()
}

#[hook]
fn use_history_paging(host: NodeRef, targets: Vec<HistoryTarget>, request: HistorySender) {
    let requested = use_mut_ref(HashSet::<String>::new);
    use_effect_with((host, targets, request), move |(host, targets, request)| {
        let registration = host.cast::<web_sys::Element>().and_then(|element| {
            let current = targets
                .iter()
                .map(|target| target.key.clone())
                .collect::<HashSet<_>>();
            requested.borrow_mut().retain(|key| current.contains(key));
            let observed = element.clone();
            let observed_targets = targets.clone();
            let observed_requested = requested.clone();
            let observed_request = request.clone();
            let inspect = Rc::new(move || {
                let viewport = observed.get_bounding_client_rect();
                for target in &observed_targets {
                    let selector = format!("[data-mosaic-card-key=\"{}\"]", target.key);
                    let Ok(Some(card)) = observed.query_selector(&selector) else {
                        continue;
                    };
                    let bounds = card.get_bounding_client_rect();
                    if bounds.top() > viewport.bottom() + 400.0
                        || bounds.bottom() < viewport.top() - 400.0
                        || !observed_requested.borrow_mut().insert(target.key.clone())
                    {
                        continue;
                    }
                    observed_request.request(target.lane, target.before);
                }
            });
            inspect();
            let on_scroll = {
                let inspect = Rc::clone(&inspect);
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_| inspect())
            };
            element
                .add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref())
                .ok()?;
            Some((element, on_scroll))
        });
        move || {
            if let Some((element, on_scroll)) = registration {
                let _ = element.remove_event_listener_with_callback(
                    "scroll",
                    on_scroll.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

#[hook]
/// Preserve the viewport only after live measurement produces a ready plane.
///
/// A temporary `None` during incremental measurement deliberately retains the
/// prior ready-plane state, so the eventual row delta has one authoritative
/// baseline and one scroll owner.
fn use_plane_scroll(
    host: NodeRef,
    plane: Option<crate::mosaic_plane::PersistentLanePlane<String>>,
) {
    let anchor = use_mut_ref(|| None::<ScrollAnchor>);
    {
        let anchor = anchor.clone();
        use_effect_with(host.clone(), move |host| {
            let registration = host.cast::<web_sys::Element>().and_then(|scroller| {
                let live_scroller = scroller.clone();
                let live_anchor = anchor.clone();
                let listener = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                    *live_anchor.borrow_mut() = visible_scroll_anchor(&live_scroller);
                });
                scroller
                    .add_event_listener_with_callback("scroll", listener.as_ref().unchecked_ref())
                    .ok()?;
                *anchor.borrow_mut() = visible_scroll_anchor(&scroller);
                Some((scroller, listener))
            });
            move || {
                if let Some((scroller, listener)) = registration {
                    let _ = scroller.remove_event_listener_with_callback(
                        "scroll",
                        listener.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }
    use_effect_with((host, plane), move |(host, plane)| {
        let Some(scroller) = host.cast::<web_sys::Element>() else {
            return;
        };
        if plane.is_none()
            || !crate::mosaic::pane_is_scrolled(f64::from(scroller.scroll_top()), None)
        {
            *anchor.borrow_mut() = None;
            return;
        }
        if let Some(previous) = anchor.borrow().clone()
            && let Some(current_offset) = scroll_anchor_offset(&scroller, &previous.key)
        {
            let delta = current_offset - previous.offset;
            if delta.abs() >= 0.5 {
                scroller.set_scroll_top((f64::from(scroller.scroll_top()) + delta).round() as i32);
            }
        }
        *anchor.borrow_mut() = visible_scroll_anchor(&scroller);
    });
}

#[derive(Clone, Debug, PartialEq)]
struct ScrollAnchor {
    key: String,
    offset: f64,
}

fn committed_scroll_cards(scroller: &web_sys::Element) -> Vec<web_sys::Element> {
    let cards = scroller
        .query_selector_all(".mosaic-message-card[data-layout-phase=\"committed\"]")
        .expect("the fixed committed Mosaic selector is valid");
    (0..cards.length())
        .filter_map(|index| cards.item(index))
        .filter_map(|card| card.dyn_into::<web_sys::Element>().ok())
        .collect()
}

fn visible_scroll_anchor(scroller: &web_sys::Element) -> Option<ScrollAnchor> {
    if !crate::mosaic::pane_is_scrolled(f64::from(scroller.scroll_top()), None) {
        return None;
    }
    let viewport = scroller.get_bounding_client_rect();
    committed_scroll_cards(scroller)
        .into_iter()
        .filter_map(|card| {
            let rect = card.get_bounding_client_rect();
            (rect.bottom() > viewport.top() && rect.top() < viewport.bottom()).then(|| {
                let offset = rect.top() - viewport.top();
                let distance = offset.abs();
                let key = card
                    .get_attribute("data-mosaic-card-key")
                    .expect("a committed Mosaic card has a stable key");
                (distance, ScrollAnchor { key, offset })
            })
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, anchor)| anchor)
}

fn scroll_anchor_offset(scroller: &web_sys::Element, key: &str) -> Option<f64> {
    let viewport_top = scroller.get_bounding_client_rect().top();
    committed_scroll_cards(scroller)
        .into_iter()
        .find(|card| card.get_attribute("data-mosaic-card-key").as_deref() == Some(key))
        .map(|card| card.get_bounding_client_rect().top() - viewport_top)
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
        .map(|card| match card {
            ActivityCard::Agent { message, .. } => NarrationMessage {
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
            },
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

#[derive(Clone)]
struct ImageResolutionBatch {
    pending_keys: Rc<RefCell<HashSet<String>>>,
    scheduled: Rc<RefCell<Option<Timeout>>>,
    revisions: UseReducerHandle<ImageRevisions>,
    measurements: UseStateHandle<Vec<ActivityMeasurement>>,
    host: web_sys::Element,
    live: Rc<RefCell<ImageResolutionSnapshot>>,
}

impl ImageResolutionBatch {
    fn new(
        revisions: UseReducerHandle<ImageRevisions>,
        measurements: UseStateHandle<Vec<ActivityMeasurement>>,
        host: web_sys::Element,
        live: Rc<RefCell<ImageResolutionSnapshot>>,
    ) -> Self {
        Self {
            pending_keys: Rc::new(RefCell::new(HashSet::new())),
            scheduled: Rc::new(RefCell::new(None)),
            revisions,
            measurements,
            host,
            live,
        }
    }

    fn queue(&self, key: String) {
        self.pending_keys.borrow_mut().insert(key);
        if self.scheduled.borrow().is_some() {
            return;
        }
        let batch = self.clone();
        let timeout = Timeout::new(0, move || {
            batch.scheduled.borrow_mut().take();
            batch.flush();
        });
        *self.scheduled.borrow_mut() = Some(timeout);
    }

    fn flush(&self) {
        let mut keys = self.pending_keys.borrow_mut().drain().collect::<Vec<_>>();
        keys.sort();
        let mut snapshot = self.live.borrow_mut();
        let width = element_content_width(&self.host)
            .map(|measured| measured.round() as i32)
            .unwrap_or(snapshot.width);
        let mut revised_keys = Vec::with_capacity(keys.len());
        let mut measurements_changed = false;
        for key in keys {
            if let Some(measured) = self.resolve_key(&mut snapshot, &key, width) {
                revised_keys.push(key);
                measurements_changed |= measured;
            }
        }
        let next_measurements = measurements_changed.then(|| snapshot.measurements.clone());
        drop(snapshot);
        if let Some(next_measurements) = next_measurements {
            self.measurements.set(next_measurements);
        }
        for key in revised_keys {
            self.revisions.dispatch(key);
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
        let Some(key) = image
            .closest(".mosaic-message-card")
            .ok()
            .flatten()
            .and_then(|card| card.get_attribute("data-mosaic-card-key"))
        else {
            return;
        };
        self.queue(key);
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

    fn cancel(&self) {
        self.scheduled.borrow_mut().take();
        self.pending_keys.borrow_mut().clear();
    }
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
fn use_image_resolution_events(
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
            let batch = ImageResolutionBatch::new(
                revisions.clone(),
                measurements.clone(),
                host.clone(),
                live.clone(),
            );
            let event_batch = batch.clone();
            let resolved =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let Some(image) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlImageElement>().ok())
                    else {
                        return;
                    };
                    event_batch.resolve_image(image);
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
            batch.scan_complete();
            Some((host, resolved, batch))
        });
        move || {
            if let Some((host, resolved, batch)) = registration {
                let callback = resolved.as_ref().unchecked_ref();
                let _ = host.remove_event_listener_with_callback_and_bool("load", callback, true);
                let _ = host.remove_event_listener_with_callback_and_bool("error", callback, true);
                batch.cancel();
            }
        }
    });
}

#[hook]
fn use_data_image_open_events(host: NodeRef) {
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
                let measurement_index = MeasurementIndex::new(&prior);
                let committed_spans = committed_spans
                    .iter()
                    .map(|(key, span)| (key.as_str(), *span))
                    .collect::<BTreeMap<_, _>>();
                let unknown = cards
                    .iter()
                    .enumerate()
                    .filter_map(|(index, activity)| {
                        let key = activity.key();
                        let image_revision = image_revisions.get(&key);
                        activity_measurement(&measurement_index, *width, activity, image_revision)
                            .is_none()
                            .then(|| {
                                let latched_span = committed_spans.get(key.as_str()).copied();
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
    element
        .set_attribute("data-mosaic-measurement-active", "true")
        .expect("the fixed Mosaic measurement attribute is valid");
    let mut spans = Vec::with_capacity(3);
    let mut widths = Vec::with_capacity(3);
    let mut heights = Vec::with_capacity(3);
    let mut measure = |span: usize| {
        set_measure_span(element, span);
        spans.push(span);
        widths.push(element.offset_width());
        let height = element.offset_height();
        heights.push(height);
        f64::from(height)
    };
    let candidates = if activity.base_span(&geometry) == crate::mosaic::TRACK_COUNT {
        let height = measure(crate::mosaic::TRACK_COUNT);
        vec![crate::mosaic::Candidate {
            span: crate::mosaic::TRACK_COUNT,
            rows: if activity.is_boundary() {
                crate::mosaic::boundary_rows_for(height, geometry.gap, geometry.module)
            } else {
                crate::mosaic::rows_for(height, geometry.gap, geometry.module)
            },
        }]
    } else {
        crate::mosaic::span_candidates(&geometry, &mut measure)
    };
    let latched = latched_span
        .filter(|span| candidates.iter().all(|candidate| candidate.span != *span))
        .map(|span| {
            let height = measure(span);
            crate::mosaic::Candidate {
                span,
                rows: crate::mosaic::rows_for(height, geometry.gap, geometry.module),
            }
        });
    set_measure_span(element, activity.base_span(&geometry));
    element
        .remove_attribute("data-mosaic-measurement-active")
        .expect("the fixed Mosaic measurement attribute is valid");
    Some(ActivityMeasurement {
        width,
        key: activity.key(),
        fingerprint: activity.measurement_fingerprint(),
        image_revision,
        candidates,
        latched,
        spans,
        widths,
        heights,
    })
}

fn set_measure_span(element: &web_sys::HtmlElement, span: usize) {
    let span = span.to_string();
    element
        .set_attribute("data-measure-span", &span)
        .expect("the fixed Mosaic measurement attribute is valid");
    element
        .style()
        .set_property("--mosaic-measure-span", &span)
        .expect("Mosaic measurement writes its CSP-safe span property");
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
        if image.get_attribute("data-mosaic-image-state").is_none() {
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
    let measurement_index = MeasurementIndex::new(measurements);
    (width >= MIN_MEASURABLE_WIDTH_PX)
        .then(|| {
            cards
                .iter()
                .map(|activity| {
                    activity_measurement(
                        &measurement_index,
                        width,
                        activity,
                        image_revisions.get(&activity.key()),
                    )
                    .map(|measurement| crate::mosaic_plane::MeasuredCard {
                        key: measurement.key.clone(),
                        candidates: measurement.candidates.clone(),
                        latched: measurement.latched,
                        content_revision: measurement.image_revision,
                        boundary: activity.is_boundary(),
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
    let cards_by_key = properties
        .cards
        .iter()
        .map(|activity| (activity.key(), activity))
        .collect::<BTreeMap<_, _>>();
    let placements = plane
        .into_iter()
        .flat_map(crate::mosaic_plane::PersistentLanePlane::cards)
        .map(|entry| (entry.key.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let measurement_index = MeasurementIndex::new(measurements);
    let mut ordered = plane
        .into_iter()
        .flat_map(|plane| plane.cards())
        .filter_map(|entry| cards_by_key.get(&entry.key).copied())
        .collect::<Vec<_>>();
    let unknown = properties
        .cards
        .iter()
        .filter(|activity| !placements.contains_key(activity.key().as_str()))
        .collect::<Vec<_>>();
    ordered.extend(unknown);
    ordered
        .into_iter()
        .enumerate()
        .map(|(order, activity)| {
            let key = activity.key();
            let image_revision = image_revisions.get(&key);
            let placement = plane.zip(placements.get(key.as_str()).copied());
            let measurement = placement
                .and_then(|(plane, _)| {
                    measurement_index.get(&key, plane.geometry(), activity, image_revision)
                })
                .or_else(|| measurement_index.latest(&key, activity));
            let layout = match (placement, measurement) {
                (Some((_plane, entry)), Some(measurement)) => {
                    CardLayout::placed(entry.card.creation_index, &entry.card, measurement)
                }
                _ => CardLayout::measuring(order, activity.base_span(geometry)),
            };
            let quote_order = u64::try_from(layout.measurement_index)
                .expect("Mosaic creation index fits the quote ordering field");
            match activity {
                ActivityCard::Agent {
                    member,
                    quote_lane,
                    message,
                } => html! {
                    <AgentMessageCard
                        key={key}
                        member={member.clone()}
                        quote_lane={*quote_lane}
                        message={message.clone()}
                        quote_dispatcher={properties.quote_dispatcher.clone()}
                        speech_queue={properties.speak.clone()}
                        speech={properties.speech}
                        lane={properties.lane}
                        order={quote_order}
                        {layout}
                    />
                },
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
