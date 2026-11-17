# UI checks on macOS

A debug build can be driven from a script: it takes keys and clicks and saves pictures of its own window. Nothing goes through the system, so this needs neither the Screen Recording nor the Accessibility permission, the real pointer and keyboard stay free, and the window can sit behind other windows the whole time. The code is `src/automation.rs`; release builds do not contain it.

For Linux there is a VM instead: [LINUX_VM.md](LINUX_VM.md).

## Starting an instance

Start the debug build with `PI_GPUI_AUTOMATION` set to a folder. Use a separate `HOME` so the test instance has its own projects, settings, plugins and pi sessions, and wrap the binary in an app bundle with its own identifier so it runs next to the installed app:

```sh
cargo build
cp -R "/Applications/Pi GPUI.app" /tmp/ui/Test.app    # any Pi GPUI.app works as the shell
plutil -replace CFBundleIdentifier -string dev.pi-gpui.uitest /tmp/ui/Test.app/Contents/Info.plist
cp target/debug/pi-gpui /tmp/ui/Test.app/Contents/MacOS/pi-gpui
codesign --force --deep -s - /tmp/ui/Test.app
mkdir -p /tmp/ui/home /tmp/ui/auto /tmp/ui/project
open -n -g /tmp/ui/Test.app --env HOME=/tmp/ui/home --env PATH="$PATH" \
  --env PI_GPUI_AUTOMATION=/tmp/ui/auto --args /tmp/ui/project
```

`open -g` keeps the window in the background, and with automation on the app does not bring itself forward.

pi in that `HOME` needs a model: point `~/.pi/agent/models.json` at any OpenAI-compatible endpoint. A small scripted server is enough to make pi call the tool you want to look at.

Keep plugins for the test instance inside its `HOME` as real copies. A link into `~/Documents` makes macOS ask for access to that folder, and the test instance waits for the answer.

## Commands

Write the commands, one per line, to a temporary file and rename it to `command` in the automation folder. The app runs them in order, removes the file and writes `done` with one line per command (`<command>: ok` or `<command>: error: ...`).

| Command | Effect |
|---|---|
| `key down space cmd-enter` | keystrokes, in gpui's notation |
| `type some text` | typed character by character |
| `click 640 400` | press and release the left button |
| `move 640 400` | move the pointer (hover) |
| `wait 500` | milliseconds |
| `shot /tmp/window.png` | PNG of the window, one pixel per point. `shot /tmp/window.png 2x` saves full resolution |

Coordinates are points from the window's top left corner, the same as pixels in a `shot` without `2x`. The window opens at 1180 x 800.

A helper for a shell session:

```sh
ui() {
  printf '%s\n' "$@" > /tmp/ui/auto/command.tmp
  rm -f /tmp/ui/auto/done
  mv /tmp/ui/auto/command.tmp /tmp/ui/auto/command
  until [ -f /tmp/ui/auto/done ]; do sleep 0.1; done
  cat /tmp/ui/auto/done
}
ui 'click 520 731' 'type hello' 'key enter' 'wait 3000' 'shot /tmp/window.png'
```

## Limits

- The app draws a frame after every command, because macOS stops the display link of a hidden window. Animations between commands are not captured.
- Clicks are plain left clicks: no drags, right clicks, scrolling or held modifiers.
- Native dialogs (the folder picker, confirmation prompts) are separate windows and are not in the picture.
