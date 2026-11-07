# Contributing

Issues and pull requests are welcome. Issues labeled [good first issue](https://github.com/bytesbay/pi-gpui/labels/good%20first%20issue) name the files to start from.

## Setup

```sh
cargo run [-- /path/to/project]
cargo test && cargo clippy --all-targets
```

You need a Mac with Xcode and pi 1.4 or newer. Rust is pinned in `rust-toolchain.toml`.

## Before you write code

- Read [docs/APP_PRINCIPLES.md](docs/APP_PRINCIPLES.md) and [docs/GPUI_STYLE_GUIDE.md](docs/GPUI_STYLE_GUIDE.md). They explain how the app talks to pi and how views are built.
- For a bigger change, open an issue first so we agree on the approach.
- Plugins don't need changes in the app. See [docs/PLUGINS.md](docs/PLUGINS.md) and the [examples](examples/plugins).

## Pull requests

- One change per PR, with a short description of what and why.
- Commit messages: lowercase conventional commits, e.g. `fix(chat): keep scroll position after compaction`.
- Run `cargo fmt`, `cargo test` and `cargo clippy --all-targets` before pushing.
- For UI changes, add a screenshot or a short recording.
