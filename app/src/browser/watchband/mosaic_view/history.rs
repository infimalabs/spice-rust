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

    fn request(&self, cursor: wire::MessageHistoryCursor) -> bool {
        admit_action(
            &self.0,
            ClientAction::MessageHistory {
                generation: cursor.generation,
                lane: cursor.lane,
                before: cursor.before,
            },
        )
    }
}

impl PartialEq for HistorySender {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0.queue, &other.0.queue)
    }
}

#[derive(Clone, PartialEq)]
struct HistoryTarget {
    cursor: wire::MessageHistoryCursor,
    eager: bool,
    key: String,
    request_key: String,
}

fn history_targets(
    cards: &[ActivityCard],
    targets: &BTreeMap<CanonicalUuid, (wire::MessageHistoryCursor, u64)>,
    revision: u64,
) -> Vec<HistoryTarget> {
    targets
        .iter()
        .map(|(lane, (cursor, completion))| {
            let lane_cards = cards
                .iter()
                .filter(|card| {
                    matches!(card, ActivityCard::Agent { message, .. } if message.lane == *lane)
                })
                .count();
            let key = cursor
                .before
                .and_then(|before| {
                    cards.iter().find_map(|card| match card {
                        ActivityCard::Agent { message, .. }
                            if message.lane == *lane && message.locus == before =>
                        {
                            Some(card.key())
                        }
                        ActivityCard::Agent { .. }
                        | ActivityCard::Operator { .. }
                        | ActivityCard::Lifecycle { .. } => None,
                    })
                })
                .or_else(|| {
                    cards.iter().find_map(|card| match card {
                        ActivityCard::Agent { message, .. } if message.lane == *lane => {
                            Some(card.key())
                        }
                        ActivityCard::Agent { .. }
                        | ActivityCard::Operator { .. }
                        | ActivityCard::Lifecycle { .. } => None,
                    })
                })
                .unwrap_or_else(|| format!("history-bootstrap-{lane}"));
            HistoryTarget {
                cursor: *cursor,
                eager: cursor.before.is_none() || lane_cards < EAGER_HISTORY_CARD_TARGET,
                request_key: format!("{revision}:{completion}:{key}"),
                key,
            }
        })
        .collect()
}

fn rotation_start(targets: &[HistoryTarget], last: Option<CanonicalUuid>) -> usize {
    last.and_then(|last| {
        targets
            .iter()
            .position(|target| target.cursor.lane == last)
            .map(|index| (index + 1) % targets.len())
    })
    .unwrap_or(0)
}

#[hook]
pub(super) fn use_history_paging(
    host: NodeRef,
    cards: &[ActivityCard],
    targets: &BTreeMap<CanonicalUuid, (wire::MessageHistoryCursor, u64)>,
    revision: u64,
    request: HistorySender,
) {
    let targets = history_targets(cards, targets, revision);
    let requested = use_mut_ref(HashSet::<String>::new);
    let last_admitted = use_mut_ref(|| None::<CanonicalUuid>);
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
            let observed_last_admitted = last_admitted.clone();
            let inspect = Rc::new(move || {
                // A plane hidden behind another destination has no viewport: its
                // zero extent is not a trailing edge, so it pages nothing until shown.
                if observed.client_height() == 0 {
                    return;
                }
                let at_edge = crate::mosaic::history_edge_is_near(
                    f64::from(observed.scroll_top()),
                    f64::from(observed.client_height()),
                    f64::from(observed.scroll_height()),
                    None,
                );
                let viewport = observed.get_bounding_client_rect();
                let start = rotation_start(&observed_targets, *observed_last_admitted.borrow());
                for offset in 0..observed_targets.len() {
                    let target = &observed_targets[(start + offset) % observed_targets.len()];
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
                    // Page visible cursors and those buried above a deeper sibling column.
                    if !target.eager && !at_edge && !target_is_near {
                        continue;
                    }
                    if !observed_requested
                        .borrow_mut()
                        .insert(target.request_key.clone())
                    {
                        continue;
                    }
                    if observed_request.request(target.cursor) {
                        *observed_last_admitted.borrow_mut() = Some(target.cursor.lane);
                    } else {
                        observed_requested.borrow_mut().remove(&target.request_key);
                    }
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
            // Showing the plane again gives it a viewport without scrolling it.
            let on_resize =
                Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(move |_, _| {
                    inspect()
                });
            let resized = web_sys::ResizeObserver::new(on_resize.as_ref().unchecked_ref()).ok()?;
            resized.observe(&element);
            Some((element, on_scroll, resized, on_resize))
        });
        move || {
            if let Some((element, on_scroll, resized, _on_resize)) = registration {
                resized.disconnect();
                let _ = element.remove_event_listener_with_callback(
                    "scroll",
                    on_scroll.as_ref().unchecked_ref(),
                );
            }
        }
    });
}
