//! The app's one content-agnostic right-edge sidebar surface.

use super::*;

pub(super) struct Content {
    pub label: AttrValue,
    pub title: AttrValue,
    pub body: Html,
    pub overlay: Html,
    pub overlay_open: bool,
    pub dismiss_overlay: Option<Callback<()>>,
}

pub(super) struct SidebarRender<'a> {
    pub content: Content,
    pub presented: bool,
    pub closing: bool,
    pub close_ref: &'a NodeRef,
    pub sheet_ref: &'a NodeRef,
    pub close: Callback<()>,
}

pub(super) fn render(render: SidebarRender<'_>) -> Html {
    let close = render
        .content
        .dismiss_overlay
        .clone()
        .unwrap_or(render.close);
    let close_click = close.reform(|_: MouseEvent| ());
    let state = if render.closing {
        "closing"
    } else if render.presented {
        "open"
    } else {
        "opening"
    };
    let close_label = format!("Close {}", render.content.label.to_lowercase());
    html! {
        <div
            class="app-sidebar-layer"
            data-sidebar-state={state}
            inert={render.closing.then_some("true")}
        >
            <button
                type="button"
                class="app-sidebar-backdrop app-overlay-veil"
                aria-label={close_label.clone()}
                onclick={close_click.clone()}
            ></button>
            <aside
                ref={render.sheet_ref.clone()}
                class="app-sidebar app-overlay-sheet"
                role="dialog"
                aria-modal="true"
                aria-labelledby="app-sidebar-title"
            >
                <div
                    class="app-sidebar-frame"
                    inert={render.content.overlay_open.then_some("true")}
                    aria-hidden={render.content.overlay_open.to_string()}
                >
                    <header class="app-sidebar-header">
                        <div>
                            <p class="app-sidebar-kicker">{render.content.label}</p>
                            <h2 id="app-sidebar-title">{render.content.title}</h2>
                        </div>
                        <button
                            ref={render.close_ref.clone()}
                            type="button"
                            class="app-sidebar-close app-compact-control"
                            aria-label={close_label}
                            onclick={close_click}
                        >{"Close"}</button>
                    </header>
                    {render.content.body}
                </div>
                <div class="app-sidebar-overlay-slot">{render.content.overlay}</div>
            </aside>
        </div>
    }
}
