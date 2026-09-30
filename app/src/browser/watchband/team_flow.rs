//! Team Dash workflow, lifecycle, and delivered-direction chronology.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

use super::*;
const OLDER_PAGE_MARGIN_PX: i32 = 96;
const DIRECTION_LABEL_BUDGET: usize = 120;

#[derive(Clone, PartialEq)]
pub(super) enum PageRequest {
    Head(crate::TaskTimelineKey),
    Older(crate::TaskTimelineKey),
}

#[derive(Properties, PartialEq)]
pub(super) struct TeamFlowProps {
    pub(super) team: crate::runtime::LaneRow,
    pub(super) standings: Vec<Standing>,
    pub(super) surface: UseStateHandle<SurfaceData>,
    pub(super) revision: u64,
    pub(super) select: Callback<wire::TaskStamp>,
    pub(super) show_chat: Callback<MouseEvent>,
    pub(super) request: Callback<PageRequest>,
}

#[function_component(TeamFlow)]
pub(super) fn team_flow(properties: &TeamFlowProps) -> Html {
    let key = crate::TaskTimelineKey::new(properties.team.members.iter().map(|member| member.lane));
    let host = use_node_ref();
    use_scroll_anchor(host.clone());

    {
        let key = key.clone();
        let request = properties.request.clone();
        use_effect_with((key, properties.revision), move |(key, _)| {
            if let Some(key) = key.clone() {
                request.emit(PageRequest::Head(key));
            }
        });
    }

    let surface = properties.surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty_flow(host, "Task timeline unavailable.");
    };
    let timeline = key.as_ref().and_then(|key| runtime.task_timeline(key));
    let mut events = Vec::new();
    if let Some(timeline) = timeline {
        events.extend(timeline.entries().iter().map(FlowEvent::Task));
    }
    events.extend(
        runtime
            .lifecycle_boundaries()
            .iter()
            .filter(|boundary| properties.team.member(boundary.lane()).is_some())
            .map(FlowEvent::Lifecycle),
    );
    for message in runtime
        .messages()
        .iter()
        .filter(|message| properties.team.member(message.lane).is_some())
    {
        events.extend(
            message
                .directive_contexts
                .iter()
                .enumerate()
                .map(|(index, context)| FlowEvent::Direction {
                    message,
                    context,
                    index,
                }),
        );
    }
    events.sort_by_key(|event| Reverse(event.display_millis()));

    let request_older = {
        let request = properties.request.clone();
        let key = key.clone();
        let next = timeline.and_then(crate::TaskTimelineState::next);
        let loading = timeline.is_some_and(crate::TaskTimelineState::loading_older);
        Callback::from(move |event: Event| {
            let Some(element) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                return;
            };
            let remaining =
                element.scroll_height() - element.client_height() - element.scroll_top();
            if remaining <= OLDER_PAGE_MARGIN_PX
                && next.is_some()
                && !loading
                && let Some(key) = key.clone()
            {
                request.emit(PageRequest::Older(key));
            }
        })
    };
    let issue = timeline.and_then(crate::TaskTimelineState::issue);
    let loading = timeline.is_none_or(crate::TaskTimelineState::loading);
    let empty = events.is_empty();
    // An empty Flow states its loading or failure as its content. A populated
    // one reports an older page's progress, or any read's failure until the
    // next read, over its rows rather than inside them.
    let status = if empty {
        None
    } else if let Some(issue) = issue {
        Some((issue, true))
    } else {
        timeline
            .is_some_and(crate::TaskTimelineState::loading_older)
            .then_some(("Loading older activity…", false))
    };
    html! {
        <>
            <div
                ref={host}
                class="team-dash-flow-body"
                data-empty={empty.to_string()}
                onscroll={request_older}
            >
                <ol class="team-dash-flow-list" aria-live="polite">
                    {for events.into_iter().map(|event| render_event(
                        event,
                        &properties.standings,
                        &properties.select,
                        &properties.show_chat,
                    ))}
                </ol>
                if empty {
                    <p class="team-dash-flow-empty" role={issue.is_some().then_some("status")}>
                        {issue.unwrap_or(if loading { "Loading team flow…" } else { "No team activity yet." })}
                    </p>
                }
            </div>
            {render_status(status)}
        </>
    }
}

