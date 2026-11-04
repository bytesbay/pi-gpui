# GPUI: component style and practices from Zed

A study of [zed-industries/zed][repo] at commit [`504a7f2cf25b817cda3998cab93473adef15f93c`][commit] (2026-11-02). Links are pinned to that commit. GPUI is pre-1.0, so code from `main` can differ from the published crate; this app uses `gpui 0.2.2` from crates.io, and the notes below call out differences we hit.

The guide separates GPUI contracts, Zed conventions and recommendations for our chat client. Examples are shortened and written for this guide.

## Quick reference

- One-off visual component: `#[derive(IntoElement)]` + `RenderOnce`.
- Long-lived state: `Entity<T>`. If it renders, `T: Render`.
- Component properties: private fields, `new(...)`, builder methods `mut self -> Self`.
- A visual state change: `cx.notify()`. A semantic event: `cx.emit(...)`.
- A handler that needs `self`: `cx.listener(...)`.
- Keep subscriptions and background tasks alive on purpose; dropping them changes behavior.
- `cx.spawn` runs on the foreground thread. CPU work belongs in `cx.background_spawn`.
- Interactive elements need stable IDs. In a changing list, use the data's ID, not the position.
- For a chat with rows of different heights: `list` with a stored `ListState`.
- Zed's `Button`, `Label`, `h_flex` and `cx.theme()` are not part of plain GPUI.

## 1. Where GPUI ends

| Layer | Provides |
|---|---|
| `gpui` | entities, contexts, render, `div`, layout, input, actions, lists, tasks |
| `gpui_platform` (newer GPUI) | app startup and platform backends; in 0.2.2 this lives inside `gpui` |
| Zed `ui` | Button, Label, Tooltip, list components, semantic styles, previews |
| Zed `theme` | themes, colors, sizes and appearance settings |
| Zed `agent_ui`, `markdown`, `editor` | application views, Markdown and the editor |

GPUI plus a small UI layer of our own is enough. There is no need to port the editor's whole component system.

Licenses in the studied version: `gpui` is Apache-2.0, `ui` and `theme` are GPL-3.0-or-later. Borrowing ideas and copying source have different consequences, so check licenses before including Zed components. [GPUI][gpui-readme], [ui/Cargo.toml][ui-cargo], [theme/Cargo.toml][theme-cargo].

A macOS detail: the current README requires `gpui_platform` with `font-kit` for glyph rasterization; without it, text gets laid out with no visible glyphs. Pin compatible GPUI versions and `Cargo.lock` instead of starting from a floating `*`.

## 2. The two main kinds of components

### `RenderOnce`: data and styling for one render

This is how `Button`, `Disclosure` and `Callout` work. The constructor takes required properties; optional ones are builder methods. The component is consumed by `render(self, ...)`.

A minimal example without Zed's `ui`:

```rust
use gpui::{prelude::*, App, Hsla, SharedString, Window, div};

#[derive(IntoElement)]
struct StatusBadge {
    label: SharedString,
    background: Hsla,
    foreground: Hsla,
    compact: bool,
}

impl StatusBadge {
    fn new(label: impl Into<SharedString>, background: Hsla, foreground: Hsla) -> Self {
        Self {
            label: label.into(),
            background,
            foreground,
            compact: false,
        }
    }

    fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }
}

impl RenderOnce for StatusBadge {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(self.background)
            .text_color(self.foreground)
            .when(self.compact, |this| this.text_sm())
            .child(self.label)
    }
}
```

Colors are passed in here; in the app they come from a small theme layer.

Worth copying from Zed:

- `impl Into<SharedString>` and `impl Into<ElementId>` in public constructors.
- Sensible defaults in `new` instead of a configuration object for every detail.
- `.when(...)` and `.when_some(...)` for conditional styling and children.
- Shared behavior delegated to a base component. Zed's `Button` wraps `ButtonLike`, which owns all button behavior.
- Callbacks are component properties; a component does not find a global store or send RPC itself.

Not every leaf needs its own `Entity`. But `RenderOnce` does not promise zero state: Zed's `CopyButton` uses `window.use_keyed_state`, element-local state with the limits described in section 4. [Button][button], [Disclosure][disclosure], [CopyButton][copy-button].

