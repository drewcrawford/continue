# continue

![logo](art/logo.png)

**continue** is a single-use, single-value channel for async Rust — a Rust
implementation of Swift's continuation API.

You get a pair: a `Sender` and a `Future`. Whatever part of your program
eventually learns the answer — another thread, a C callback, a completion
handler, a JS promise on wasm — calls `send` exactly once. The `Future`
resolves with that value wherever you `.await` it. That's the whole job:
turning "someone will call me back later" into a well-behaved `Future`.

```rust
use r#continue::continuation;

let (sender, future) = continuation();

// Hand the sender to whatever will eventually produce the value...
thread::spawn(move || {
    sender.send("Hello from another thread!");
});

// ...and await it like any other future.
assert_eq!(future.await, "Hello from another thread!");
```

(The crate is named after a keyword, so in source it's `r#continue`.)

# The contract

The API is small because the rules are strict:

* **Exactly one value.** `Sender::send` consumes the sender. There is no
  second send.
* **Dropping a sender without sending panics.** Like Swift's *checked*
  continuations, a leaked continuation is treated as a programmer error and
  caught loudly, rather than leaving a future that silently never resolves.
* **The future may be dropped at any time.** That's always safe. A send to a
  dropped future is a no-op; the sender can call `Sender::is_cancelled`
  first to skip work that no longer has an audience (best-effort — the
  future can still be dropped between the check and the send).
* **Cancellation handlers are opt-in.** `continuation_cancel` takes a
  `FutureCancellation` implementation whose hook runs when the future side
  is dropped early — the place to abort the in-flight work that would have
  produced the value.

# Thread safety

By default the types (`Sender`, `Future`, `FutureCancel`) are `Send` but not
`Sync`. If you need to park them somewhere that requires `Sync` — say, inside
an `Arc`ed struct — the `sync` module provides `SyncSender`, `SyncFuture`,
and `SyncFutureCancel` wrappers that add the bound by requiring `&mut self`
access.

# For those more familiar with Swift

continue is the answer to how to do
[`withCheckedContinuation`](https://developer.apple.com/documentation/swift/withcheckedcontinuation(isolation:function:_:)),
[`CheckedContinuation`](https://developer.apple.com/documentation/swift/checkedcontinuation),
and related APIs when in Rust. For the multi-value analog
(`AsyncStream.Continuation`), see the sibling crate
[`continue_stream`](https://crates.io/crates/continue_stream).

# For those entirely *too* familiar with Rust

You may well ask: why use this? I can 'simply' write my output into the
future's memory, signal the waker, and be done with it.

Not quite. First, because wakers implicitly have a short lifetime (until the
next poll, e.g. you must re-register wakers on each poll), you need some way
to smuggle this value across threads. The usual hammer for this nail is
[`atomic-waker`](https://crates.io/crates/atomic-waker), which it will not
surprise you to learn is a dependency.

Secondly, Drop is surprisingly hard. In Rust, the Future side can be dropped
early. In which case: a) are you writing to a sound memory location, b) will
you `Drop` the right number of times regardless of how Dropped or
in-the-process-of-being-Dropped the future side is, c) did you want to run
some code on Drop to cancel in-flight tasks, d) did you want to
optimistically poll the cancellation state, and how will you smuggle that
across, etc.

Thirdly, executors are surprisingly hard. It would be *sound* for an executor
to keep polling you forever after it has a result — is your implementation
sound in that case? Across `Drop` and `!Clone` types?

I found myself making too many mistakes, in too many places, and so I've
decided to make them all in one place: right here!

# Compared to the alternatives

**vs. [`oneshot`](https://crates.io/crates/oneshot),
[`tokio::sync::oneshot`](https://docs.rs/tokio/latest/tokio/sync/oneshot/),
[`futures::channel::oneshot`](https://docs.rs/futures/latest/futures/channel/oneshot/)** —
same shape (a single-producer, single-consumer, single-message channel whose
send consumes the sender), different philosophy about the failure case. In
those channels, a sender dropped without sending surfaces as an error at the
*receiver*: the output is `Result<T, RecvError>` (or `Canceled`), so every
await site carries an error branch for a condition that is usually a bug.
continue takes Swift's *checked*-continuation stance: dropping a sender
without sending panics at the guilty drop site, where the backtrace names
the culprit — and in exchange, the future's `Output` is just `R`, with no
error case to propagate. continue also treats receiver-side cancellation as
a first-class event (a `FutureCancellation` hook that runs when the
future is dropped, plus `Sender::is_cancelled`) rather than something you
discover when a send fails. Reach for `oneshot` instead when a missing
value is a legitimate runtime outcome you want to handle, or when you need
its blocking/timeout *synchronous* receive — continue's receive side is
async only.

**vs. [`continue_stream`](https://crates.io/crates/continue_stream)** — the
sibling crate, for the many-values case: Swift's
`AsyncStream.Continuation` where this crate is `CheckedContinuation`. Its
channel is buffered, the sender can send repeatedly, and dropping the
sender ends the stream gracefully with `None` instead of panicking. If
the callback you are bridging fires once with one answer, use continue; if
it fires repeatedly, use continue_stream.

# Where it sits in the ecosystem

continue is a foundational primitive in the [sealedabstract crate
family](https://sealedabstract.com/code/) — near the bottom of the stack, so
most of the ecosystem's async machinery is built on it somewhere:

* [`some_executor`](https://crates.io/crates/some_executor) (the executor
  abstraction trait) and `some_global_executor` use it to wire task
  completion back to spawners.
* The channel crates ([`ampsc`](https://crates.io/crates/ampsc), `ampmc`,
  `aspmc`) and
  [`portable_async_sleep`](https://crates.io/crates/portable_async_sleep)
  use it as their wakeup primitive.
* Higher layers like [`app_window`](https://crates.io/crates/app_window)
  (windowing) and
  [`images_and_words`](https://crates.io/crates/images_and_words) (GPU
  middleware) use it to await OS callbacks and GPU completions.

It slots equally well into codebases outside that family: it has no opinion
about your executor, works with any runtime (or none — a hand-rolled
`block_on` is enough), and treats wasm32 as a first-class target, including
threaded wasm with atomics.

Its own dependencies are deliberately thin: `atomic-waker`, plus the
`logwise` logging facade for observability (a no-op unless a dispatcher is
installed).

# Observability

A continuation owns a small `logwise` context token whose parent is the
context active when the continuation is created. Polling, completion, and
cancellation enter that token only for the duration of their own work, then
restore the caller's thread or worker context. A completion performed
elsewhere links the completion-site context to the continuation instead of
replacing its parent.

Two opt-in features build on this (both off by default; the default build
stays a lightweight primitive):

* **`logwise-forensic`** compiles stable completion, resume, and cancellation
  events for diagnosing hangs. There is no runtime dependency: without a
  dispatcher, tokens and instrumentation are no-ops.
* **`exfiltrate`** maintains a bounded registry of outstanding continuations,
  queryable from outside the process via
  [`exfiltrate`](https://sealedabstract.com/code/)'s `snapshot` command. A
  continuation is a future waiting for something *outside* Rust to signal it;
  when that signal never comes, the program hangs with no stack to look at.
  The registry reports each continuation's creation site and age — and a
  continuation outstanding for 30 seconds in a program whose operations take
  milliseconds *is* the bug.

# Development

* `cargo build` / `cargo test` / `cargo clippy` / `cargo fmt` — the usual.
* `./scripts/wasm32/tests` runs the test suite on wasm32 via the `wasm_lite`
  runner (nightly Rust, `cargo install wasm_lite_cli`); `check`, `clippy`,
  and `docs` siblings cover the other wasm CI checks.

Requires Rust 1.95+ (edition 2024).

# License

MIT OR Apache-2.0.
