# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[Unreleased]: https://github.com/drewcrawford/continue/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/drewcrawford/continue/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/drewcrawford/continue/releases/tag/v0.1.1
