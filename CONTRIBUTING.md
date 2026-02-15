# Contributing

Thank you for your interest in contributing to Raven File Manager.

## Development Setup

1. Install dependencies (GTK4, Libadwaita, Rust 1.70+)
2. Clone the repository
3. Run `cargo test --workspace` to verify everything compiles

## Project Structure

Raven is a Rust workspace with 12 crates. See [ARCHITECTURE.md](ARCHITECTURE.md) for full details.

- Backend logic lives in the `crates/` directory
- The GTK4 UI lives in `crates/raven-ui/`
- The application entry point and command loop is `src/main.rs`
- Shared types go in `crates/raven-core/`

## Development Workflow

### Running Tests

```sh
# All tests
cargo test --workspace

# Specific crate
cargo test -p raven-ai
cargo test -p raven-core

# Single test
cargo test -p raven-ai test_parse_images
```

### Adding a New Command

1. Add the variant to `AppCommand` in `crates/raven-core/src/commands.rs`
2. Add corresponding event(s) to `AppEvent` in `crates/raven-core/src/events.rs`
3. Handle the command in the match loop in `src/main.rs`
4. Handle the event(s) in `RavenWindow::handle_event()` in `crates/raven-ui/src/window.rs`

### Adding a New Widget

1. Create the widget file in `crates/raven-ui/src/widgets/`
2. Add `pub mod <name>;` to `crates/raven-ui/src/widgets/mod.rs`
3. Wire it into `RavenWindow` in `crates/raven-ui/src/window.rs`

### Adding a New Crate

1. Create the crate directory under `crates/`
2. Add it to the `[workspace]` members list in the root `Cargo.toml`
3. Add it to `[workspace.dependencies]` for cross-crate usage
4. Depend on `raven-core` for shared types

## Code Style

- Follow standard Rust conventions (`cargo fmt`, `cargo clippy`)
- Use `tracing` for logging (not `println!` or `eprintln!`)
- Error types go in each crate's `error.rs` using `thiserror`
- Async code uses `tokio`; the UI thread uses `glib::spawn_future_local`
- GTK closures that capture owned values use `RefCell` wrapping (since `connect_*` is `Fn`, not `FnOnce`)

## Architecture Guidelines

- **raven-core must stay lightweight** - no heavy dependencies
- **No shared mutable state** between UI and backend threads
- **Backend crates must not depend on GTK** - keep UI-free for testability
- **New shared types** between backend and UI go in `raven-core`
- **All I/O in the backend** - the UI thread only renders and sends commands

## Testing Guidelines

- Write unit tests for all backend logic
- Use `tempfile::TempDir` for filesystem tests
- Mock traits where needed rather than hitting real services
- UI code is tested manually (GTK runtime required)
- Aim for tests that are fast and deterministic

## License

By contributing, you agree that your contributions will be licensed under GPL-3.0-or-later.