/// The populated Flow's read status, laid over the bottom of the panel so its
/// scroll content never changes size. The live region stays mounted, empty
/// when there is nothing to report, so every report is announced.
fn render_status(status: Option<(&str, bool)>) -> Html {
    html! {
        <p
            class="team-dash-flow-status"
            role="status"
            data-shown={status.is_some().to_string()}
            data-issue={status.is_some_and(|(_, issue)| issue).to_string()}
        >{status.map(|(text, _)| text.to_owned())}</p>
    }
}

fn empty_flow(host: NodeRef, message: &'static str) -> Html {
    html! {
        <>
            <div ref={host} class="team-dash-flow-body" data-empty="true">
                <ol class="team-dash-flow-list"></ol>
                <p class="team-dash-flow-empty" role="status">{message}</p>
            </div>
            {render_status(None)}
        </>
    }
}

enum FlowEvent<'a> {
    Task(&'a wire::intent::TaskTimelineEntry),
    Lifecycle(&'a crate::runtime::LaneLifecycleBoundary),
    Direction {
        message: &'a wire::AgentMessage,
        context: &'a wire::MessageDirective,
        index: usize,
    },
}

impl FlowEvent<'_> {
    fn lane(&self) -> wire::CanonicalUuid {
        match self {
            Self::Task(entry) => entry.lane,
            Self::Lifecycle(boundary) => boundary.lane(),
            Self::Direction { message, .. } => message.lane,
        }
    }

    fn display_millis(&self) -> i64 {
        match self {
            Self::Task(entry) => entry.at.get().as_millisecond(),
            Self::Lifecycle(boundary) => boundary.observed().get().as_millisecond(),
            Self::Direction { context, .. } => context.authored.get().as_millisecond(),
        }
    }
}

fn render_event(
    event: FlowEvent<'_>,
    standings: &[Standing],
    select: &Callback<wire::TaskStamp>,
    show_chat: &Callback<MouseEvent>,
) -> Html {
    let lane = event.lane();
    let Some(standing) = standings.iter().find(|standing| standing.lane == lane) else {
        return Html::default();
    };
    let accent = standing.accent;
    let member = standing.label.clone();
    match event {
        FlowEvent::Task(entry) => {
            let task = entry.task.clone();
            let activate = select.reform(move |_| task.clone());
            let action = task_event_label(&entry.event);
            html! {
                <li
                    key={format!("task-flow-{}", entry.position)}
                    class="team-dash-flow-row"
                    data-flow-kind="task"
                    data-agent-lane={lane.to_string()}
                    data-accent={accent.to_string()}
                >
                    <button type="button" onclick={activate}>
                        <span class="team-dash-flow-member">{member}</span>
                        <span class="team-dash-flow-copy">
                            <strong>{action}</strong>
                            <span>{format!("{} · {}", entry.handle, entry.title)}</span>
                        </span>
                        <Age authored={entry.at.get().as_millisecond()} class="team-dash-flow-age" />
                    </button>
                </li>
            }
        }
        FlowEvent::Lifecycle(boundary) => {
            let target = format!("lane-lifecycle-{}", boundary.seq());
            let origin = format!("team-dash-flow-{target}");
            html! {
                <li
                    key={origin.clone()}
                    class="team-dash-flow-row"
                    data-flow-kind="lifecycle"
                    data-agent-lane={lane.to_string()}
                    data-accent={accent.to_string()}
                >
                    <a
                        id={origin}
                        data-dash-return=""
                        data-dash-return-origin=""
                        href={crate::view::card_href(&target)}
                        onclick={show_chat.clone()}
                    >
                        <span class="team-dash-flow-member">{member}</span>
                        <span class="team-dash-flow-copy">
                            <strong>{"lifecycle"}</strong>
                            <span>{boundary.cause().to_owned()}</span>
                        </span>
                        <Age authored={boundary.observed().get().as_millisecond()} class="team-dash-flow-age" />
                    </a>
                </li>
            }
        }
        FlowEvent::Direction {
            message,
            context,
            index,
        } => {
            let text = usize::try_from(context.start)
                .ok()
                .zip(usize::try_from(context.end).ok())
                .and_then(|(start, end)| message.text.get(start..end))
                .map(watchband::one_line)
                .map(|line| watchband::truncate(&line, DIRECTION_LABEL_BUDGET))
                .unwrap_or_else(|| "Direction delivered".to_owned());
            let source = agent_message_source_id(message);
            let origin = format!("team-dash-flow-{source}-direction-{index}");
            html! {
                <li
                    key={origin.clone()}
                    class="team-dash-flow-row"
                    data-flow-kind="direction"
                    data-agent-lane={lane.to_string()}
                    data-accent={accent.to_string()}
                >
                    <a
                        id={origin}
                        data-dash-return=""
                        data-dash-return-origin=""
                        href={crate::view::card_href(&source)}
                        onclick={show_chat.clone()}
                    >
                        <span class="team-dash-flow-member">{member}</span>
                        <span class="team-dash-flow-copy">
                            <strong>{"direction"}</strong>
                            <span>{text}</span>
                        </span>
                        <Age authored={context.authored.get().as_millisecond()} class="team-dash-flow-age" />
                    </a>
                </li>
            }
        }
    }
}

