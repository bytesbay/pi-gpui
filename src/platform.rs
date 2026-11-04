//! Window behavior gpui 0.2.2 does not provide on macOS.

// `objc` 0.2 macros probe a `cargo-clippy` feature that modern rustc flags.
#![allow(unexpected_cfgs)]

use gpui::Window;

/// Starts dragging the window from our custom title bar.
///
/// Call it on the first mouse move after a mouse down in the title bar, like
/// Zed does. Newer gpui implements this in `Window::start_window_move` with
/// `-[NSWindow performWindowDragWithEvent:]`; in 0.2.2 that method is a no-op
/// on macOS, so we send the same message ourselves.
#[cfg(target_os = "macos")]
pub fn start_window_drag(window: &Window) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // `Window` also has an inherent `window_handle()`; ask the raw-window-handle trait.
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let view = appkit.ns_view.as_ptr() as *mut Object;

    // SAFETY: `view` is gpui's live NSView for this window (we are inside one of
    // its event handlers, on the main thread). All messages are standard AppKit
    // API, and null results are checked before use.
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let event: *mut Object = msg_send![app, currentEvent];
        if event.is_null() {
            return;
        }
        let _: () = msg_send![ns_window, performWindowDragWithEvent: event];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_window_drag(window: &Window) {
    window.start_window_move();
}

/// Tells macOS whether the app looks dark or light, so system-drawn parts
/// (traffic lights, menus, scroll bars, permission sheets) and other apps
/// asking for our appearance match the theme instead of the system setting.
#[cfg(target_os = "macos")]
pub fn set_app_appearance(dark: bool) {
    use std::ffi::CString;

    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let name = if dark {
        "NSAppearanceNameDarkAqua"
    } else {
        "NSAppearanceNameAqua"
    };
    let Ok(name) = CString::new(name) else {
        return;
    };
    // SAFETY: called on the main thread after AppKit finished launching; all
    // messages are standard AppKit API and null results are checked.
    unsafe {
        let name: *mut Object = msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
        let appearance: *mut Object = msg_send![class!(NSAppearance), appearanceNamed: name];
        if appearance.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setAppearance: appearance];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_app_appearance(_dark: bool) {}

/// Moves a file to the Trash, so a deleted session can still be restored.
#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &std::path::Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    use objc::runtime::{NO, Object};
    use objc::{class, msg_send, sel, sel_impl};

    let path_c = CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // SAFETY: standard Foundation calls on the main thread; the out-pointers
    // are valid for the duration of the call and results are checked.
    unsafe {
        let string: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: path_c.as_ptr()];
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: string];
        let manager: *mut Object = msg_send![class!(NSFileManager), defaultManager];
        let mut error: *mut Object = std::ptr::null_mut();
        let ok: objc::runtime::BOOL = msg_send![
            manager,
            trashItemAtURL: url
            resultingItemURL: std::ptr::null_mut::<*mut Object>()
            error: &mut error
        ];
        if ok != NO {
            return Ok(());
        }
        if error.is_null() {
            return Err("the file could not be moved to the Trash".to_string());
        }
        let description: *mut Object = msg_send![error, localizedDescription];
        let utf8: *const std::os::raw::c_char = msg_send![description, UTF8String];
        Err(std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn move_to_trash(path: &std::path::Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|error| error.to_string())
}
