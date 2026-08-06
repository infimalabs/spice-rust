//! The watchband: one tile per seated lane, and the compose bar that steers it.
//!
//! This is the operator's primary surface, not a decoration on the four read
//! axes. The axes answer questions about the journal; the watchband answers
//! "who is running, what did they just say, and how do I redirect them", which
//! is the only question that cannot be answered by reading.
//!
//! The structure is taken from the predecessor 0.31.0 UI rather than invented:
//! a single horizontal strip of fixed-height tiles above the workspace, each
//! tile carrying the lane's latest pending direction, its driver, and a hover
//! card reading name, lane, pending count, run state, claim and claim title.
//! One compose input serves the whole strip and sends to the focused tile,
//! because the predecessor puts Compose in the command bar rather than in each
//! tile.
//!
//! A tile is drawn from three joined snapshots — the lane roster, the steering
//! inbox, and the board — and every one of them can be absent independently.
//! The tile degrades a field at a time rather than disappearing, so a lane that
//! is seated but has no claim still shows as seated.

use super::*;

/// The steering control, carried as one value.
///
/// The focused tile, the compose buffer, and the submit that joins them are
/// three fields of one control rather than three independent inputs, and
/// passing them separately is what lets a caller wire a compose box to the
/// wrong tile.
#[derive(Clone)]
pub(super) struct Steering {
    pub(super) focus: UseStateHandle<Option<CanonicalUuid>>,
    pub(super) compose: UseStateHandle<String>,
    pub(super) submit: Callback<SubmitEvent>,
}

/// One lane tile, already joined across the three snapshots that feed it.
struct Tile {
    lane: CanonicalUuid,
    worktree: String,
    life: String,
    vendor: String,
    occupant: Option<CanonicalUuid>,
    disposition: String,
    pending: usize,
    headline: Option<String>,
    body: Option<String>,
    claim: Option<(String, String)>,
}

impl Tile {
    /// Whether this tile can receive direction.
    ///
    /// The resident refuses `steer::lane-vacant` for an unoccupied lane, so the
    /// tile refuses first rather than sending a command that cannot land.
    const fn seated(&self) -> bool {
        self.occupant.is_some()
    }

    /// The hover card, matching the predecessor's four-line tooltip.
    fn hover(&self) -> String {
        let mut lines = vec![
            format!("{}, {}", self.worktree, self.life),
            format!("{} pending, {}", self.pending, self.disposition),
        ];
        match &self.claim {
            Some((handle, title)) => {
                lines.push(handle.clone());
                lines.push(title.clone());
            }
            None => lines.push("no claim".to_owned()),
        }
        lines.join("\n")
    }
}

/// Render the strip and its compose bar.
pub(super) fn render(surface: &UseStateHandle<SurfaceData>, steering: &Steering) -> Html {
    let tiles = tiles(surface);
    let focused = *steering.focus;
    html! {
        <section class="watchband" aria-label="Lanes">
            <div class="watchband-bar">
                {compose_bar(&tiles, focused, steering)}
            </div>
            <div class="watchband-strip" role="list">
                {if tiles.is_empty() {
                    html! {
                        <p class="watchband-empty">
                            {"No lanes are seated. Start an agent to give the strip a tile."}
                        </p>
                    }
                } else {
                    html! { for tiles.iter().map(|tile| {
                        render_tile(tile, focused, &steering.focus)
                    }) }
                }}
            </div>
        </section>
    }
}

/// The single compose input, aimed at the focused tile.
fn compose_bar(tiles: &[Tile], focused: Option<CanonicalUuid>, steering: &Steering) -> Html {
    let target = focused.and_then(|lane| tiles.iter().find(|tile| tile.lane == lane));
    let placeholder = match target {
        Some(tile) if tile.seated() => format!("Steer {}", tile.worktree),
        Some(tile) => format!("{} is vacant", tile.worktree),
        None => "Select a lane to steer".to_owned(),
    };
    let ready = target.is_some_and(Tile::seated);
    let oninput = {
        let compose = steering.compose.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(input) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                compose.set(input.value());
            }
        })
    };
    html! {
        <form class="compose" onsubmit={steering.submit.clone()}>
            <input
                class="compose-input"
                type="text"
                value={(*steering.compose).clone()}
                placeholder={placeholder}
                disabled={!ready}
                {oninput}
            />
            <button class="compose-send" type="submit" disabled={!ready}>{"Send"}</button>
        </form>
    }
}

