//! macOS desktop integration: one borderless `NSWindow` per `NSScreen` at
//! `kCGDesktopWindowLevel`. Spec: `docs/01-SYSTEM-ARCHITECTURE.md` section 2.2 (pitfall #2).
//!
//! Compiled as `crate::mac_desktop` (macOS only). All AppKit calls require a
//! [`MainThreadMarker`].
//!
//! AppKit marks a desktop-level window as occluded as soon as a large window covers it, which makes
//! `WKWebView` suspend `requestAnimationFrame` and `CAMetalLayer` throttle. [`TruongDesktopWindow`]
//! overrides `occlusionState` to always report visible; Smart Pause (`policy.rs`) decides on its
//! own using `CGWindowListCopyWindowInfo`.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{class, define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSResponder, NSScreen, NSWindow, NSWindowCollectionBehavior,
    NSWindowOcclusionState, NSWindowStyleMask, NSWorkspace,
};
use objc2_core_graphics::{CGWindowLevelForKey, CGWindowLevelKey};
use objc2_foundation::{NSDictionary, NSNumber, NSRect, NSString, NSURL};

/// Publishes a still to the macOS system wallpaper on the primary display.
pub fn set_system_wallpaper(mtm: MainThreadMarker, path: &std::path::Path) -> Result<(), String> {
    let screen = NSScreen::screens(mtm)
        .firstObject()
        .ok_or("No primary display is available")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: the primary NSScreen is accessed on the main thread. Empty options use
    // the system's default scaling.
    unsafe {
        NSWorkspace::sharedWorkspace()
            .setDesktopImageURL_forScreen_options_error(&url, &screen, &NSDictionary::new())
            .map_err(|error| error.localizedDescription().to_string())
    }
}

define_class!(
    // SAFETY:
    // - NSWindow -> NSResponder -> NSObject is the exact inheritance chain.
    // - TruongDesktopWindow is MainThreadOnly and does not implement Drop.
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TruongDesktopWindow"]
    pub struct TruongDesktopWindow;

    impl TruongDesktopWindow {
        /// Always report Visible so macOS never throttles WKWebView requestAnimationFrame
        /// or CAMetalLayer display sync when normal windows cover the desktop.
        #[unsafe(method(occlusionState))]
        fn __occlusion_state(&self) -> NSWindowOcclusionState {
            NSWindowOcclusionState::Visible
        }

        #[unsafe(method(canBecomeKeyWindow))]
        fn __can_become_key_window(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn __can_become_main_window(&self) -> bool {
            false
        }
    }
);

/// Extracts the persistent CoreGraphics `CGDirectDisplayID` (`u32`) for an `NSScreen`.
pub fn screen_display_id(screen: &NSScreen) -> u32 {
    let desc = screen.deviceDescription();
    let key = NSString::from_str("NSScreenNumber");
    desc.objectForKey(&key)
        .and_then(|obj| obj.downcast::<NSNumber>().ok())
        .map(|num| num.unsignedIntValue())
        .unwrap_or(0)
}

/// Creates the desktop-level wallpaper window covering `screen`.
pub fn create_desktop_window_for_screen(
    mtm: MainThreadMarker,
    screen: &NSScreen,
) -> Retained<NSWindow> {
    let frame: NSRect = screen.frame();
    let style = NSWindowStyleMask::Borderless;

    // SAFETY: `initWithContentRect:styleMask:backing:defer:` is NSWindow's designated initializer
    // and TruongDesktopWindow adds no ivars.
    let window: Retained<TruongDesktopWindow> = unsafe {
        msg_send![
            mtm.alloc::<TruongDesktopWindow>(),
            initWithContentRect: frame,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };

    let ns_window: Retained<NSWindow> = Retained::into_super(window);
    // SAFETY: we keep our own `Retained` and never rely on AppKit releasing it on close.
    unsafe {
        ns_window.setReleasedWhenClosed(false);
    }

    configure_desktop_window(&ns_window);
    ns_window
}

/// Desktop-level settings shared by [`create_desktop_window_for_screen`] and
/// [`attach_window_to_desktop`].
fn configure_desktop_window(ns_window: &NSWindow) {
    // Sit directly above root wallpaper and below Finder desktop icons
    let desktop_level = CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey) as isize;
    ns_window.setLevel(desktop_level);

    // Persist across all Spaces, Stage Manager, and Mission Control
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );

    ns_window.setOpaque(true);
    ns_window.setHasShadow(false);
    // Let Finder desktop icons receive direct clicks; forward cursor via global monitor
    ns_window.setIgnoresMouseEvents(true);
    ns_window.setRestorable(false);
    ns_window.orderFrontRegardless();
}

