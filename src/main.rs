mod attachments;
mod chat;
mod diff;
mod dock;
mod file_tree;
mod highlight;
mod hub;
mod jsonl;
mod keys;
mod markdown;
mod open_url;
mod pi;
mod pi_updates;
mod platform;
mod plugins;
mod selectable;
mod sessions;
mod settings;
#[cfg(not(target_os = "macos"))]
mod single_instance;
mod text_input;
mod ui;
mod updater;
mod window_frame;

use std::path::PathBuf;

use futures::StreamExt;

use gpui::{
    App, Application, Bounds, Focusable, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType,
    TitlebarOptions, WindowBounds, WindowOptions, actions, point, prelude::*, px, size,
};

use crate::hub::HubView;
use crate::ui::Assets;

actions!(
    app,
    [Quit, CheckForUpdates, About, Hide, HideOthers, ShowAll]
);

fn main() {
    #[cfg(target_os = "macos")]
    migrate_data_directory();
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("pi-gpui {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // Release tooling: the pi versions this build supports, as JSON.
    if std::env::args().nth(1).as_deref() == Some("--pi-compat") {
        println!("{}", pi::compatibility_json());
        return;
    }
    // Usage: pi-gpui [workspace | pi-gpui://link]. A workspace is added to
    // the sidebar and a new session is opened in it. Linux link handlers pass
    // the link as the argument (resources/pi-gpui.desktop).
    let initial_link = std::env::args()
        .nth(1)
        .filter(|argument| argument.starts_with("pi-gpui:"));
    let initial_workspace = std::env::args_os()
        .nth(1)
        .filter(|_| initial_link.is_none())
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

    // pi-gpui:// links (Info.plist registers the scheme). macOS can deliver
    // them before the window exists, so they wait in a channel.
    let (url_sender, mut url_receiver) = futures::channel::mpsc::unbounded::<Vec<String>>();
    #[cfg(not(target_os = "macos"))]
    {
        let message = match (&initial_link, &initial_workspace) {
            (Some(link), _) => link.clone(),
            (None, Some(workspace)) => open_url::workspace_link(workspace),
            (None, None) => single_instance::ACTIVATE.to_string(),
        };
        if single_instance::forward(&message) {
            return;
        }
        let sender = url_sender.clone();
        single_instance::listen(move |message| {
            sender.unbounded_send(vec![message]).ok();
        });
    }
    if let Some(link) = &initial_link {
        url_sender.unbounded_send(vec![link.clone()]).ok();
    }
    let app = Application::new().with_assets(Assets);
    app.on_open_urls(move |urls| {
        url_sender.unbounded_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        settings::init(cx);
        text_input::bind_keys(cx);
        chat::bind_keys(cx);
        file_tree::bind_keys(cx);
        hub::bind_keys(cx);
        cx.bind_keys([KeyBinding::new(&crate::keys::binding("cmd-q"), Quit, None)]);
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
        ]);
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &About, cx| {
            cx.open_url("https://github.com/bytesbay/pi-gpui");
        });
        cx.on_action(|_: &Quit, cx| cx.quit());
        updater::AutoUpdater::init(cx);
        highlight::preload();
        cx.on_action(|_: &CheckForUpdates, cx| {
            updater::AutoUpdater::global(cx).update(cx, |updater, cx| updater.check(true, cx));
        });
        cx.set_menus(app_menus());
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
            // Linux: draw the frame ourselves where the compositor allows it
            // (window_frame.rs); app_id matches the .desktop file for the icon.
            window_decorations: (!cfg!(target_os = "macos"))
                .then_some(gpui::WindowDecorations::Client),
            app_id: Some("pi-gpui".into()),
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

        cx.spawn(async move |cx| {
            while let Some(urls) = url_receiver.next().await {
                let opened = window.update(cx, |view, window, cx| {
                    // "activate" (single_instance.rs) only brings the window forward.
                    for url in urls.iter().filter(|url| url.as_str() != "activate") {
                        view.open_url(url, cx);
                    }
                    window.activate_window();
                });
                if opened.is_err() {
                    break;
                }
                cx.update(|cx| cx.activate(true)).ok();
            }
        })
        .detach();
    });
}

/// The app was called "Pi GPU UI" before; keep its projects and settings.
#[cfg(target_os = "macos")]
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

/// The macOS menu bar, laid out like Zed's.
fn app_menus() -> Vec<Menu> {
    use crate::text_input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    vec![
        Menu {
            name: "Pi GPUI".into(),
            items: vec![
                MenuItem::action("About Pi GPUI", About),
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Settings…", hub::OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Pi GPUI", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Pi GPUI", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Session", hub::NewChat),
                MenuItem::action("Close Session", hub::CloseChat),
                MenuItem::action("Reload All Sessions", hub::ReloadSessions),
                MenuItem::action("Add Project…", hub::AddWorkspace),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Left Panel", hub::ToggleLeftDock),
                MenuItem::action("Right Panel", hub::ToggleRightDock),
                MenuItem::action("Files", hub::ToggleFiles),
                MenuItem::action("Changes", hub::ToggleChanges),
                MenuItem::separator(),
                MenuItem::action("Session Tree", chat::ToggleTree),
                MenuItem::action("Change Model", chat::ToggleModelPicker),
                MenuItem::action("Expand or Collapse Step Details", hub::ToggleDetails),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", hub::ToggleFullScreen),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", hub::Minimize),
                MenuItem::action("Zoom", hub::Zoom),
            ],
        },
    ]
}
