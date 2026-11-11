# Pi GPUI

A native GUI for the [pi](https://github.com/earendil-works/pi) coding agent, written in Rust with [GPUI](https://www.gpui.rs), the UI framework behind Zed. No Electron, no webview. It runs the pi you already have over RPC, with the same models, extensions, skills and sessions you use in the terminal. For macOS and Linux.

The app is 16 MB, its window is up in about 0.4 s, and it uses about 75 MB of memory at idle. Electron clients for pi take 0.5 to 1.1 s and 190 to 430 MB. Measured on an M1 Pro with no projects open, counting each app's own processes ([method and script](docs/BENCHMARKS.md)).

[![macOS 13+, Apple silicon](https://img.shields.io/badge/macOS-13%2B%20Apple%20silicon-111?logo=apple&logoColor=white)](#install)
[![Linux x86_64 and arm64](https://img.shields.io/badge/Linux-x86__64%20%C2%B7%20arm64%20(beta)-111?logo=linux&logoColor=white)](#install)
[![MIT](https://img.shields.io/badge/license-MIT-111)](LICENSE)

[Website](https://pi-gpui.dev) · [Download](https://github.com/bytesbay/pi-gpui/releases/latest) · [Plugins](docs/PLUGINS.md) · [中文](docs/README.zh-CN.md)

![Pi GPUI: sessions working in several projects, pi adding retries to a Rust client, and the change in the Changes panel](docs/demo.gif)

<sub>Model replies in this recording are scripted so the take is repeatable. The app, the file edits and the `cargo test` run are real.</sub>

## Install

**macOS** (Apple silicon, macOS 13 or newer)

```sh
brew install --cask bytesbay/tap/pi-gpui
```

Or download `Pi-GPUI-*.zip` from [Releases](https://github.com/bytesbay/pi-gpui/releases) and move the app to Applications. Releases are signed with a Developer ID certificate and notarized by Apple.

**Linux** (x86_64 and arm64, beta)

```sh
curl -fsSL https://pi-gpui.dev/install.sh | sh
```

The [script](site/install.sh) puts the app in `~/.local/pi-gpui.app`, links `~/.local/bin/pi-gpui` and adds a launcher entry. It needs no root. Or unpack `pi-gpui-*-linux-*.tar.gz` from [Releases](https://github.com/bytesbay/pi-gpui/releases) yourself. It runs on X11 and Wayland and needs glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36) and a Vulkan driver. See [Linux](#linux) for what differs from the Mac app.

On both systems the app updates itself: when a new version is downloaded, "Restart to update" appears in the title bar. Windows isn't supported.

Needs pi 1.4 or newer within pi 1.x. The app finds pi in `PI_EXECUTABLE`, `PATH` or your login shell (so nvm installs work).

To move a session you started in the terminal into the app, add the `/gpui` command to pi:

```sh
pi install npm:pi-gpui
```

`/gpui` closes that pi and opens the same session in Pi GPUI. Other tools can do the same with a link: `open "pi-gpui://open?workspace=/path/to/project&session=/path/to/session.jsonl"` (the session is optional). On Linux pass the link to the app: `pi-gpui "pi-gpui://open?..."`, or just a folder: `pi-gpui /path/to/project`.

## It runs DOOM

Plugins can draw live views inside tool calls, in any language. [This one](examples/plugins/doom) gives pi a `doom` tool:

![DOOM running inside a pi tool call in Pi GPUI](docs/doom.gif)

## Features

- Projects and their pi sessions in a sidebar. Every open session runs its own pi, so switching never stops work.
- Slash commands: pi's extension commands, prompt templates and skills, plus `/compact`, `/tree`, `/fork`, `/clone`, `/model`, `/new`, `/name`, `/session`, `/copy`, `/export`.
- Side panels like Zed's docks: projects on the left; the project's files (colored by git status) and its git changes in a floating panel on the right. Right-click a panel's tab to move it to the other side; drag a panel's edge to resize it.
- Session tree with keyboard navigation and branch summaries.
- Paste or drop files and images into the message field. They show up as chips in the text (hover an image to preview it) and are sent as paths, like in pi's terminal: a pasted image is saved to `$TMPDIR/pi-clipboard-*.png`, a file gets a temporary link. Images in messages and tool results are shown in the transcript.
- Fork from any message.
- Extension questions (`select`, `confirm`, `input`, `editor`) are asked inline in the transcript: pick with a click, digits or arrows, or type your own answer.
- Plugins in any language: buttons, keybindings and status, tools for pi, and live views inside tool calls. [It runs DOOM](examples/plugins/doom). See [docs/PLUGINS.md](docs/PLUGINS.md).
- Per-session model and thinking level, without touching pi's defaults.
- Markdown answers styled after Zed's agent panel, with syntax-highlighted code; all transcript text is selectable.
- A context ring next to the model: context used and when pi compacts, prompt cache hit rate and expiry, subscription plan limits, and token totals for the session.
- Compaction shown in the transcript with token counts and the summary.
- Ayu Dark and Ayu Light themes, following the system's light or dark mode by default, plus your own themes (six colors are enough) and Zed themes.
- Sessions can be renamed, hidden, closed (⌘W stops their pi, the session stays) or deleted (to the Trash).
- Updates for pi and its extensions next to the pi version, with versions and whether this app supports the new pi. Applied with pi's own `pi update`, only when you click.

## Keys

| Keys | Action |
|---|---|
| `⌘N` | New session |
| `⌘,` | Settings |
| `⌘O` | Add project |
| `⌘1` … `⌘9` | Switch to the n-th open session tab (hold `⌘` to see numbers) |
| `⌘⇧R` | Reload all open sessions (fresh pi: picks up updated pi, extensions, skills and settings) |
| `⌘B` / `⌘R` | Show or hide the left / right panel |
| `⌘⇧T` | Session tree |
| `⌘⇧C` | Compact the context (also the button next to the context ring) |
| `⌘⇧M` | Model and thinking level |
| `⌘⇧E` | Expand or collapse step details |
| `↩` / `⇧↩` | Send / new line (sending while pi works steers it) |
| `Esc` | Stop the run |
| `/` | Command menu |

In the tree: `↑↓` move, `←→` jump between branches, `↩` go there, `⌥↩` go there and summarize the branch you leave, `⌘↩` fork from your message.

On Linux `⌘` is `Ctrl` and `⌥` is `Alt`, with one exception: compact is `Ctrl+Alt+C`, because `Ctrl+Shift+C` copies the selection as it does in a terminal.

## Settings

`~/Library/Application Support/pi-gpui/settings.json` (on Linux `~/.config/pi-gpui/settings.json`) is the source of truth. The app applies it as soon as it is saved, so you or a pi agent can edit it directly; the Settings page (`⌘,`) writes the same file and keeps its comments.

```jsonc
{
  "theme": "system",              // or "Ayu Dark", "Ayu Light", or one of your themes
  "ui_font_family": ".SystemUIFont",
  "agent_font_size": 14,          // messages and answers
  "buffer_font_family": "JetBrains Mono NL",  // code; bundled with the app
  "buffer_font_size": 12,
}
```

### Themes

A theme is a JSON file in the `themes` folder next to `settings.json`; "New theme" on the Settings page creates one and opens it. Six colors are enough, the rest is derived from them. `style` overrides single colors with [Zed's theme keys](https://zed.dev/docs/themes), and a Zed theme file dropped into the folder works as is. Changes apply on save.

```jsonc
{
  "name": "Midnight",
  "background": "#14161b",
  "text": "#e3e1db",
  "accent": "#7aa2f7",
  "added": "#9ece6a",
  "removed": "#f7768e",
  "warning": "#e0af68",
  "style": { "panel.background": "#101216", "syntax": { "keyword": "#bb9af7" } },
}
```

## Compatibility

The app has its own version and supports a range of pi versions (`MIN_PI` and `TESTED_PI` in `src/pi.rs`). pi minors newer than the tested one still run, marked "not tested yet". Each release publishes `pi-compat.json`, and the updater only offers releases that support your pi. The app updates pi only when you click.

Tree navigation needs something pi's RPC lacks, so the app passes a small extension of its own (`extensions/pi-gpui-bridge.ts`) to the pi processes it starts. Nothing is installed.

## Linux

The Linux app is the same code as the Mac app. What differs:

- The window draws its own frame and buttons where the desktop lets it (GNOME, most Wayland compositors); otherwise the window manager's title bar is used.
- There is no menu bar. Settings, Check for Updates and Quit are under `…` in the title bar.
- Shortcuts use `Ctrl` (see [Keys](#keys)); text fields follow the usual Linux keys (`Ctrl+←/→` by word, `Home`/`End`, `Ctrl+Backspace`).
- Settings, projects and plugins live in `~/.config/pi-gpui` (or `$XDG_CONFIG_HOME/pi-gpui`).
- Deleted sessions go to the Trash through `gio`, `kioclient` or `trash-put`; without any of them nothing is deleted.
- Only one app runs per user: starting `pi-gpui` again, with a folder or a `pi-gpui://` link, hands it to the running app.
- A copy that was not installed into a `pi-gpui.app` folder (a distro package, a `cargo build`) does not update itself.

So far it was tested on Ubuntu 24.04 on arm64 in a VM (GNOME on X11, and Wayland under Weston). Other desktops, real GPUs and the x86_64 build have not been tried yet, hence beta: please report what you see in [#5](https://github.com/bytesbay/pi-gpui/issues/5).

## Known limits

- A pi running in a terminal on the same session is not detected.
- Extension status lines, widgets and window titles (`setStatus`, `setWidget`, `setTitle`) are not shown yet. Extension questions (`select`, `confirm`, `input`, `editor`) work.

## Development

```sh
cargo run [-- /path/to/project]
cargo test && cargo clippy --all-targets
script/bundle.sh        # macOS: target/release/Pi GPUI.app
script/bundle-linux.sh  # Linux: target/release/pi-gpui.app and the release tarball
script/release.sh       # tag the Cargo.toml version; CI builds and publishes the release
```

Rust 1.90 (pinned). On macOS you need Xcode; shaders compile at startup, so the Metal Toolchain is not needed. On Debian and Ubuntu `script/linux-deps.sh` installs the libraries the build needs (xkbcommon, Wayland, XCB, fontconfig, freetype); other distributions have the same ones under their own names. Set `PI_GPUI_PRETEND_VERSION=<older>` to try the update flow against a real release.

Working on the Linux app from a Mac: [docs/LINUX_VM.md](docs/LINUX_VM.md) describes a local VM with a desktop that can be built in, clicked and screenshotted from the command line.

### Signing

Releases are signed with a Developer ID certificate and notarized, so Gatekeeper opens them without a warning. The release workflow needs these repository secrets:

| Secret | What |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Developer ID Application certificate with its private key, exported as .p12, base64 (`base64 -i cert.p12 \| pbcopy`) |
| `MACOS_CERTIFICATE_PASSWORD` | the password set when exporting the .p12 |
| `APPLE_API_KEY_P8` | App Store Connect API key (.p8 file contents), role Developer |
| `APPLE_API_KEY_ID` | that key's ID |
| `APPLE_API_ISSUER_ID` | the Issuer ID shown above the keys list |

To sign and notarize locally:

```sh
xcrun notarytool store-credentials pi-gpui   # once; asks for the API key or an app-specific password
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" script/bundle.sh
NOTARY_PROFILE=pi-gpui script/notarize.sh
```

Docs: [principles](docs/APP_PRINCIPLES.md), [GPUI practices](docs/GPUI_STYLE_GUIDE.md), [plugins](docs/PLUGINS.md).

The app bundles [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) (SIL Open Font License 1.1, `assets/fonts/OFL.txt`).

## License

MIT, see [LICENSE](LICENSE).
