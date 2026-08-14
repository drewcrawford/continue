# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3] - 2026-08-14

Three soundness fixes around cancellable continuations, plus a new home for the WASM test toolchain.

### Changed
- Swapped the WASM test toolchain from `wasm-bindgen`/`wasm-bindgen-test`/`wasm_thread` to the `wasm_lite` runner and `wasm_lite_std` helpers
- `wasm_lite` and `wasm_lite_std` now come from crates.io rather than a sibling checkout, so the repo builds without any neighbors present. The test runner is the published `wasm_lite_cli` binary (`cargo install wasm_lite_cli`)
- Replaced the `test_executors` dev-dependency with small local async test helpers
- Retuned wasm32 rustflags for the new runner (bulk-memory/mutable-globals features, `__stack_pointer` and `__heap_base` exports, 1GB max memory)
- `Debug for SyncFutureCancel` no longer asks for `R: Debug` or `C: Debug`—it never printed either one, and now it doesn't pretend to need them
- The `sync` module finally explains itself: a module header on why the wrappers restore `Sync` by narrowing access rather than by locking, and what that costs you

### Fixed
- Prevented `SyncFutureCancel`'s `Debug` implementation from exposing a `Send + !Sync` cancellation handler through shared references, closing a safe-code data race
- Released `FutureCancel`'s shared state and registered waker on drop instead of quietly leaving them behind
- Made cancellation-handler panics unwind normally instead of triggering a second panic and aborting the process

## [0.1.2] - 2025-12-20

### Added
- New brute-force test coverage to catch those sneaky edge cases before they catch you
- SPDX license identifiers—because playing nicely with license scanners is the neighborly thing to do

### Changed
- Updated dependencies to their latest and greatest versions
- Freshened up the docs with clearer explanations
- CI configuration got a little tune-up to keep the build pipeline humming

### Removed
- Cleaned out some future patches that were no longer pulling their weight

### Fixed
- Squashed a few test gremlins that were causing flaky failures

## [0.1.1] - 2025-12-20

Initial release of `continue`, bringing Swift's elegant continuation API to Rust.

### Features
- Single-use channel implementation for sending exactly one value from sender to receiver
- Thread-safe async primitives with `Send` support
- Optional `Sync` wrappers in the `sync` module for multi-threaded scenarios
- Cancellable continuations with custom cancellation handlers
- WebAssembly support with proper threading via `wasm_thread`
- Zero-cost abstractions with Arc-based shared state management

### Safety Guarantees
- Panic on sender drop without sending (catches programmer errors early)
- Safe future dropping at any time
- No data races thanks to atomic state transitions

[Unreleased]: https://github.com/drewcrawford/continue/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/drewcrawford/continue/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/drewcrawford/continue/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/drewcrawford/continue/releases/tag/v0.1.1
