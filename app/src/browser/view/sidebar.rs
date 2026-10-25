//! The app's one content-agnostic right-edge sidebar surface.

use super::*;

/// Standard side-view widths. Content takes the one its layout needs.
#[derive(Clone, Copy)]
pub(super) enum Width {
    Small,
    Large,
}

impl Width {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Large => "large",
        }
    }
}

/// What stands beside the kicker in a sidebar's header.
pub(super) enum Heading {
    /// The sheet's title, which names the dialog.
    Title(AttrValue),
    /// Content the sidebar supplies in the title's place; the kicker names the
    /// dialog.
    Beside(Html),
}

pub(super) struct Content {
    pub label: AttrValue,
    pub heading: Heading,
    pub width: Width,
    /// The palette index the whole sheet, header and body, wears as its base;
    /// `None` keeps the page's.
    pub base_accent: Option<usize>,
    pub body: Html,
    pub overlay: Html,
    pub overlay_open: bool,
    pub dismiss_overlay: Option<Callback<()>>,
    /// The launcher's twin, standing over the launcher's own place and closing
    /// the sidebar in place of the Close button. It sits beside the fading
    /// surface rather than inside it, so the trigger never changes while the
    /// view appears. It carries the sidebar's close ref and close callback.
    pub anchor: Option<Html>,
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
    let anchored = render.content.anchor.is_some();
    html! {
        <div
            class="app-sidebar-layer"
            data-sidebar-state={state}
            data-sidebar-width={render.content.width.as_str()}
            data-sidebar-anchored={anchored.then_some("true")}
            inert={render.closing.then_some("true")}
        >
            <button
                type="button"
                class="app-sidebar-backdrop app-overlay-veil"
                aria-label={close_label.clone()}
                onclick={close_click.clone()}
            ></button>
            <aside
                class="app-sidebar"
                role="dialog"
                aria-modal="true"
                aria-labelledby="app-sidebar-title"
                data-base-flavor={render.content.base_accent.map(|_| "sheet")}
                data-base-accent={render.content.base_accent.map(|base| base.to_string())}
            >
                {render.content.anchor.unwrap_or_default()}
                <div ref={render.sheet_ref.clone()} class="app-sidebar-surface app-overlay-sheet">
                    <div
                        class="app-sidebar-frame"
                        inert={render.content.overlay_open.then_some("true")}
                        aria-hidden={render.content.overlay_open.to_string()}
                    >
                        <header class="app-sidebar-header">
                            <div>
                                {match render.content.heading {
                                    Heading::Title(title) => html! {
                                        <>
                                            <p class="app-sidebar-kicker">{render.content.label}</p>
                                            <h2 id="app-sidebar-title">{title}</h2>
                                        </>
                                    },
                                    Heading::Beside(beside) => html! {
                                        <>
                                            <p id="app-sidebar-title" class="app-sidebar-kicker">{render.content.label}</p>
                                            {beside}
                                        </>
                                    },
                                }}
                            </div>
                            if !anchored {
                                <button
                                    ref={render.close_ref.clone()}
                                    type="button"
                                    class="app-sidebar-close app-compact-control"
                                    aria-label={close_label}
                                    onclick={close_click}
                                >{"Close"}</button>
                            }
                        </header>
                        {render.content.body}
                    </div>
                    <div class="app-sidebar-overlay-slot">{render.content.overlay}</div>
                </div>
            </aside>
        </div>
    }
}
