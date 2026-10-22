//! The macOS window: one `tao` event loop hosting one `wry` webview.
//!
//! `wry`'s own `WKUIDelegate` answers file-upload panels and new-window
//! requests but leaves JavaScript `alert` and `confirm` unanswered, which
//! WebKit then settles as dismissed: every `confirm` returns `false`. The
//! console gates task settlement and draft discard on `confirm`, so the shell
//! installs its own delegate that puts those dialogs on screen as native
//! alerts, keeps the file panel, and sends new-window requests out to the
//! system browser.

use std::ptr::null_mut;

use block2::Block;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSModalResponseOK, NSOpenPanel};
use objc2_foundation::{MainThreadMarker, NSArray, NSObjectProtocol, NSString, NSURL};
use objc2_web_kit::{
    WKFrameInfo, WKNavigationAction, WKOpenPanelParameters, WKUIDelegate, WKWebView,
    WKWebViewConfiguration, WKWindowFeatures,
};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{WebViewBuilder, WebViewExtMacOS};

use crate::WindowError;

/// The window's opening size; the operator resizes from there.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 860.0);

pub(crate) fn show(
    title: &str,
    url: &str,
    external: impl Fn(&str) + Send + Sync + 'static,
) -> Result<(), WindowError> {
    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(INITIAL_SIZE)
        .build(&event_loop)
        .map_err(|error| WindowError::Platform(error.to_string()))?;
    let webview = WebViewBuilder::new()
        .with_url(url)
        .build(&window)
        .map_err(|error| WindowError::Platform(error.to_string()))?;
    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| WindowError::Platform("the window must open on the main thread".into()))?;
    // WebKit holds its UI delegate weakly; this binding keeps ours alive for
    // the window's whole life.
    let delegate = ShellDelegate::new(main_thread, Box::new(external));
    // SAFETY: the delegate outlives the webview (dropped after the event loop
    // returns), and this runs on the main thread the webview was built on.
    unsafe {
        webview
            .webview()
            .setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    }
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
    drop(delegate);
    Ok(())
}

struct ShellDelegateIvars {
    external: Box<dyn Fn(&str) + Send + Sync>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SpiceShellUIDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ShellDelegateIvars]
    struct ShellDelegate;

    unsafe impl NSObjectProtocol for ShellDelegate {}

    unsafe impl WKUIDelegate for ShellDelegate {
        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &Block<dyn Fn()>,
        ) {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.runModal();
            handler.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(
            &self,
            _webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &Block<dyn Fn(Bool)>,
        ) {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let confirmed = alert.runModal() == NSAlertFirstButtonReturn;
            handler.call((Bool::new(confirmed),));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn open_panel(
            &self,
            _webview: &WKWebView,
            parameters: &WKOpenPanelParameters,
            _frame: &WKFrameInfo,
            handler: &Block<dyn Fn(*const NSArray<NSURL>)>,
        ) {
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setCanChooseFiles(true);
            // SAFETY: plain property reads on a live panel-parameters object
            // WebKit handed to this call.
            unsafe {
                panel.setAllowsMultipleSelection(parameters.allowsMultipleSelection());
                panel.setCanChooseDirectories(parameters.allowsDirectories());
            }
            if panel.runModal() == NSModalResponseOK {
                handler.call((Retained::as_ptr(&panel.URLs()),));
            } else {
                handler.call((null_mut(),));
            }
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn new_window(
            &self,
            _webview: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: the navigation action is live for this call.
            let url = unsafe { action.request() }
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string());
            if let Some(url) = url {
                (self.ivars().external)(&url);
            }
            None
        }
    }
);

impl ShellDelegate {
    fn new(
        main_thread: MainThreadMarker,
        external: Box<dyn Fn(&str) + Send + Sync>,
    ) -> Retained<Self> {
        let delegate = main_thread
            .alloc::<Self>()
            .set_ivars(ShellDelegateIvars { external });
        // SAFETY: plain NSObject init on a freshly allocated instance whose
        // ivars are set.
        unsafe { msg_send![super(delegate), init] }
    }
}
