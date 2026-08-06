//! One independently measured and compensated Mosaic plane per lineage.

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
    use_narration(
        properties.speech,
        properties.speak.clone(),
        activity_messages(&properties.cards),
    );
    let geometry = crate::mosaic::Geometry::new(16.0, *width);
    let candidate_sets = properties
        .cards
        .iter()
        .map(|card| activity_candidates(card, &geometry))
        .collect::<Vec<_>>();
    let plane = crate::mosaic_plane::arrange(&candidate_sets);
    use_plane_scroll(
        host.clone(),
        plane.max_row,
        geometry.module,
        properties.cards.len(),
    );
    let rendered_cards = render_plane_cards(properties, &plane);
    html! {
        <section
            ref={host}
            class="lane-mosaic-plane"
            data-agent-plane={properties.lane.to_string()}
            data-lineage-policy={properties.life.label()}
            data-lineage-members={properties.members.to_string()}
            data-lanes={geometry.lane_count.to_string()}
            data-plane-rows={plane.rows().to_string()}
            data-max-row={plane.max_row.to_string()}
            data-module={geometry.module.to_string()}
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

fn activity_messages(cards: &[ActivityCard]) -> Vec<(String, String)> {
    cards
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
        .collect()
}

#[hook]
fn use_narration(speech: LineageSpeech, speak: Callback<String>, messages: Vec<(String, String)>) {
    let narration = use_mut_ref(|| (false, BTreeMap::<String, ()>::new()));
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
fn use_plane_scroll(host: NodeRef, max_row: i32, module: i32, card_count: usize) {
    let scroll_state = use_mut_ref(|| None::<(i32, i32, usize)>);
    use_effect_with(
        (host, max_row, module, card_count),
        move |(host, max_row, module, card_count)| {
            let previous = *scroll_state.borrow();
            if let Some((previous_max_row, previous_module, previous_card_count)) = previous
                && let Some(scroller) = host.cast::<web_sys::Element>()
            {
                let scroll_top = f64::from(scroller.scroll_top());
                let replaying = *module != previous_module || *card_count <= previous_card_count;
                if let Some(delta) = crate::mosaic::scroll_compensation(
                    previous_max_row,
                    *max_row,
                    *module,
                    crate::mosaic::pane_is_scrolled(scroll_top, None),
                    replaying,
                ) {
                    scroller.set_scroll_top((scroll_top + delta).round() as i32);
                }
            }
            *scroll_state.borrow_mut() = Some((*max_row, *module, *card_count));
        },
    );
}

fn render_plane_cards(
    properties: &MosaicPlaneProps,
    plane: &crate::mosaic_plane::LanePlane,
) -> Html {
    properties
        .cards
        .iter()
        .zip(plane.cards.iter())
        .enumerate()
        .map(|(order, (activity, card))| match activity {
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
                card,
                plane.max_row,
            ),
            ActivityCard::Directive { lane, directive } => render_directive_card(
                *lane,
                directive,
                &lane.to_string(),
                &properties.drafts,
                &properties.speak,
                order as u64,
                card,
                plane.max_row,
            ),
        })
        .collect()
}

fn activity_candidates(
    activity: &ActivityCard,
    geometry: &crate::mosaic::Geometry,
) -> Vec<crate::mosaic::Candidate> {
    let (text, attachment_height) = match activity {
        ActivityCard::Agent { message, .. } => (message.text.as_str(), 0.0),
        ActivityCard::Directive { directive, .. } => {
            let height = directive
                .attachments
                .iter()
                .filter(|attachment| !directive.text.contains(&attachment.data_url))
                .map(|attachment| {
                    if attachment.content_type.starts_with("image/") {
                        crate::mosaic::Reservation::Image.pixels(16.0) + 10.0
                    } else {
                        42.0
                    }
                })
                .sum::<f64>();
            (directive.text.as_str(), height)
        }
    };
    crate::mosaic::span_candidates(geometry, |span| {
        let width = f64::from(geometry.card_width(0, span).max(160));
        let characters_per_line = (width / 7.2).floor().max(18.0) as usize;
        let text_lines = text
            .lines()
            .map(|line| line.chars().count().max(1).div_ceil(characters_per_line))
            .sum::<usize>()
            .max(1);
        54.0 + text_lines as f64 * 19.0 + attachment_height
    })
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
