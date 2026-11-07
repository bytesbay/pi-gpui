# App plugins

Plugins are the main way to extend Pi GPUI. New interface features that the agent does not need are built as plugins too, so the API gets tested by its own authors.

## What goes where

| Need | Extend with | Runs in |
|---|---|---|
| Tools, agent commands, models, context | a pi extension | the pi process |
| Input, buttons, status, panels, custom UI | an app plugin | the plugin's own process |
| Themes, grammars, icons | data in a plugin (planned) | the app |

An app plugin does not depend on pi sessions: it works with no session open and while the agent is busy.

## How a plugin works

A plugin is a folder with `plugin.json` and a program written in any language. The app starts the program as a separate process and exchanges JSON lines with it over stdin/stdout. If the plugin crashes or hangs, the app keeps working. The signal to exit is the end of stdin; the same happens when the app quits.

The app looks for plugins in:

1. `~/Library/Application Support/pi-gpui/plugins/<plugin>/`;
2. `Contents/Resources/plugins/` in the app bundle, or `plugins/` in the repository when run from a build (built-in plugins; none ship today).

Later, plugins will be installable as pi packages (`pi install`): a plugin folder inside the package, referenced by our key in the package manifest.

### `plugin.json`

```json
{
  "id": "dictation",
  "name": "Dictation",
  "version": "0.1.0",
  "protocol": 1,
  "run": ["./dictation"],
  "contributes": {
    "commands": [
      {
        "id": "dictation.toggle",
        "title": "Dictation",
        "icon": "microphone",
        "keybinding": "cmd-shift-d",
        "placement": ["composer"]
      }
    ]
  }
}
```

- `protocol`: the plugin protocol version (currently `1`). A plugin for another protocol is not loaded. The protocol changes only when messages change incompatibly, not with every app release.
- `run`: the program and its arguments. A path starting with `./` is relative to the plugin folder. Other names are looked up in `PATH` and in the user's login shell, so `["node", "index.js"]` finds Node installed through nvm.
- `contributes.commands`: commands. The app shows them **before the plugin starts**; the process starts on first use.
  - `id`: must start with the plugin id and a dot.
  - `placement`: where to show a button. `composer` (the message field) is supported today.
  - `icon`: `microphone`, `terminal`, `file`, `sparkle`, `message`, `tool`, `folder`, `refresh`, `check`, `info`, `warning`.
  - `keybinding`: a GPUI keystroke (`cmd-shift-d`).
- `contributes.piExtensions`: pi extension files (relative to the plugin folder) that the app loads into every pi it starts, for example a tool the agent can call.
- `contributes.toolEvents`: pi tool names whose calls the plugin follows. When pi starts one, the app sends `tool.start` (starting the plugin if needed), for example to ask the user something in that call with `prompt.input`.
- `contributes.toolViews`: live views the plugin draws inside tool call rows. `tool` is the pi tool name, `width` and `height` the frame size (the view keeps that aspect ratio), `title` a name for placeholders.
- `activation`: `lazy` (default) starts the plugin on first use; `startup` starts it with the app, for plugins that serve something (a local server, a bridge).
- `permissions`: `["sessions"]` lets the plugin list, open and drive pi sessions and follow their events (see Sessions). Without it `sessions.*` requests fail.

### Live views

When pi calls a tool that has a view, the app sends `view.open` and shows the plugin's frames in that tool call. A plugin streams to one call at a time: a newer call of the same tool takes the stream over, and older rows keep their last frame. Frames for other calls are dropped. Clicking a view gives it the keyboard (`view.focus`), keys arrive as `view.key`, and Esc or a click elsewhere gives the keyboard back. Release held keys when focus goes away.

The [DOOM plugin](../examples/plugins/doom) is a complete example: a pi tool `doom`, and a game that runs only while the view has the keyboard.

### Prompts

A plugin can ask the user for text in a field inside a tool call: `prompt.input` with the call's `callId`. The field takes the keyboard; Enter or the submit button answers, Esc or Cancel cancels, and the answer comes back as `prompt.result`. With `secret: true` the text shows as dots, cannot be copied, and has an eye button to reveal it. The answer goes only to the plugin, never to pi or the transcript.

A prompt closes when it is answered, when the plugin sends `prompt.close`, when the plugin exits, or when its tool call ends (then `prompt.result` comes with `cancelled` and `closed`, and nobody waits for an answer).

The [Secrets plugin](../examples/plugins/secrets) is a complete example: a pi tool `request_secret` that waits while the plugin asks for the value in a masked field, stores it in a file and tells the tool only the outcome.

## Protocol 1

Every message is a JSON object on a single line; lines end with `\n`. A message looks like `{"method": "...", "params": {...}}`. Most messages are notifications without a reply.

A plugin message with an `id` is a request: the app always answers it with `{"id": ..., "result": {...}}` or `{"id": ..., "error": {"message": "..."}}`, also for unknown methods. Requests may be answered out of order.

### App → plugin

| Method | Params | When |
|---|---|---|
| `initialize` | `protocol`, `app`, `plugin.id`, `plugin.dir` | right after start |
| `command.run` | `command`, `workspace?`, `session?` | the user pressed a button or keybinding |
| `view.open` | `tool`, `callId`, `args`, `workspace?`, `session?` | pi called a tool this plugin draws; stream to `callId` from now on |
| `view.focus` | `callId`, `focused` | the view got or lost the keyboard |
| `tool.start` | `tool`, `callId`, `args`, `workspace?`, `session?` | pi started a call of a tool in `contributes.toolEvents` |
| `prompt.result` | `id`, `value` or `cancelled`, `closed?` | the user answered or cancelled a prompt; `closed` means its tool call ended first |
| `view.key` | `callId`, `key`, `down` | a key went down or up in a focused view. `key` is GPUI's name (`up`, `space`, `a`; the typed character on non-Latin layouts); modifiers come as `control`, `shift`, `alt` |

