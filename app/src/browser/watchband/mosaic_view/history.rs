//! Member-local progressive history requests for one merged Mosaic plane.

use std::collections::HashSet;

use super::*;

/// Cold advertised lanes hydrate this many visible cards before geometry gates paging.
const EAGER_HISTORY_CARD_TARGET: usize = 25;

#[derive(Clone)]
pub(in crate::browser::watchband) struct HistorySender(SenderHandle);

impl HistorySender {
    pub(in crate::browser::watchband) fn new(sender: SenderHandle) -> Self {
        Self(sender)
    }

    fn request(&self, lane: CanonicalUuid, before: wire::MessageLocus) {
        let _admitted = admit_action(&self.0, ClientAction::MessageHistory { lane, before });
    }
}

impl PartialEq for HistorySender {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0.queue, &other.0.queue)
    }
}

#[derive(Clone, PartialEq)]
struct HistoryTarget {
    lane: CanonicalUuid,
    before: wire::MessageLocus,
    eager: bool,
    key: String,
    request_key: String,
}

fn history_targets(
    cards: &[ActivityCard],
    targets: &BTreeMap<CanonicalUuid, (wire::MessageLocus, u64)>,
    revision: u64,
) -> Vec<HistoryTarget> {
    targets
        .iter()
        .filter_map(|(lane, (before, completion))| {
            let lane_cards = cards
                .iter()
                .filter(|card| {
                    matches!(card, ActivityCard::Agent { message, .. } if message.lane == *lane)
                })
                .count();
            cards.iter().find_map(|card| match card {
                ActivityCard::Agent { message, .. }
                    if message.lane == *lane && message.locus == *before =>
                {
                    let key = card.key();
                    Some(HistoryTarget {
                        lane: *lane,
                        before: *before,
                        eager: lane_cards < EAGER_HISTORY_CARD_TARGET,
                        request_key: format!("{revision}:{completion}:{key}"),
                        key,
                    })
                }
                ActivityCard::Agent { .. } => None,
                ActivityCard::Lifecycle { .. } => None,
            })
        })
        .collect()
}

#[hook]
pub(super) fn use_history_paging(
    host: NodeRef,
    cards: &[ActivityCard],
    targets: &BTreeMap<CanonicalUuid, (wire::MessageLocus, u64)>,
    revision: u64,
    request: HistorySender,
) {
    let targets = history_targets(cards, targets, revision);
    let requested = use_mut_ref(HashSet::<String>::new);
    use_effect_with((host, targets, request), move |(host, targets, request)| {
        let registration = host.cast::<web_sys::Element>().and_then(|element| {
            let current = targets
                .iter()
                .map(|target| target.request_key.clone())
                .collect::<HashSet<_>>();
            requested.borrow_mut().retain(|key| current.contains(key));
            let observed = element.clone();
            let observed_targets = targets.clone();
            let observed_requested = requested.clone();
            let observed_request = request.clone();
            let inspect = Rc::new(move || {
                let at_edge = crate::mosaic::history_edge_is_near(
                    f64::from(observed.scroll_top()),
                    f64::from(observed.client_height()),
                    f64::from(observed.scroll_height()),
                    None,
                );
                let viewport = observed.get_bounding_client_rect();
                for target in &observed_targets {
                    let selector = format!("[data-mosaic-card-key=\"{}\"]", target.key);
                    let target_is_near = observed
                        .query_selector(&selector)
                        .ok()
                        .flatten()
                        .is_some_and(|card| {
                            let bounds = card.get_bounding_client_rect();
                            bounds.top() <= viewport.bottom() + 400.0
                                && bounds.bottom() >= viewport.top() - 400.0
                        });
                    // Keep visible-cursor prefetching, but let the merged plane's
                    // trailing edge page a cursor buried above a deeper sibling column.
                    if !target.eager && !at_edge && !target_is_near {
                        continue;
                    }
                    if !observed_requested
                        .borrow_mut()
                        .insert(target.request_key.clone())
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
