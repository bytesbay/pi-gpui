//! Drives the app's own window for UI checks on macOS: debug builds only,
//! and only when `PI_GPUI_AUTOMATION` names a folder.
//!
//! Keys and clicks are dispatched inside the app and the screenshot is of the
//! app's own window, so the checks need neither the Screen Recording nor the
//! Accessibility permission, never touch the real pointer or keyboard, and
//! work while the window sits behind others. See docs/MAC_UI_CHECKS.md.
//!
//! Write commands, one per line, to `<folder>/command` (rename the file into
//! place). The app runs them in order, removes the file and writes
//! `<folder>/done` with one result line per command:
//!
//! ```text
//! key down space cmd-enter     keystrokes, in gpui's notation
//! type some text               typed character by character
//! click 640 400                press and release at window coordinates
//! move 640 400                 move the pointer (hover)
//! wait 500                     milliseconds
//! shot /tmp/window.png         PNG of the window, 1 px per point (`2x` after
//!                              the path for full resolution)
//! ```

// `objc` 0.2 macros probe a `cargo-clippy` feature that modern rustc flags.
#![allow(unexpected_cfgs)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AsyncApp, Keystroke, Modifiers, Window};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const POLL: Duration = Duration::from_millis(100);
/// Time for the app to react to an event before the next one.
const SETTLE: Duration = Duration::from_millis(40);

/// The folder commands are read from, when automation is on.
pub fn folder() -> Option<PathBuf> {
    std::env::var_os("PI_GPUI_AUTOMATION")
        .filter(|folder| !folder.is_empty())
        .map(PathBuf::from)
}

pub fn start(window: AnyWindowHandle, cx: &mut App) {
    let Some(folder) = folder() else {
        return;
    };
    eprintln!("pi-gpui: automation commands from {}", folder.display());
    cx.spawn(async move |cx| {
        let (command, done) = (folder.join("command"), folder.join("done"));
        loop {
            cx.background_executor().timer(POLL).await;
            let Ok(script) = std::fs::read_to_string(&command) else {
                continue;
            };
            std::fs::remove_file(&command).ok();
            let mut report = String::new();
            for line in script
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                let outcome = match run(line, window, cx).await {
                    Ok(()) => "ok".to_string(),
                    Err(error) => format!("error: {error}"),
                };
                report.push_str(&format!("{line}: {outcome}\n"));
            }
            let staged = folder.join("done.tmp");
            if std::fs::write(&staged, report).is_ok() {
                std::fs::rename(&staged, &done).ok();
            }
        }
    })
    .detach();
}

async fn run(line: &str, window: AnyWindowHandle, cx: &mut AsyncApp) -> Result<(), String> {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match name {
        "key" => {
            for keys in rest.split_whitespace() {
                let keystroke = Keystroke::parse(keys).map_err(|error| error.to_string())?;
                dispatch_key(window, keystroke, cx).await?;
            }
        }
        "type" => {
            for character in rest.chars() {
                dispatch_key(window, typed(character), cx).await?;
            }
        }
        "click" | "move" => {
            let (x, y) = parse_point(rest)?;
            let target =
                update(window, cx, |window, _| native_window(window))?.ok_or("no native window")?;
            let events: &[MouseEvent] = if name == "click" {
                &[MouseEvent::Move, MouseEvent::Down, MouseEvent::Up]
            } else {
                &[MouseEvent::Move]
            };
            for event in events {
                // SAFETY: as in `settle`.
                unsafe { send_mouse(target, *event, x, y) };
                settle(window, cx).await?;
            }
        }
        "wait" => {
            let milliseconds: u64 = rest.parse().map_err(|_| "wait needs milliseconds")?;
            cx.background_executor()
                .timer(Duration::from_millis(milliseconds))
                .await;
        }
        "shot" => {
            let (path, scale) = rest.split_once(' ').unwrap_or((rest, ""));
            if path.is_empty() {
                return Err("shot needs a path".into());
            }
            let target = settle(window, cx).await?;
            // The frame reaches the window server when the run loop turns.
            cx.background_executor().timer(SETTLE).await;
            // SAFETY: as in `settle`.
            unsafe { capture(target, Path::new(path), scale.trim() == "2x")? };
        }
        _ => return Err("unknown command".into()),
    }
    Ok(())
}

fn update<R>(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    run: impl FnOnce(&mut Window, &mut App) -> R,
) -> Result<R, String> {
    window
        .update(cx, |_, window, cx| run(window, cx))
        .map_err(|error| error.to_string())
}

async fn dispatch_key(
    window: AnyWindowHandle,
    keystroke: Keystroke,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    update(window, cx, |window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    })?;
    settle(window, cx).await?;
    Ok(())
}

/// Lets the app react to an event, then draws a frame: the next event is
/// hit-tested and routed against what the last frame showed.
async fn settle(window: AnyWindowHandle, cx: &mut AsyncApp) -> Result<NativeWindow, String> {
    cx.background_executor().timer(SETTLE).await;
    let target =
        update(window, cx, |window, _| native_window(window))?.ok_or("no native window")?;
    // SAFETY: on the main thread, outside any app update (drawing re-enters
    // the app), with a live view from this window.
    unsafe { draw(target) };
    Ok(target)
}