### `Entity<T>` + `Render`: a long-lived view

`ThreadsArchiveView` holds:

- a `FocusHandle`;
- a `ListState`;
- data and selection;
- a child `Entity<Editor>`;
- `Vec<Subscription>` and `Task<()>` fields;
- weak references to the workspace and services.

`new` creates the editor, the subscriptions and the list state. `render` builds the tree from current state, with larger parts split into `render_header`, `render_toolbar` and `render_list_entry`.

In our app, `ChatView` and `HubView` are views; badges, labels and buttons are `RenderOnce`. Long-lived child entities are created on initialization or when data changes, never on every render. [Real example][archive].

## 3. Structure and code style

Zed's conventions:

- `window: &mut Window` comes before `cx: &mut Context<Self>` or `cx: &mut App`.
- Use `&App` when a context is only read.
- Inside an `entity.update` callback, use the inner `cx`.
- Inside callbacks the mutable view is usually called `this`; in regular methods it is `self`.
- Full-word variable names, except the conventional `cx` and local indices.
- Comments explain why, not what the code does.
- No `unwrap()` on expected failures of files, network, processes or input.
- No silently swallowed errors with `let _ = ...`: show the user the failed operation and log the technical cause.
- Files like `components.rs` + `components/button.rs`, no `mod.rs`, and no separate file for every helper.

These are project conventions, not Rust rules. GPUI examples contain `unwrap()` and shortcuts that should not be copied into product code. [Zed rules][rules].

A consistent builder order helps readability: id/focus/actions, geometry, styling, events, children. That is our convention, not a GPUI requirement.

Keep `impl IntoElement` typed while it is convenient. `AnyElement` is for real type boundaries: `if`/`match` branches, heterogeneous collections, the `list` API. Zed calls `.into_any_element()` exactly there. Two more traps we hit in 0.2.2 with edition 2024: `impl IntoElement` returned from a method that takes `&mut Context` captures that borrow, so such methods cannot be called from a `map` closure; return `AnyElement` there. And functions returning `impl Fn` closures produce distinct types per function, so pick one constructor when the branch is dynamic (see `Tooltip::with_optional_keys`).

## 4. IDs and the lifetime of state

An ID affects interactive state, keyed local state and accessibility. A global ID includes the IDs of all ancestors, so uniqueness is required within that path, not across the app.

For a changing list:

```rust
// message_id is a usize that stays the same when the list is re-sorted.
div()
    .id(("message", message_id))
    .child(div().id("copy-button"))
```

Avoid:

- a UUID created in `render`: the element changes identity every frame;
- an array index as identity when rows are inserted, removed or sorted;
- several elements with the same global ID in one frame.

In this version `text!(...)` derives its ID from the call site, so one call in a loop yields identical IDs. Give each text a unique ID or give the row its own ID. [ID rules][a11y].

`window.use_keyed_state` keeps state while the element is rendered in consecutive frames. With virtualization an element can leave the tree.

So:

- a short "copied" flash can live locally;
- expanded tool cards, drafts, selection and edited text must survive scrolling. Store them in the view or model by message ID.

This follows directly from the `use_keyed_state` contract; it is not a general replacement for React hooks. [Contract][keyed-state].

## 5. Updates, events and ownership

| Mechanism | Use it to |
|---|---|
| `entity.read(cx)` | read state |
| `entity.update(cx, ...)` | change state |
| `cx.notify()` | signal a change that may affect rendering and notify observers |
| `cx.observe(...)` | react to another entity's notify |
| `cx.emit(Event)` | report a semantic event |
| `cx.subscribe(...)` | receive a typed event |

`update` does not replace `notify`, and `emit` does not request a redraw by itself.

```rust
fn toggle_details(&mut self, cx: &mut Context<Self>) {
    self.details_open = !self.details_open;
    cx.notify();
}
```

A `Subscription` is cancelled on drop. Store subscriptions in `_subscriptions: Vec<Subscription>`, or call `detach` deliberately. A local variable in `new` does not keep a subscription alive after the constructor returns.

Strong reference cycles between entities keep them alive. A parent may own a child; the back reference is usually a `WeakEntity`. Do not add `Arc<Mutex<View>>` for ordinary UI state: GPUI already manages access on the foreground thread.

