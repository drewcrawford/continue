# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust library (edition 2024, requires rustc 1.85.1+) implementing a Swift-style continuation API for asynchronous programming. It provides a single-use channel implementation where a sender sends exactly one value to a receiver (Future).

## Key Commands

### Development
- `cargo build` - Build the library
- `cargo test` - Run all tests (unit and integration)
- `cargo doc --open` - Generate and open documentation in browser

### WebAssembly Testing
- `./scripts/wasm32/tests` - Run WASM tests via the `wasm_lite` runner
  - Uses nightly Rust and the `wasm_lite` binary on `PATH` (`cargo install wasm_lite_cli`)
  - Enables atomics, bulk-memory, and mutable-globals features
  - Rebuilds std with `-Z build-std` (see `.cargo/config.toml`)
- `./scripts/wasm32/check`, `./scripts/wasm32/clippy`, `./scripts/wasm32/docs` - Other wasm32 CI checks

### Testing Specific Components
- `cargo test --test hung_sender` - Run integration test for dropped futures
- `cargo test --lib` - Run only unit tests in src/lib.rs
- `cargo test sync` - Run tests matching "sync"

### Linting and Checks
- `cargo clippy` - Run Rust linter
- `cargo fmt` - Format code
- `cargo check` - Quick type check without building

## Architecture

The library consists of two main modules:

1. **Core API (src/lib.rs)**:
   - `continuation()` - Creates a simple sender/future pair
   - `continuation_cancel()` - Creates a cancellable sender/future pair
   - State machine: Empty → Data → Gone (or FutureHangup if dropped early)
   - Uses `Arc<Shared<R>>` for thread-safe shared state

2. **Sync Wrappers (src/sync.rs)**:
   - `SyncSender`, `SyncFuture`, `SyncFutureCancel` - Thread-safe wrappers
   - Adds `Sync` bounds via `&mut self` access requirements

Key design principles:
- Dropping a sender without sending panics (programmer error)
- Future can be safely dropped at any time
- Types are `Send` but not `Sync` by default; use `sync` module wrappers when `Sync` is needed
- Supports custom cancellation handlers via `FutureCancellation` trait

## Testing Notes

- Async tests are driven by small local helpers in `src/lib.rs`'s test module (`poll_once` with `Waker::noop()`, a parking `block_on`)
- WASM tests use `wasm_lite_std` (a wasm32-only dev-dependency from crates.io) for threads, channels, and `async_doctest!`, with `#[wasm_lite::wasm_lite_test]` as the test attribute
- Tests conditionally compile for WASM via `#[cfg(target_arch = "wasm32")]`