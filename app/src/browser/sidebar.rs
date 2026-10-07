//! Content-agnostic lifecycle, focus, and dismissal for the app sidebar.

use super::*;
use crate::work::TaskOverviewSelection;

/// The one content mounted in the app sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Content {
    Tasks(TaskOverviewSelection),
}

impl Content {
    pub(super) const fn tasks(&self) -> Option<&TaskOverviewSelection> {
        match self {
            Self::Tasks(selection) => Some(selection),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub content: Option<Content>,
    /// The destination owns focus, so ending this request skips the close slide
    /// and focus return.
    pub leaving: bool,
}

pub(super) struct Controls {
    pub content: UseStateHandle<Option<Content>>,
    pub presented: UseStateHandle<bool>,
    pub closing: UseStateHandle<bool>,
    pub close_ref: NodeRef,
    pub sheet_ref: NodeRef,
    /// Override the captured launcher with a stable surviving element.
    pub return_focus_to: Callback<web_sys::Element>,
    pub close: Callback<()>,
}

#[hook]
pub(super) fn use_controls(mounted: bool, requested: Request, close: Callback<()>) -> Controls {
    let content = use_state(|| None::<Content>);
    let return_focus = use_mut_ref(|| None::<web_sys::Element>);
    let presented = use_state_eq(|| false);
    let closing = use_state_eq(|| false);
    let close_ref = use_node_ref();
    let sheet_ref = use_node_ref();
    let open = open_callback(
        content.clone(),
        presented.clone(),
        closing.clone(),
        return_focus.clone(),
    );
    let request_close = {
        let closing = closing.clone();
        Callback::from(move |()| {
            closing.set(true);
            close.emit(());
        })
    };
    let complete_close =
        complete_close_callback(content.clone(), presented.clone(), closing.clone());
    let leave = leave_callback(
        content.clone(),
        presented.clone(),
        closing.clone(),
        return_focus.clone(),
    );
    use_request(requested, open.clone(), leave, &content, closing.clone());
    use_present(
        content.is_some() && mounted && !*presented,
        presented.clone(),
    );
    overlay::use_close_transition(
        *closing,
        *presented && mounted && !overlay::reduced_motion(),
        sheet_ref.clone(),
        complete_close,
    );
    use_focus(&content, mounted, close_ref.clone(), return_focus.clone());
    overlay::use_escape_owner(
        content.is_some() && !*closing && mounted,
        request_close.clone(),
    );
    let return_focus_to = {
        let return_focus = return_focus.clone();
        Callback::from(move |target| *return_focus.borrow_mut() = Some(target))
    };
    Controls {
        content,
        presented,
        closing,
        close_ref,
        sheet_ref,
        return_focus_to,
        close: request_close,
    }
}

#[hook]
fn use_request(
    requested: Request,
    open: Callback<Content>,
    leave: Callback<()>,
    content: &UseStateHandle<Option<Content>>,
    closing: UseStateHandle<bool>,
) {
    let shown = content.is_some();
    let prior = use_mut_ref(|| None::<Content>);
    let leaving = requested.leaving;
    use_effect_with(requested.content, move |requested| {
        let was_requested = prior.borrow().is_some();
        *prior.borrow_mut() = requested.clone();
        match requested {
            Some(content) => open.emit(content.clone()),
            None if shown && was_requested && leaving => leave.emit(()),
            None if shown && was_requested => closing.set(true),
            None => {}
        }
    });
}

/// Swap content in place when already open. Only opening from empty or
/// interrupting a close begins a presentation transition and captures focus.
fn open_callback(
    content: UseStateHandle<Option<Content>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<Content> {
    Callback::from(move |next: Content| {
        if content.as_ref() == Some(&next) && !*closing {
            return;
        }
        if content.is_none() {
            *return_focus.borrow_mut() = active_focus_target();
        }
        if content.is_none() || *closing {
            presented.set(false);
            closing.set(false);
        }
        content.set(Some(next));
    })
}

fn active_focus_target() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .active_element()
        .filter(|element| {
            element
                .dyn_ref::<web_sys::HtmlElement>()
                .is_some_and(|element| element.tab_index() >= 0)
        })
}

/// Drop the sidebar for a destination that owns focus, without a close slide.
fn leave_callback(
    content: UseStateHandle<Option<Content>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<()> {
    Callback::from(move |()| {
        return_focus.borrow_mut().take();
        presented.set(false);
        closing.set(false);
        content.set(None);
    })
}

fn complete_close_callback(
    content: UseStateHandle<Option<Content>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
) -> Callback<()> {
    Callback::from(move |()| {
        presented.set(false);
        closing.set(false);
        content.set(None);
    })
}

/// Present one frame after the mounted layer appears, producing a real opening
/// transition even when content was selected before the layer could render.
#[hook]
fn use_present(pending: bool, presented: UseStateHandle<bool>) {
    use_effect_with(pending, move |pending| {
        let frame = pending.then(|| {
            let window = web_sys::window().expect("app presentation has a browser window");
            let show = presented.clone();
            let callback = Closure::once(move |_: f64| show.set(true));
            let frame = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .expect("app window accepts a presentation frame");
            (window, frame, callback)
        });
        move || {
            if let Some((window, frame, callback)) = frame {
                let _ = window.cancel_animation_frame(frame);
                drop(callback);
            }
        }
    });
}

/// Focus enters the sidebar when it opens and returns to the surviving launcher
/// after close. Content swaps leave focus inside the already-presented sheet.
#[hook]
fn use_focus(
    content: &UseStateHandle<Option<Content>>,
    mounted: bool,
    close: NodeRef,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) {
    use_effect_with(content.is_some() && mounted, move |active| {
        if *active && let Some(button) = close.cast::<web_sys::HtmlElement>() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = button.focus_with_options(&options);
        }
    });
    use_effect_with((**content).clone(), move |content| {
        if content.is_some() {
            return;
        }
        let _ = return_focus
            .borrow_mut()
            .take()
            .filter(overlay::returnable)
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            .is_some_and(|element| element.focus().is_ok());
    });
}