Reading or updating an entity that is currently inside `update` panics at runtime. Work with the `&mut self` you were given instead of reaching back through a stored handle. [Ownership][ownership], [contexts][contexts], [double-lease check][entity-map].

For streaming, our default is to apply a small batch of deltas and notify the affected view once. Never refresh every window per token. Batch sizes are measured; a fixed 50 ms is not a GPUI rule.

## 6. Handlers and the keyboard

A plain handler has the signature:

```rust
Fn(&ClickEvent, &mut Window, &mut App)
```

When it needs the view, use `cx.listener`:

```rust
.on_click(cx.listener(|this, _event, _window, cx| {
    this.toggle_details(cx);
}))
```

`cx.listener` captures a weak reference to the entity. For callbacks that return a value, such as rendering a list row, there is `cx.processor`.

A subtle difference: in the studied version `cx.processor` captures a strong `Entity`. It is fine for the callback of an element being built, but storing it inside the same entity creates an ownership cycle. [Both helpers][callbacks].

Keyboard commands are actions:

```rust
actions!(chat, [SendMessage, CancelRun]);

// In render:
div()
    .key_context("Chat")
    .track_focus(&self.focus_handle)
    .on_action(cx.listener(Self::send_message))
    .on_action(cx.listener(Self::cancel_run))
```

`KeyBinding`s for the `Chat` context are registered separately. An action handler receives `&SendMessage`, `&mut Window` and `&mut Context<Self>`. Actions with data (we use one for plugin commands) use `#[derive(gpui::Action)]` with `#[action(namespace = ..., no_json)]`. `KeyBinding::new` panics on an invalid keystroke, so validate user-provided keystrokes with `Keystroke::parse` first.

Mouse and keyboard must trigger the same action. Keep a `FocusHandle`, hand focus to the editor correctly and restore it after popups close. `key_context` does not set focus by itself.

Zed's `ButtonLike` owns hover, active, focus-visible, disabled, accessibility and propagation. A button inside a clickable row must not also trigger the row's action. `stop_propagation()` and `window.prevent_default()` solve different problems: stopping propagation versus preventing default behavior such as moving focus. Use each on purpose. [ButtonLike][button-like], [actions and focus example][testing].

## 7. Async: what really runs off the UI thread

- `cx.spawn`: async on the foreground thread. Heavy synchronous code inside it still blocks input and frames.
- `cx.background_spawn`: work on the background executor. Pass owned data in and apply the result on the foreground.
- A `Task` cancels its work on drop. Store the handle in a field, await it or detach it explicitly.
- `_load_task: Task<()>` is often initialized with `Task::ready(())`; assigning a new task cancels the old one.
- Cancelling a future does not kill an external process or interrupt a CPU-bound function mid-call. Those need their own stop protocol.
- Do not hold `&mut Window`, `&mut App`, `Context<T>` or borrowed entity state across an `await`. Use the async context and short `update` calls.

A good real-world pattern is `Markdown::start_background_parse`:

```text
source snapshot
    → background parse and highlighting
    → foreground: apply the result
    → notify
```

While a parse is in flight, new changes set `should_reparse`. When it finishes, the current source is processed. Instead of a queue of parse jobs per token there is one current parse plus a flag for new work. [Markdown][markdown-parse].

For us:

1. The RPC reader keeps reading the stream independently of rendering.
2. Session switches carry a session identity; a late response never overwrites a new chat.
3. The Markdown parser never runs unbounded on every delta.
4. Process and network errors reach the UI. A view that disappeared after its window closed is an expected case and handled separately.
5. If a library needs Tokio, run a Tokio runtime explicitly and bridge with channels. The GPUI executor is not a Tokio runtime.

## 8. Lists: choose the API by row height

| Content | API |
|---|---|
| Rows of equal height: models, simple sessions | `uniform_list` |
| Markdown, tool results, attachments of varying height | `list` + `ListState` |

`uniform_list` passes the range of indices it needs. Build only that range; never prepare the full element list up front. [Example][uniform-list].

`ListState` is stored in the view, not recreated in every `render`. A typical call in Zed:

