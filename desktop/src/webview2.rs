//! The Windows window: one `tao` event loop hosting one `wry` WebView2
//! webview.
//!
//! WebView2 answers JavaScript dialogs and file pickers itself, so the shell
//! adds only the new-window policy: every request leaves through `external`
//! and the window stays the only webview. A missing WebView2 runtime surfaces
//! as the webview build's error.

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{NewWindowResponse, WebViewBuilder};

use crate::{INITIAL_SIZE, WindowError};

pub(crate) fn show(
    title: &str,
    url: &str,
    external: impl Fn(&str) + Send + Sync + 'static,
    opened: impl FnOnce(),
) -> Result<(), WindowError> {
    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(INITIAL_SIZE)
        .build(&event_loop)
        .map_err(|error| WindowError::Platform(error.to_string()))?;
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_new_window_req_handler(move |link, _| {
            external(&link);
            NewWindowResponse::Deny
        })
        .build(&window)
        .map_err(|error| WindowError::Platform(error.to_string()))?;
    opened();
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
    drop(webview);
    Ok(())
}
