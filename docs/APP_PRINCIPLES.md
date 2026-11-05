# Pi GPUI: purpose and rules

This document records the decisions behind the app. [GPUI_STYLE_GUIDE.md](GPUI_STYLE_GUIDE.md) covers implementation practices.

## Purpose

The app is a native hub for working with pi: projects, sessions, talking to the agent, watching runs and controlling them from one window.

The agent engine stays in pi. We do not build our own agent loop, duplicate model providers or port pi's tools to Rust.

## 1. Work with the installed pi

- The user installs and authenticates pi.
- The app finds the pi executable. The user can point it at another one explicitly.
- We do not download pi, ship our own copy or install it on first launch.
- We never install or update pi, its packages or dependencies on our own. The app shows available updates next to the pi version and runs pi's own `pi update` only when the user clicks; a pi update that would leave the supported range is not offered.
- If pi is missing, the app shows a clear message with installation instructions.

The runtime dependency is a compatible installed pi. GPUI and the Rust crates we use are build dependencies of the app, not extra programs the user has to install.

## 2. Talk over CLI/RPC, without an SDK wrapper

The app starts `pi --mode rpc` directly as a child process with the right working directory and arguments.

```text
GPUI app
    ↕ JSONL over stdin/stdout
installed pi --mode rpc
    ↕
models, tools, extensions and project files
```

- We do not use the TypeScript SDK, a Rust SDK client or a Rust port of pi.
- We do not run `bash -c` to drive the agent. The program path and arguments are passed to the process separately; shell commands are never assembled from user text.
- The RPC process stays alive for further commands. We do not spawn a new process per message or delta.
- Commands go to stdin; responses and events are read from stdout continuously; diagnostics are read from stderr separately.
- stdout is parsed as JSONL split on LF. Commands and responses are matched by ID, not by arrival order.
- Events arrive as a stream. There is no need to poll every 50 ms.

Shell commands the agent runs as a tool are part of pi's work. They are not a transport between the app and the agent.

## 3. The app supports a range of pi versions

The app has its own version (semver, currently `0.x`). pi support is a range kept in `src/pi.rs`:

- `MIN_PI`: the oldest supported pi. Older versions are refused.
- The major of `MIN_PI`: a new pi major is refused until an app release supports it.
- `TESTED_PI`: the newest pi minor the app was tested with. Newer minors of the same major run, with a note in the sidebar ("not tested yet").

```text
supported(pi) = pi >= MIN_PI AND pi.major == MIN_PI.major
untested(pi)  = (pi.major, pi.minor) > (TESTED_PI.major, TESTED_PI.minor)
```

| `MIN_PI` / `TESTED_PI` | Installed pi | Result |
|---|---|---|
| 1.4.0 / 1.4 | 1.3.9 | Refused: update pi |
| 1.4.0 / 1.4 | 1.4.7 | Supported |
| 1.4.0 / 1.4 | 1.6.0 | Runs, marked "not tested yet" |
| 1.4.0 / 1.4 | 2.0.0 | Refused: check for app updates |

