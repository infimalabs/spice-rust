//! A Design room's collapsible blocks as one strip of summaries over one
//! shared panel. Every summary keeps its place in the strip whatever is open,
//! and the one opened block reads in a full-width panel beneath it, so a
//! short block never stands beside a long one's empty height.

use std::sync::atomic::{AtomicU32, Ordering};

use yew::prelude::*;

/// The next mounted strip's panel ordinal. The same Design can show its strip
/// in more than one surface at once, its console menu and its read view, so
/// the room alone cannot name a panel uniquely in the document.
static NEXT_PANEL: AtomicU32 = AtomicU32::new(0);

/// One block in the strip: `summary` is its always-visible line and `body`
/// what the panel shows while it is open. `key` names the block in
/// `data-sheet-block`.
#[derive(Clone, PartialEq)]
pub(in crate::browser) struct RoomBlock {
    pub(in crate::browser) key: &'static str,
    pub(in crate::browser) label: &'static str,
    pub(in crate::browser) summary: Html,
    pub(in crate::browser) body: Html,
}

#[derive(Properties, PartialEq)]
pub(in crate::browser) struct RoomBlocksProps {
    /// The room these blocks describe; it prefixes the panel's id, which each
    /// mounted strip makes unique.
    pub(in crate::browser) room: AttrValue,
    pub(in crate::browser) blocks: Vec<RoomBlock>,
}

/// The strip of `blocks` with at most one open. Opening a block closes the
/// open one; activating the open block closes it. Callers key the component
/// by room so a different room starts closed. The panel's id is fixed for the
/// life of this mount and differs from every other mounted strip's.
#[function_component(RoomBlocks)]
pub(in crate::browser) fn room_blocks(props: &RoomBlocksProps) -> Html {
    let open = use_state(|| None::<&'static str>);
    let ordinal = *use_state(|| NEXT_PANEL.fetch_add(1, Ordering::Relaxed));
    let panel_id = format!("room-block-panel-{}-{ordinal}", props.room);
    let opened = open.and_then(|key| props.blocks.iter().find(|block| block.key == key));
    html! {
        <div class="room-blocks">
            <div class="room-block-strip">
                {for props.blocks.iter().map(|block| {
                    let expanded = *open == Some(block.key);
                    let onclick = {
                        let open = open.clone();
                        let key = block.key;
                        Callback::from(move |_: MouseEvent| {
                            open.set((*open != Some(key)).then_some(key));
                        })
                    };
                    html! {
                        <button
                            key={block.key}
                            type="button"
                            class="room-block-tab"
                            data-sheet-block={block.key}
                            aria-expanded={expanded.to_string()}
                            aria-controls={expanded.then(|| panel_id.clone())}
                            {onclick}
                        >
                            {block.summary.clone()}
                        </button>
                    }
                })}
            </div>
            if let Some(block) = opened {
                <div
                    id={panel_id}
                    class="room-block-panel"
                    role="region"
                    aria-label={block.label}
                    data-sheet-block={block.key}
                >
                    {block.body.clone()}
                </div>
            }
        </div>
    }
}