```rust
list(
    self.list_state.clone(),
    cx.processor(Self::render_list_entry),
)
.flex_1()
.size_full()
```

`render_list_entry` receives an index and returns an `AnyElement`. Message data stays in the model; elements are created on demand.

The main trap for a chat: the list caches heights, including those of invisible rows. When a message grows, a tool card expands or an image loads, report the geometry change. To replace one row:

```rust
self.list_state.splice(index..index + 1, 1);
cx.notify();
```

`splice` invalidates the list's view of that range; the data itself is updated separately.

- `reset` is for replacing the whole set. In the current implementation it drops the scroll anchor; never reset per token.
- `measure_all()` measures everything on first layout. That is a deliberate cost, not a free scrollbar improvement.
- `overdraw` is in pixels. Do not copy Zed's value without measuring with our messages.
- If a row contains a focusable editor, look at `splice_focusable`: a focused element may keep rendering outside the viewport.
- For follow-bottom, separate "the user is reading history" from "follow the answer". A new delta must not pull a reader down.

In 0.2.2, `ListAlignment::Bottom` gives this behavior: the list follows the end while the user is at the bottom and keeps the position otherwise. `scroll_to` pins a position and disables following; `reset` re-enables it. Invalidation and measurement contracts are in the [List source][list-source].

## 9. Layout, themes and caching

Zed's styling practices:

- `h_flex()` and `v_flex()` shorten common flex combinations; our own versions take a few lines.
- Semantic colors (`Muted`, `Error`, `Success`) and theme tokens instead of hex values across the app.
- Text that stretches in a row needs permission to shrink and a bounded overflow. Zed pairs truncation with `min_w_0().overflow_hidden()`.
- Icons and buttons next to text must not shrink by accident; use `flex_shrink_0` and sizing.
- A scroll container needs a bounded area. `overflow_y_scroll` does not replace constraining the parent's size.
- An ellipsis suits a dynamic session title. Fixed labels of important actions are better left untruncated.

`px(...)`, `rems(...)`, `Pixels` and `Hsla` make units explicit. Do not turn everything into unrelated `f32`s.

### `Entity` is not React.memo

A plain `.child(entity)` does not promise to cache the subtree.

`cached(StyleRefinement)` reuses a view's subtree until it is notified or the cache conditions change, but it needs a definite size: the size comes from the style, not from measuring content. Do not apply `cached` blindly to an auto-height Markdown message. Start with virtualization, background work and targeted updates; then profile and choose cache boundaries. [cached contract][cached].

### Animations: measured in this app

In GPUI 0.2.2, `with_animation` requests a frame on every display refresh (120 Hz on ProMotion), and `notify` marks the view and all its ancestors dirty. A single spinner made the whole window re-render every frame and cost 25-33% of a core, in release builds too.

What we do instead (`src/ui/animation.rs`):

- loading components (`Spinner`, `LoadingLabel`) derive their phase from wall time;
- a view that shows loading state re-renders on a 12.5 fps timer (`AnimationTicker`), and only while something is loading;
- heavy child views are embedded with `AnyView::cached`, so ticks elsewhere reuse their last frame.

Result: about 6% CPU while pi works, about 0% when idle. Account for reduced motion and for animations that are not visible.

### A custom title bar on macOS

Use `TitlebarOptions { appears_transparent: true, traffic_light_position }` and draw a bar of your own. In GPUI 0.2.2, `Window::start_window_move` does nothing on macOS, so the window cannot be dragged by such a bar. Like newer Zed versions, we call `-[NSWindow performWindowDragWithEvent:]` on the first mouse move after a mouse down in the bar (`src/platform.rs`). Double-click: `window.titlebar_double_click()`. Zed's title bar is 34 px with traffic lights at (9, 9) and 78 px of left padding on the macOS 26+ SDK.

## 10. Text input and accessibility

`div().on_key_down(...)` is not a text editor.

`examples/input.rs` shows what is required: `EntityInputHandler`, selection, marked text for IME, clipboard, bounds and converting the platform's UTF-16 offsets to the UTF-8 offsets of a Rust string. Emoji and combined characters quickly break a "delete one byte" implementation.