fn render_tile(
    tile: &Tile,
    focused: Option<CanonicalUuid>,
    focus: &UseStateHandle<Option<CanonicalUuid>>,
) -> Html {
    let selected = focused == Some(tile.lane);
    let onclick = {
        let focus = focus.clone();
        let lane = tile.lane;
        Callback::from(move |_| {
            focus.set((*focus != Some(lane)).then_some(lane));
        })
    };
    let class = if selected {
        "lane-tile lane-tile--focused"
    } else {
        "lane-tile"
    };
    html! {
        <button
            class={class}
            type="button"
            role="listitem"
            title={tile.hover()}
            aria-pressed={selected.to_string()}
            {onclick}
        >
            <span class="lane-tile-top">
                <span class="lane-tile-name">{tile.worktree.clone()}</span>
                <span class="lane-tile-head">{
                    tile.headline.clone().unwrap_or_else(|| tile.disposition.clone())
                }</span>
            </span>
            <span class="lane-tile-body">{
                tile.body.clone().unwrap_or_else(|| match &tile.claim {
                    Some((_, title)) => title.clone(),
                    None => "No direction pending.".to_owned(),
                })
            }</span>
            <span class="lane-tile-foot">
                <span class="lane-tile-claim">{
                    tile.claim
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), |(handle, _)| handle.clone())
                }</span>
                {if tile.pending > 0 {
                    html! { <span class="lane-tile-pending">{tile.pending}</span> }
                } else {
                    Html::default()
                }}
                <span class="lane-tile-vendor">{tile.vendor.clone()}</span>
            </span>
        </button>
    }
}

/// Join the lane roster, the steering inbox and the board into tiles.
///
/// Lane order is the roster's, which is canonical, so a tile does not move
/// under the operator's cursor when its pending count changes.
fn tiles(surface: &UseStateHandle<SurfaceData>) -> Vec<Tile> {
    let Some(runtime) = (**surface).as_ref() else {
        return Vec::new();
    };
    let Some(lanes) = runtime.lanes() else {
        return Vec::new();
    };
    let inbox = runtime.inbox();
    let board = runtime.board();
    lanes
        .lanes
        .iter()
        .map(|lane| {
            let pending = inbox
                .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == lane.lane))
                .map(|row| row.pending.as_slice())
                .unwrap_or_default();
            let latest = pending.last();
            let claim = board.and_then(|board| {
                board.tasks.iter().find_map(|task| {
                    let claim = task.claim.as_ref()?;
                    (claim.body.lane == lane.lane
                        && claim.body.disposition == wire::ClaimDisposition::Active)
                        .then(|| {
                            (
                                task.statement.body.handle.to_string(),
                                task.presentation_title.clone(),
                            )
                        })
                })
            });
            Tile {
                lane: lane.lane,
                worktree: lane.worktree.clone(),
                life: lane.life.clone(),
                vendor: lane.vendor.to_string(),
                occupant: lane.occupant,
                disposition: lane.disposition.clone(),
                pending: pending.len(),
                headline: latest.map(|directive| directive.key.clone()),
                body: latest.map(|directive| directive.text.clone()),
                claim,
            }
        })
        .collect()
}

/// The residency the focused lane's directive must witness.
///
/// Returned together because the resident re-proves all three as one witness
/// and refuses a command whose expectation differs by any field.
pub(super) fn residency(
    surface: &UseStateHandle<SurfaceData>,
    lane: CanonicalUuid,
) -> Option<(CanonicalUuid, CanonicalUuid)> {
    let inbox = (**surface).as_ref()?.inbox()?;
    let row = inbox.lanes.iter().find(|row| row.lane == lane)?;
    Some((row.occupant?, inbox.epoch))
}
