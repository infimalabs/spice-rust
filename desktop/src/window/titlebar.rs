//! The console's header is the window's title bar.
//!
//! The window paints the console under a transparent, titleless title bar, and
//! its traffic lights stand centred in the header's leading edge. The shell
//! publishes the width they occupy as `--app-titlebar-inset` on the document
//! root; the header reserves that much leading space, so the browser, which
//! never sets it, renders exactly as before. Full screen hides the traffic
//! lights, so the shell withdraws the inset there.
//!
//! AppKit lays the lights out again whenever the window resizes, and tao's
//! own inset only reapplies while its content view draws, which the webview
//! covering it never prompts; the shell places them itself after each resize.
//!
//! WKWebView takes every click over the page, so the window's own title-bar
//! drag never sees one. The page's initialization script reports a primary
//! mouse-down on the header's empty area, anything outside its controls, over
//! the IPC channel; the shell starts a native window drag for it, or performs
//! the system's title-bar double-click action for the second press.

use objc2_app_kit::{NSWindow, NSWindowButton};
use objc2_foundation::{NSPoint, NSString, NSUserDefaults};
use tao::window::Window;
use wry::WebView;

/// The close light's leading edge, from the window's leading edge.
const LEADING: f64 = 20.0;
/// The lights' vertical centre, from the window's top: the centre of the
/// console's 60px command bar (`app/chrome.css`).
const CENTRE: f64 = 30.0;

/// The page's side: report the header's empty-area presses and ask for the
/// inset once the document exists, including after every reload.
pub(super) const SCRIPT: &str = r#"(() => {
  const controls = "button, a, input, select, textarea, [tabindex], [contenteditable]";
  addEventListener("mousedown", (event) => {
    const target = event.target;
    if (event.button !== 0 || !(target instanceof Element)) return;
    if (!target.closest(".commandbar") || target.closest(controls)) return;
    event.preventDefault();
    window.ipc.postMessage(event.detail === 2 ? "double-click" : "drag");
  });
  window.ipc.postMessage("ready");
})();"#;

/// What the page asks of its window.
pub(super) enum Request {
    /// A document started; publish the current inset to it.
    Ready,
    /// The header's empty area was pressed: move the window.
    Drag,
    /// The header's empty area was pressed twice.
    DoubleClick,
}

impl Request {
    /// The request one IPC message names, if any.
    pub(super) fn parse(message: &str) -> Option<Self> {
        match message {
            "ready" => Some(Self::Ready),
            "drag" => Some(Self::Drag),
            "double-click" => Some(Self::DoubleClick),
            _ => None,
        }
    }
}

/// The inset last published to the page.
#[derive(Default)]
pub(super) struct Titlebar {
    published: Option<bool>,
}

impl Titlebar {
    /// Answer one page request.
    pub(super) fn answer(
        &mut self,
        request: Request,
        window: &Window,
        ns_window: &NSWindow,
        webview: &WebView,
    ) {
        match request {
            Request::Ready => {
                self.published = None;
                self.follow(window, ns_window, webview);
            }
            Request::Drag => {
                let _ = window.drag_window();
            }
            Request::DoubleClick => double_click(ns_window),
        }
    }

    /// Keep the traffic lights in the header, and publish the inset when full
    /// screen hid or restored them.
    pub(super) fn follow(&mut self, window: &Window, ns_window: &NSWindow, webview: &WebView) {
        let lights = window.fullscreen().is_none();
        let reserve = lights.then(|| place(ns_window)).flatten();
        if self.published == Some(lights) {
            return;
        }
        let script = match reserve {
            Some(width) => format!(
                "document.documentElement.style.setProperty('--app-titlebar-inset', '{width}px')"
            ),
            None => "document.documentElement.style.removeProperty('--app-titlebar-inset')".into(),
        };
        if webview.evaluate_script(&script).is_ok() {
            self.published = Some(lights);
        }
    }
}

/// Centre the traffic lights in the header's leading edge, and answer the
/// leading width they take: their inset on both sides of their span.
fn place(window: &NSWindow) -> Option<f64> {
    let [Some(close), Some(minimize), Some(zoom)] = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .map(|button| window.standardWindowButton(button)) else {
        return None;
    };
    // SAFETY: the lights live in the title-bar view inside the title-bar
    // container, both owned by the window for its whole life.
    let container = unsafe { close.superview()?.superview()? };
    // The container hangs from the window's top and lays the lights out from
    // its own bottom edge.
    let light = close.frame();
    let mut frame = container.frame();
    frame.size.height = CENTRE + light.origin.y + light.size.height / 2.0;
    frame.origin.y = window.frame().size.height - frame.size.height;
    container.setFrame(frame);
    let spacing = minimize.frame().origin.x - light.origin.x;
    for (index, button) in [&close, &minimize, &zoom].into_iter().enumerate() {
        let origin = NSPoint::new(LEADING + index as f64 * spacing, button.frame().origin.y);
        button.setFrameOrigin(origin);
    }
    Some((2.0 * (LEADING + spacing) + zoom.frame().size.width).ceil())
}

/// The user's choice for a title-bar double-click: minimize, nothing, or zoom,
/// which is also the system default.
fn double_click(window: &NSWindow) {
    let action = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|action| action.to_string());
    match action.as_deref() {
        Some("Minimize") => window.miniaturize(None),
        Some("None") => {}
        _ => window.zoom(None),
    }
}