This is common practice for clients of tools made by others (compare kubectl's version skew or the `engines` range of VS Code extensions): a new pi minor does not break the app, and the app version describes the app.

Connection check:

1. Resolve the exact pi executable.
2. Read its version with `--version`, with a timeout.
3. Check the range above.
4. Start RPC only for a supported pi. Check again every time a process is started.

If the version cannot be read or parsed, the connection is refused. The app shows the executable path, the detected version or error, and what to do: update pi, or check for app updates.

Within the range, features that need something pi lacks are switched off one by one (feature detection), instead of refusing the whole connection.

We never update pi without the user and never swap the executable.

Each release publishes `pi-compat.json` (`pi-gpui --pi-compat`) with this range, so the updater only offers releases that support the installed pi. Raise `MIN_PI` only when the app needs something newer; raise `TESTED_PI` after testing a new pi minor.

## 4. pi owns the agent and its data

| pi is responsible for | The app is responsible for |
|---|---|
| The agent loop and model calls | Windows, panels and navigation |
| Tools and extensions | Projects and switching sessions |
| Authentication and pi resources | Input, rendering the stream and results |
| History, branches, compaction | UI state and drafts |
| Running and stopping agent work | Managing RPC processes and showing their state |

pi sessions stay usable from the CLI. We do not create a second, incompatible format for agent history.

We do not rewrite pi's JSONL, credentials or settings behind its back. Changes go through supported commands. Our own UI data is stored separately. A display cache can be rebuilt from pi's data; it is never the source of truth for model context.

Connecting over RPC does not mean every extension UI is supported. Supported dialogs are shown explicitly; limits of terminal-only or HTML components are explained to the user.

## 5. The hub controls work without hiding it

The user should always see:

- which pi is used and where it is installed;
- which project and session the work happens in;
- which model and mode are selected;
- whether a run is in progress, waiting for input, finished or failed;
- how to stop the work and where to find error details.

Switching tabs or chats does not cancel a run by itself. Rules for quitting the app with background work still running need a decision before they are implemented.

Accepting an RPC command does not mean the work is done. In the pi version we studied, `agent_settled` marks the end of automatic work; the client must follow the contract of the supported branch.

We never automatically repeat a command that may change files after a lost connection; we first find out the state of the run. Two writers of one session must never be started silently.

pi 1.4 does not prevent two processes from opening the same session. So inside the app a session is opened at most once, and a session with a live pi-gui lease file (`<session>.jsonl.lease`) is not opened. A terminal pi leaves no lease files, so that conflict cannot be detected yet.

The session list is built by reading pi's session directory (read-only, matched by the `cwd` field in each file header): pi has no RPC command that lists sessions. A session is opened with `pi --mode rpc --session <file>`, and its history comes from the `get_messages` command.

Extra hub features, such as schedules, a Git panel or remote machines, are discussed separately. They must never become required for opening a project and talking to pi.

## 6. Responsiveness is a requirement

- The UI never waits synchronously for a process, the network, the filesystem or a Markdown parser.
- Typing and scrolling stay responsive while the model generates and tools run.
- Long histories are rendered as a virtualized list.
- The stream is applied as changes to individual messages, without rebuilding the whole history for each delta.
- UI updates may be batched, but text must never be lost, events never reordered and errors never hidden.
- Queues have a bounded size and defined behavior under load. Closing a window leaves no forgotten tasks or processes.
- Every wait is visible immediately: sending, starting pi, loading history and sessions, and running tools show a spinner or a pulsing label.
- Loading animations do not render at the display refresh rate. A view with something loading re-renders on a 12.5 fps timer, and nothing renders when nothing loads (measured: a spinner on `with_animation` cost 25-33% CPU, the timer-driven one about 6%).

GPU rendering alone does not guarantee speed. We measure input latency, frames, memory and behavior on long chats.

## 7. The app is built on plugins

- The agent side (tools, agent commands, models, context) is extended by pi extensions and runs in the pi process.
- When RPC lacks something the app needs from pi (today: moving within the session tree), the app ships its own small pi extension (`extensions/pi-gpui-bridge.ts`) and passes it to its own pi processes with `--extension`. It is not installed and does not affect pi elsewhere.
- The interface and input (dictation, buttons, status, panels) are extended by app plugins: separate processes in any language, JSON lines over stdio. Specification: [PLUGINS.md](PLUGINS.md).
- A new interface feature that the core (chat, sessions, projects) does not need is first attempted as a plugin. That way the API is tested by its own authors.
- A plugin never runs code inside the app: a crashed or hung plugin cannot break the window.
- Plugins declare the plugin protocol version they speak; plugins for another protocol are not loaded.
- Plugin commands are visible before the plugin starts: the process starts on first use.

## 8. Minimal infrastructure of our own

- One execution backend: pi.
- One main transport: its RPC over stdio.
- No mandatory Node host of our own, extra daemon or SDK layer.
- No server infrastructure for basic local work.
- A dependency is added for a concrete task. A JSON parser or a UI library is fine; a duplicate agent engine is not.
- We do not copy the whole architecture of Zed or T3. We take only what helps this app.

## Before adding a feature

1. Is this an interface/hub task, or already pi's responsibility?
2. Does the supported RPC branch have a command or event for it?
3. Can it be done without installing an extra runtime?
4. What happens on cancellation, process failure and session switch?
5. Does it hurt typing, scrolling or long histories?
