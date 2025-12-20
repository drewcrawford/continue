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
- `./wasmtest.sh` - Run WASM tests in Safari with threading support
  - Uses nightly Rust with `wasm-pack test --safari`
  - Enables atomics, bulk-memory, and mutable-globals features
  - Rebuilds std with `-Z build-std=panic_abort,std`

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

- Uses `test_executors` crate for async test utilities (`sleep_on`, `poll_once`, `spawn_local`)
- WASM tests use `wasm_thread` for spawning threads and `wasm_bindgen_test` for browser testing
- Tests conditionally compile for WASM via `#[cfg(target_arch = "wasm32")]`