/// The keystroke that types `character`.
fn typed(character: char) -> Keystroke {
    let key = match character {
        ' ' => "space".to_string(),
        '\n' => "enter".to_string(),
        _ => character.to_lowercase().to_string(),
    };
    Keystroke {
        modifiers: Modifiers {
            shift: character.is_uppercase(),
            ..Modifiers::default()
        },
        key,
        key_char: (character != '\n').then(|| character.to_string()),
    }
}

fn parse_point(text: &str) -> Result<(f64, f64), String> {
    let mut numbers = text.split_whitespace().map(str::parse::<f64>);
    match (numbers.next(), numbers.next()) {
        (Some(Ok(x)), Some(Ok(y))) => Ok((x, y)),
        _ => Err("expected `x y`".into()),
    }
}

/// gpui's NSView of the window, the window's number and its height in points.
#[derive(Clone, Copy)]
struct NativeWindow {
    view: *mut Object,
    number: u32,
    height: f64,
}

#[derive(Clone, Copy)]
enum MouseEvent {
    Move,
    Down,
    Up,
}

#[repr(C)]
struct NSPoint {
    x: f64,
    y: f64,
}

/// Hands gpui's view a mouse event at (`x`, `y`) from the window's top left
/// corner, as AppKit would. The event never passes through the system, so the
/// real pointer stays where it is.
unsafe fn send_mouse(target: NativeWindow, event: MouseEvent, x: f64, y: f64) {
    // NSEventTypeMouseMoved, NSEventTypeLeftMouseDown, NSEventTypeLeftMouseUp.
    let (kind, clicks): (usize, isize) = match event {
        MouseEvent::Move => (5, 0),
        MouseEvent::Down => (1, 1),
        MouseEvent::Up => (2, 1),
    };
    // AppKit counts from the bottom left corner.
    let location = NSPoint {
        x,
        y: target.height - y,
    };
    unsafe {
        let process: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let timestamp: f64 = msg_send![process, systemUptime];
        let nil: *mut Object = std::ptr::null_mut();
        let native: *mut Object = msg_send![class!(NSEvent),
            mouseEventWithType: kind
            location: location
            modifierFlags: 0usize
            timestamp: timestamp
            windowNumber: target.number as isize
            context: nil
            eventNumber: 0isize
            clickCount: clicks
            pressure: 1.0f32];
        if native.is_null() {
            return;
        }
        let _: () = match event {
            MouseEvent::Move => msg_send![target.view, mouseMoved: native],
            MouseEvent::Down => msg_send![target.view, mouseDown: native],
            MouseEvent::Up => msg_send![target.view, mouseUp: native],
        };
    }
}

fn native_window(window: &Window) -> Option<NativeWindow> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let view = appkit.ns_view.as_ptr() as *mut Object;
    // SAFETY: `view` is gpui's live NSView for this window, on the main thread.
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let number: isize = msg_send![ns_window, windowNumber];
        Some(NativeWindow {
            view,
            number: u32::try_from(number).ok()?,
            height: f64::from(f32::from(window.viewport_size().height)),
        })
    }
}

#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCreateImage(
        bounds: CGRect,
        list_option: u32,
        window: u32,
        image_option: u32,
    ) -> *mut c_void;
    fn CGImageRelease(image: *mut c_void);
}

const WINDOW_LIST_INCLUDING_WINDOW: u32 = 1 << 3;
const IMAGE_BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
const IMAGE_NOMINAL_RESOLUTION: u32 = 1 << 4;
const BITMAP_FILE_TYPE_PNG: usize = 4;

/// Draws a frame even if the window is hidden behind others (gpui stops its
/// display link then).
unsafe fn draw(target: NativeWindow) {
    unsafe {
        let layer: *mut Object = msg_send![target.view, layer];
        let _: () = msg_send![target.view, displayLayer: layer];
    }
}

/// Writes the window's picture to `path`.
unsafe fn capture(target: NativeWindow, path: &Path, full_resolution: bool) -> Result<(), String> {
    unsafe {
        // CGRectNull: the bounds of the window itself.
        let bounds = CGRect {
            x: f64::INFINITY,
            y: f64::INFINITY,
            width: 0.,
            height: 0.,
        };
        let mut image_option = IMAGE_BOUNDS_IGNORE_FRAMING;
        if !full_resolution {
            image_option |= IMAGE_NOMINAL_RESOLUTION;
        }
        let image = CGWindowListCreateImage(
            bounds,
            WINDOW_LIST_INCLUDING_WINDOW,
            target.number,
            image_option,
        );
        if image.is_null() {
            return Err("the window could not be captured".into());
        }
        let bitmap: *mut Object = msg_send![class!(NSBitmapImageRep), alloc];
        let bitmap: *mut Object = msg_send![bitmap, initWithCGImage: image];
        CGImageRelease(image);
        let properties: *mut Object = msg_send![class!(NSDictionary), dictionary];
        let data: *mut Object =
            msg_send![bitmap, representationUsingType: BITMAP_FILE_TYPE_PNG properties: properties];
        let written = if data.is_null() {
            Err("the picture could not be encoded".to_string())
        } else {
            let bytes: *const u8 = msg_send![data, bytes];
            let length: usize = msg_send![data, length];
            std::fs::write(path, std::slice::from_raw_parts(bytes, length))
                .map_err(|error| error.to_string())
        };
        let _: () = msg_send![bitmap, release];
        written
    }
}