/// Moves an existing window (e.g. a Tauri `WebviewWindow` hosting a Web / Video wallpaper) to the
/// desktop level with the same level, collection behavior, click-through and opacity settings as
/// [`create_desktop_window_for_screen`].
///
/// The `occlusionState` override of [`TruongDesktopWindow`] cannot be applied to windows created
/// by another class; Smart Pause never reads `occlusionState` either way (pitfall #2).
pub fn attach_window_to_desktop(_mtm: MainThreadMarker, ns_window: &NSWindow) {
    configure_desktop_window(ns_window);
}

/// Creates one desktop window per connected `NSScreen`, keyed by `CGDirectDisplayID`.
pub fn create_desktop_windows_for_all_screens(
    mtm: MainThreadMarker,
) -> Vec<(u32, Retained<NSWindow>)> {
    NSScreen::screens(mtm)
        .iter()
        .map(|screen| {
            (
                screen_display_id(&screen),
                create_desktop_window_for_screen(mtm, &screen),
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacScreenEvent {
    /// Fired on NSApplicationDidChangeScreenParametersNotification (monitor hotplug, resolution, HiDPI scale)
    ScreenParametersChanged,
    /// Fired on NSWorkspaceActiveSpaceDidChangeNotification (Space swipe / Full Screen transition)
    ActiveSpaceChanged,
}

/// Registers main-queue observers for display hotplug/resolution changes and macOS Spaces transitions.
///
/// Observers live for the rest of the process (tokens are intentionally retained/leaked).
/// `on_event` runs on the main queue.
pub fn start_screen_and_space_observer(
    _mtm: MainThreadMarker,
    on_event: impl Fn(MacScreenEvent) + Send + Sync + 'static,
) {
    let on_event = std::sync::Arc::new(on_event);

    // SAFETY: plain Foundation/AppKit class messages with correctly typed arguments; the
    // notification centers copy the blocks.
    unsafe {
        let queue: *mut AnyObject = msg_send![class!(NSOperationQueue), mainQueue];
        let nil: *mut AnyObject = std::ptr::null_mut();

        // 1. Display hotplug / resolution / arrangement change on NSNotificationCenter.defaultCenter
        let default_center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        let screen_name = NSString::from_str("NSApplicationDidChangeScreenParametersNotification");
        let cb_screen = std::sync::Arc::clone(&on_event);
        let screen_block = RcBlock::new(move |_note: *mut AnyObject| {
            cb_screen(MacScreenEvent::ScreenParametersChanged);
        });
        let token1: *mut AnyObject = msg_send![
            default_center,
            addObserverForName: &*screen_name,
            object: nil,
            queue: queue,
            usingBlock: &*screen_block
        ];
        let _: *mut AnyObject = msg_send![token1, retain];

        // 2. Active Space change on NSWorkspace.sharedWorkspace.notificationCenter
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let ws_center: *mut AnyObject = msg_send![workspace, notificationCenter];
        let space_name = NSString::from_str("NSWorkspaceActiveSpaceDidChangeNotification");
        let cb_space = std::sync::Arc::clone(&on_event);
        let space_block = RcBlock::new(move |_note: *mut AnyObject| {
            cb_space(MacScreenEvent::ActiveSpaceChanged);
        });
        let token2: *mut AnyObject = msg_send![
            ws_center,
            addObserverForName: &*space_name,
            object: nil,
            queue: queue,
            usingBlock: &*space_block
        ];
        let _: *mut AnyObject = msg_send![token2, retain];
    }
}