### Plugin → app

| Method | Params | Effect |
|---|---|---|
| `composer.insert` | `text`, `replaceLast?`, `spaceBefore?` | inserts text at the cursor of the active message field. `replaceLast` replaces the plugin's previous insertion (for partial results). `spaceBefore` adds a space when the character before the insertion is not whitespace |
| `composer.setText` | `text` | replaces the whole message field |
| `command.setState` | `command`, `active` | toggled state of the button (for example, recording) |
| `status.set` | `id`, `text`, `tone?`, `busy?` | a status item in the title bar; `busy` shows a spinner; `tone`: `info`, `warning`, `error`, `success` |
| `status.clear` | `id` | removes a status item |
| `notify` | `message`, `tone?` | a notice in the window |
| `view.frame` | `callId`, `data`, `mimeType` | a frame for a live view: base64 `image/jpeg` or `image/png`. Ignored unless `callId` is the call the plugin streams to |
| `prompt.input` | `id`, `callId`, `title`, `description?`, `placeholder?`, `submitLabel?`, `secret?` | a text field in that tool call (see Prompts). The same `id` again updates the open prompt |
| `prompt.close` | `id` | removes an open prompt without an answer |
| `log` | `message` | a line in the app log (stderr) |

The app ignores unknown methods, so the protocol can grow without breaking older plugins.

### Sessions

With `"permissions": ["sessions"]` a plugin can drive pi sessions the way the user does: what it sends shows in the transcript, and the user sees the same session. An open session has a short id (`"1"`, `"2"`, …) that stays valid until the app quits.

Requests:

| Method | Params | Result |
|---|---|---|
| `sessions.list` | `limit?` (recent sessions per project, default 20) | `projects`: `[{path, name, sessions: [{file, title, modified, hidden, open}]}]` (`modified` in Unix ms, `open` the id when open); `open`: session summaries |
| `sessions.open` | `workspace`, `file?`, `focus?` | the summary. Opens the session file, or a new session when `file` is absent; adds the project to the sidebar if needed. `focus` also shows it in the window |
| `sessions.get` | `session` | the summary |
| `sessions.focus` | `session` | the summary; the session is shown in the window |
| `sessions.prompt` | `session`, `message`, `mode?` | pi's `prompt` response data (`disposition`). While pi works the message steers; `mode: "followUp"` queues it until the run ends |
| `sessions.abort` | `session` | stops the run, like Esc |
| `sessions.answer` | `session`, `question`, `answer?` / `confirmed?` / `cancel?` | answers the extension question pi waits on. `answer` is an option (or its 1-based number) or text |
| `sessions.rpc` | `session`, `command` | pi's response data for a pi RPC command. Allowed: reads (`get_state`, `get_messages`, `get_session_stats`, `get_commands`, `get_available_models`, `get_tree`, …) and settings (`set_model`, `set_thinking_level`, `set_session_name`, `compact`, `bash`, `clear_queue`, …). Prompts, forks and session switches are not |
| `sessions.subscribe` | `session` (an id or `"*"`) | starts the events below |
| `sessions.unsubscribe` | `session` | stops them |

A session summary is `{id, workspace, file, title, status, running, compacting, model, thinkingLevel, question, active}`. `status` is `ready`, `busy` or `stopped`; `question` is the extension question pi waits on (`{id, kind, title, message, options}`) or `null`; `active` means it is shown in the window.

Events for subscribed sessions (app → plugin):

| Method | Params | When |
|---|---|---|
| `session.events` | `session`, `events` | pi's own records of that session, in order: the [JSON event stream](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/json.md) (`agent_start`, `message_update`, `tool_execution_end`, `agent_settled`, …) and `extension_ui_request`. Responses to commands are not included. `{"type": "process_exited"}` when pi stops |
| `session.state` | `session` (a summary) | a session opened, or its title, file or running state changed |
| `session.closed` | `session` (the id) | the session was closed (its project removed) |

### JavaScript example

```js
import readline from "node:readline";

const send = (method, params = {}) =>
  process.stdout.write(JSON.stringify({ method, params }) + "\n");

readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const { method, params } = JSON.parse(line);
  if (method === "command.run" && params.command === "hello.insert") {
    send("composer.insert", { text: "Hello from a plugin", spaceBefore: true });
  }
});
```

`readline` is fine for protocol 1: the app never writes the U+2028/U+2029 characters inside lines (it escapes them).

## Performance

A plugin runs next to the user's work all day, so it should cost nothing while idle.

- Wait on events, never poll: stdin lines, file watches (`fs.watch`, FSEvents), sockets. A short timer is acceptable only as a fallback when an event can be missed.
- Start fast and do heavy setup lazily, on the first command or view that needs it.
- Send only what changed: one `status.set` per state change, not per tick. Frames only while a view is live and focused, at the rate the content changes.
- Keep messages small. Encode frames at the size the view shows (JPEG for video, PNG for flat UI).
- Cache what you read and invalidate it on change events, not on every call.
- Exit when stdin closes and release everything you opened.

## Permissions

macOS asks for permissions on behalf of the app that started the process. While Pi GPUI runs from a terminal, the permission prompt names the terminal; for a bundled `.app` it names the app.

## Security

A plugin runs with the user's permissions, like a pi extension. Install plugins only from sources you trust. A plugin cannot run code inside the app or break its interface: it can only send the messages described above.

## Planned

- A binary channel for view frames, for larger views at full frame rate.
- Panels and tabs described as data built from our components.
- Highlighting grammars and themes as data.
- Installing plugins as pi packages and a plugin list in settings.