fn task_event_label(event: &wire::intent::TaskTimelineEvent) -> &'static str {
    use wire::intent::TaskTimelineEvent;
    match event {
        TaskTimelineEvent::Added {} => "added",
        TaskTimelineEvent::Edited {} => "edited",
        TaskTimelineEvent::Phased { .. } => "changed phase",
        TaskTimelineEvent::Completed {} => "completed",
        TaskTimelineEvent::Assigned { .. } => "assigned review",
        TaskTimelineEvent::Reviewed { .. } => "reviewed",
        TaskTimelineEvent::Deferred {} => "deferred",
        TaskTimelineEvent::Woken {} => "woke",
        TaskTimelineEvent::Deleted {} => "deleted",
        TaskTimelineEvent::Claimed {} => "claimed",
        TaskTimelineEvent::Carried {} => "received handoff",
        TaskTimelineEvent::Rebound {} => "recovered claim",
        TaskTimelineEvent::Released {} => "released",
        TaskTimelineEvent::Lapsed {} => "lease lapsed",
    }
}

struct ScrollAnchor {
    observer: web_sys::ResizeObserver,
    _aligned: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
    scroll: Closure<dyn FnMut(web_sys::Event)>,
    plane: web_sys::HtmlElement,
}

impl ScrollAnchor {
    fn attach(plane: web_sys::HtmlElement) -> Option<Self> {
        let list = plane.query_selector(".team-dash-flow-list").ok()??;
        let anchor = Rc::new(RefCell::new(None::<(web_sys::Element, f64)>));
        let capture = {
            let plane = plane.clone();
            let list = list.clone();
            let anchor = Rc::clone(&anchor);
            move || capture_anchor(&plane, &list, &anchor)
        };
        capture();
        let scroll = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| capture());
        plane
            .add_event_listener_with_callback("scroll", scroll.as_ref().unchecked_ref())
            .ok()?;

        let observed_plane = plane.clone();
        let observed_list = list.clone();
        let observed_anchor = Rc::clone(&anchor);
        let aligned =
            Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(move |_, _| {
                if observed_plane.scroll_top() <= 0 {
                    observed_anchor.borrow_mut().take();
                    return;
                }
                if let Some((element, offset)) = observed_anchor.borrow().as_ref()
                    && element.is_connected()
                {
                    let now = element.get_bounding_client_rect().top()
                        - observed_plane.get_bounding_client_rect().top();
                    observed_plane.scroll_by_with_x_and_y(0.0, now - offset);
                }
                capture_anchor(&observed_plane, &observed_list, &observed_anchor);
            });
        let observer = web_sys::ResizeObserver::new(aligned.as_ref().unchecked_ref()).ok()?;
        observer.observe(&list);
        Some(Self {
            observer,
            _aligned: aligned,
            scroll,
            plane,
        })
    }
}

impl Drop for ScrollAnchor {
    fn drop(&mut self) {
        self.observer.disconnect();
        let _ = self
            .plane
            .remove_event_listener_with_callback("scroll", self.scroll.as_ref().unchecked_ref());
    }
}

fn capture_anchor(
    plane: &web_sys::HtmlElement,
    list: &web_sys::Element,
    anchor: &Rc<RefCell<Option<(web_sys::Element, f64)>>>,
) {
    if plane.scroll_top() <= 0 {
        anchor.borrow_mut().take();
        return;
    }
    let plane_top = plane.get_bounding_client_rect().top();
    let children = list.children();
    let visible = (0..children.length()).find_map(|index| {
        let element = children.item(index)?;
        (element.get_bounding_client_rect().bottom() > plane_top).then(|| {
            let offset = element.get_bounding_client_rect().top() - plane_top;
            (element, offset)
        })
    });
    *anchor.borrow_mut() = visible;
}

#[hook]
fn use_scroll_anchor(host: NodeRef) {
    use_effect_with(host, move |host| {
        let anchor = host
            .cast::<web_sys::HtmlElement>()
            .and_then(ScrollAnchor::attach);
        move || drop(anchor)
    });
}