For the composer, prefer a proven input/editor component with a suitable license. If you write your own, test IME, non-Latin text, emoji, paste, selection and undo separately. [Input example][input].

A UI layer of our own needs roles, accessible button names, expanded/toggled state, tab navigation and visible focus. A tooltip does not replace an accessible name, and color must not be the only error indicator. [GPUI accessibility][a11y], [Disclosure][disclosure].

## 11. Previews, tests and performance checks

Zed's visual components implement `Component::preview` and show their variants: default, disabled, selected, sizes, long labels. `RegisterComponent` belongs to Zed's infrastructure.

A separate gallery view is enough for us. The point is to see all states of a component side by side, not to port Zed's registry. [CopyButton example][copy-button].

For behavior:

- `#[gpui::test]` + `TestAppContext` for models, events and tasks.
- `VisualTestContext` for windows, focus and action dispatch.
- GPUI executor timers and `run_until_parked` in deterministic tests, never arbitrary sleeps.
- Replace external processes and the network with controlled event sources.
- Test closing a view with a pending task, unsubscribing, fast session switches and that no delta is lost or reordered.

Performance must be measured in a production-like configuration. Zed states it explicitly: a benchmark must not enable `test-support`; use `bench-support` for that infrastructure.

Measure foreground work, input latency and frames during streaming, not only the total time to process an answer. The budget is about 8.33 ms per frame at 120 Hz and about 16.67 ms at 60 Hz. That is a target, not a GPUI guarantee. [Tests][testing], [benchmark method][bench], [UI checklist][ui-checklist].

## 12. Review rules for pi-gpui

- No RPC, file I/O or full Markdown parse inside `render`.
- Subscriptions, tasks and child entities have a clear owner.
- Rows keep their identity and needed state after scrolling.
- Height changes notify `ListState`, without a full reset per token.
- Search computes matches on data and does not require rendering the whole history.
- Click and shortcut lead to the same action; disabled really blocks the call.
- Errors, loading and cancellation are visible to the user.
- Behavior is measured on a long chat while typing, generating and resizing.

## Sources to read

| Start with | Look at |
|---|---|
| [Disclosure][disclosure] | a small builder, composition, `RenderOnce` |
| [Button][button] + [ButtonLike][button-like] | a shared base, semantic properties and interactions |
| [CopyButton][copy-button] | keyed state, transient state, preview |
| [ThreadsArchiveView][archive] | entity, subscriptions, focus, helpers and list |
| [Markdown parse][markdown-parse] | a background task and coalesced updates |
| [List][list-source] | the height cache contract and splice |
| [Input][input] | what a real text input requires |
| [Testing][testing] | practical tests of models, tasks and actions |

[repo]: https://github.com/zed-industries/zed
[commit]: https://github.com/zed-industries/zed/commit/504a7f2cf25b817cda3998cab93473adef15f93c
[rules]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/AGENTS.md
[gpui-readme]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/README.md
[ui-cargo]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/ui/Cargo.toml
[theme-cargo]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/theme/Cargo.toml
[button]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/ui/src/components/button/button.rs
[button-like]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/ui/src/components/button/button_like.rs#L756-L899
[disclosure]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/ui/src/components/disclosure.rs
[copy-button]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/ui/src/components/button/copy_button.rs
[archive]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/agent_ui/src/threads_archive_view.rs#L140-L237
[ownership]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/_ownership_and_data_flow.rs
[contexts]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/docs/contexts.md
[entity-map]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/app/entity_map.rs#L175-L199
[keyed-state]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/window.rs#L4586-L4623
[callbacks]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/app/context.rs#L235-L264
[uniform-list]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/examples/uniform_list.rs
[list-source]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/elements/list.rs
[markdown-parse]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/markdown/src/markdown.rs#L1246-L1450
[cached]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/view.rs#L249-L260
[animation]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/elements/animation.rs
[input]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/examples/input.rs
[a11y]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/src/_accessibility.rs
[testing]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/crates/gpui/examples/testing.rs
[bench]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/.agents/skills/gpui-bench/SKILL.md
[ui-checklist]: https://github.com/zed-industries/zed/blob/504a7f2cf25b817cda3998cab93473adef15f93c/docs/src/development/ui-checklist.md
