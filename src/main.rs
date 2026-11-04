mod chat;
mod highlight;
mod hub;
mod jsonl;
mod markdown;
mod pi;
mod platform;
mod plugins;
mod selectable;
mod sessions;
mod text_input;
mod ui;
mod updater;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, point, prelude::*, px, size,
};

use crate::hub::HubView;
use crate::ui::{Assets, Theme};

actions!(app, [Quit, CheckForUpdates]);

fn main() {
    migrate_data_directory();
    // Release tooling: the pi versions this build supports, as JSON.
    if std::env::args().nth(1).as_deref() == Some("--pi-compat") {
        println!("{}", pi::compatibility_json());
        return;
    }
    // Usage: pi-gpui [workspace]. The workspace is added to the sidebar and
    // a new session is opened in it.
    let initial_workspace =
        std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .map(|path| match path.canonicalize() {
                Ok(path) if path.is_dir() => path,
                Ok(path) => {
                    eprintln!("pi-gpui: {} is not a directory", path.display());
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("pi-gpui: {}: {error}", path.display());
                    std::process::exit(1);
                }
            });

    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            let system_is_dark = matches!(
                cx.window_appearance(),
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            );
            let theme = Theme::default_theme(system_is_dark);
            platform::set_app_appearance(theme.is_dark());
            cx.set_global(theme);
            text_input::bind_keys(cx);
            chat::bind_keys(cx);
            hub::bind_keys(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            updater::AutoUpdater::init(cx);
            highlight::preload();
            cx.on_action(|_: &CheckForUpdates, cx| {
                updater::AutoUpdater::global(cx).update(cx, |updater, cx| updater.check(true, cx));
            });
            cx.set_menus(vec![Menu {
                name: "Pi GPUI".into(),
                items: vec![
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Quit Pi GPUI", Quit),
                ],
            }]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1180.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Draw our own title bar; macOS keeps the traffic lights.
                titlebar: Some(TitlebarOptions {
                    title: Some("Pi GPUI".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(11.))),
                }),
                ..Default::default()
            };

            let window = match cx.open_window(options, |window, cx| {
                cx.new(|cx| HubView::new(initial_workspace.clone(), window, cx))
            }) {
                Ok(window) => window,
                Err(error) => {
                    eprintln!("pi-gpui: failed to open a window: {error:#}");
                    cx.quit();
                    return;
                }
            };

            let focused = window.update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
                cx.activate(true);
            });
            if let Err(error) = focused {
                eprintln!("pi-gpui: failed to focus the window: {error:#}");
            }
        });
}

/// The app was called "Pi GPU UI" before; keep its projects and settings.
fn migrate_data_directory() {
    let Some(home) = sessions::home_directory() else {
        return;
    };
    let support = home.join("Library/Application Support");
    let (old, new) = (support.join("pi-gpu-ui"), support.join("pi-gpui"));
    if old.is_dir()
        && !new.exists()
        && let Err(error) = std::fs::rename(&old, &new)
    {
        eprintln!("pi-gpui: could not move {}: {error}", old.display());
    }
}
