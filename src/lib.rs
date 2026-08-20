// SPDX-License-Identifier: MIT OR Apache-2.0
/*!
# continue

![logo](https://github.com/drewcrawford/continue/raw/main/art/logo.png)

continue is a Rust implementation of a Swift-style continuation API.

# For those more familiar with Rust

A continuation is a type of single-use channel.  The sender side of the channel sends a value.  The receiver of the channel is a `Future`, that `Output`s said value.  (It is a programmer error to drop a sender before sending).

A common usecase for the continuation type is when you need to implement a custom Future based on being signaled by an external source when the `Future` is complete.

# Thread Safety

By default, continuation types ([`Sender`], [`Future`], [`FutureCancel`]) are `Send` but not `Sync`. If you need to share these types across threads in data structures that require `Sync` (like storing them in an `Arc`), use the thread-safe wrappers provided in the [`sync`] module.

# Observability context

A continuation owns a small `logwise` context token whose parent is the context
active when the continuation is created. Polling, completion, and cancellation
enter that token only for the duration of their own work, then restore the
caller's thread or worker context. A completion performed elsewhere links the
completion-site context to the continuation instead of replacing its parent.

The `logwise-forensic` feature compiles stable completion, resume, and
cancellation events for diagnosing hangs. It is off by default. There is no
runtime dependency: without a dispatcher, tokens and instrumentation are
no-ops.

# For those more familiar with Swift

continue is the answer to how to do [`withCheckedContinuation`](https://developer.apple.com/documentation/swift/withcheckedcontinuation(isolation:function:_:)), [`CheckedContinuation`](https://developer.apple.com/documentation/swift/checkedcontinuation), and related APIs when in Rust.

# For those entirely *too* familiar with Rust

You may well ask: why use this?  I can 'simply' write my output into the future's memory, signal the waker, and be done with it.

Not quite.  First, because wakers implicitly have a short lifetime (until the next poll, e.g. you must re-register wakers on each poll), you need some way to
smuggle this value across threads.  The usual hammer for this nail is [`atomic-waker`](https://crates.io/crates/atomic-waker), which it will not surprise you to learn is a dependency.

Secondly Drop is surprisingly hard.  In Rust, the Future side can be dropped early.  In which case: a) are you writing to a sound memory location, b) will you `Drop` the right number of times regardless of how Dropped or in-the-process-of-being-Dropped the future side is,  c) Did you want to run some code on Drop to cancel in-flight tasks, d) Did you want to optimistically poll the cancellation state and how will you smuggle that across, etc.

Thirdly, executors are surprisingly hard.  It would be *sound* for an executor to keep polling you forever after it has a result, is your implementation sound in that case? Across `Drop` and `!Clone` types?

I found myself making too many mistakes, in too many places, and so I've decided to make them all in one place: right here!


*/

use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Context, Poll};

pub mod sync;

#[cfg(feature = "exfiltrate")]
pub mod exfiltrate_provider;
#[cfg(feature = "exfiltrate")]
pub mod registry;

/// Internal state of a continuation
#[repr(u8)]
enum State {
    /// No data has been sent yet
    Empty,
    /// Data has been sent and is available
    Data,
    /// Data has been consumed by the Future
    Gone,
    /// The Future has been dropped before receiving data
    FutureHangup,
}

/// Shared state between Sender and Future
#[derive(Debug)]
struct Shared<R> {
    /// The data to be sent, wrapped in UnsafeCell for interior mutability
    data: UnsafeCell<MaybeUninit<R>>,
    /// The current state of the continuation
    state: AtomicU8,
    /// Waker to notify the Future when data is available
    waker: atomic_waker::AtomicWaker,
    /// Durable causal identity created beneath the creator/awaiter context.
    continuation_context: logwise::ContextToken,
    /// This continuation's row in the debug registry. See `registry`.
    #[cfg(feature = "exfiltrate")]
    registry_id: u64,
}

/**
The Sender-side of the continuation.

It is a programmer error (panic) to drop this type without sending a value.
*/
#[derive(Debug)]
pub struct Sender<R> {
    shared: Arc<Shared<R>>,
    sent: bool,
}

/**
The receive side of a continuation, with cancellation support.
*/
#[derive(Debug)]
pub struct FutureCancel<R, C: FutureCancellation> {
    future: ManuallyDrop<Future<R>>,
    cancellation: C,
}

/// Trait for handling cancellation of a Future.
///
/// This trait allows you to execute custom logic when a Future is dropped
/// before it receives its value.
///
/// # Example
///
/// ```
/// use r#continue::{continuation_cancel, FutureCancellation};
/// use std::sync::{Arc, Mutex};
///
/// struct CancelHandler {
///     cancelled: Arc<Mutex<bool>>,
/// }
///
/// impl FutureCancellation for CancelHandler {
///     fn cancel(&mut self) {
///         *self.cancelled.lock().unwrap() = true;
///         println!("Future was cancelled!");
///     }
/// }
///
/// let cancelled = Arc::new(Mutex::new(false));
/// let handler = CancelHandler { cancelled: cancelled.clone() };
///
/// let (sender, future) = continuation_cancel::<String, _>(handler);
///
/// // Drop the future without awaiting it
/// drop(future);
///
/// // The cancel handler was called
/// assert!(*cancelled.lock().unwrap());
///
/// // Sending to a cancelled future is a no-op
/// sender.send("This won't be received".to_string());
/// ```
pub trait FutureCancellation {
    /// Called when the future is dropped before receiving a value.
    fn cancel(&mut self);
}

/**
Creates a new continuation.

If you need to provide a custom cancel implementation, use [continuation_cancel] instead.

# Examples

Basic usage:
```
use r#continue::continuation;
# #[cfg(not(target_arch = "wasm32"))]
# use std::thread;
# #[cfg(target_arch = "wasm32")]
# use wasm_lite_std as thread;

let (sender, future) = continuation::<String>();

// Send data from another thread
thread::spawn(move || {
    sender.send("Hello from another thread!".to_string());
});

// Block on the future to get the result
# macro_rules! run_async {
#     ($future:expr) => {{
#         #[cfg(not(target_arch = "wasm32"))]
#         {
#             block_on($future);
#         }
#         #[cfg(target_arch = "wasm32")]
#         {
#             wasm_lite_std::async_doctest!($future);
#         }
#     }};
# }
# #[cfg(not(target_arch = "wasm32"))]
# fn block_on<F: std::future::Future>(future: F) -> F::Output {
#     use std::sync::Arc;
#     use std::task::{Context, Poll, Wake, Waker};
#     struct ThreadWaker(std::thread::Thread);
#     impl Wake for ThreadWaker {
#         fn wake(self: Arc<Self>) {
#             self.0.unpark();
#         }
#     }
#     let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
#     let mut context = Context::from_waker(&waker);
#     let mut future = std::pin::pin!(future);
#     loop {
#         match future.as_mut().poll(&mut context) {
#             Poll::Ready(value) => return value,
#             Poll::Pending => std::thread::park(),
#         }
#     }
# }
# run_async!(async {
let result = future.await;
assert_eq!(result, "Hello from another thread!");
# });
```

Using multiple continuations:
```
use r#continue::continuation;

// Create multiple continuations
let (tx1, rx1) = continuation::<i32>();
let (tx2, rx2) = continuation::<i32>();

// Send values
tx1.send(42);
tx2.send(100);

// Collect results
# macro_rules! run_async {
#     ($future:expr) => {{
#         #[cfg(not(target_arch = "wasm32"))]
#         {
#             block_on($future);
#         }
#         #[cfg(target_arch = "wasm32")]
#         {
#             wasm_lite_std::async_doctest!($future);
#         }
#     }};
# }
# #[cfg(not(target_arch = "wasm32"))]
# fn block_on<F: std::future::Future>(future: F) -> F::Output {
#     use std::sync::Arc;
#     use std::task::{Context, Poll, Wake, Waker};
#     struct ThreadWaker(std::thread::Thread);
#     impl Wake for ThreadWaker {
#         fn wake(self: Arc<Self>) {
#             self.0.unpark();
#         }
#     }
#     let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
#     let mut context = Context::from_waker(&waker);
#     let mut future = std::pin::pin!(future);
#     loop {
#         match future.as_mut().poll(&mut context) {
#             Poll::Ready(value) => return value,
#             Poll::Pending => std::thread::park(),
#         }
#     }
# }
# run_async!(async {
let sum = rx1.await + rx2.await;
assert_eq!(sum, 142);
# });
```
*/
#[cfg_attr(feature = "exfiltrate", track_caller)]
pub fn continuation<R>() -> (Sender<R>, Future<R>) {
    let continuation_context =
        logwise::context::child(logwise::context::capture(), "continue.continuation");
    // Recorded before the shared state is built, so the id it hands back can
    // live in it: the id is how every later transition finds its row.
    #[cfg(feature = "exfiltrate")]
    let registry_id = registry::record_created(
        continuation_context.into_parts().0,
        std::panic::Location::caller(),
    );
    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
        continuation_context,
        #[cfg(feature = "exfiltrate")]
        registry_id,
    });
    (
        Sender {
            shared: shared.clone(),
            sent: false,
        },
        Future { shared },
    )
}

/**
Creates a new continuation with a custom cancellation handler.

# Parameters
- `cancellation` - The cancellation implementation to use. You can use the [FutureCancellation] trait to react to cancel events, or Drop to react to drop events
  (regardless of whether the future is cancelled).

# Examples

```
use r#continue::{continuation_cancel, FutureCancellation};
# #[cfg(not(target_arch = "wasm32"))]
# use std::thread;
# #[cfg(target_arch = "wasm32")]
# use wasm_lite_std as thread;

struct CleanupHandler {
    resource_id: u32,
}

impl FutureCancellation for CleanupHandler {
    fn cancel(&mut self) {
        println!("Cleaning up resource {}", self.resource_id);
        // Perform cleanup operations here
    }
}

let handler = CleanupHandler { resource_id: 42 };
let (sender, future) = continuation_cancel::<String, _>(handler);

// If the future is dropped without receiving data, the handler will be called
thread::spawn(move || {
    thread::sleep(std::time::Duration::from_millis(100));
    sender.send("Data".to_string());
});

// Drop the future early to trigger cancellation
drop(future);
// Output: "Cleaning up resource 42"
```

# Example with async task cancellation

```
use r#continue::{continuation_cancel, FutureCancellation};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
# #[cfg(not(target_arch = "wasm32"))]
# use std::thread;
# #[cfg(target_arch = "wasm32")]
# use wasm_lite_std as thread;

struct TaskCanceller {
    should_stop: Arc<AtomicBool>,
}

impl FutureCancellation for TaskCanceller {
    fn cancel(&mut self) {
        self.should_stop.store(true, Ordering::Relaxed);
    }
}

let should_stop = Arc::new(AtomicBool::new(false));
let canceller = TaskCanceller { should_stop: should_stop.clone() };

let (sender, future) = continuation_cancel::<i32, _>(canceller);
sender.send(0); //avoid crash
// Start a background task that checks the cancellation flag
thread::spawn(move || {
    let mut counter = 0;
    while !should_stop.load(Ordering::Relaxed) {
        counter += 1;
        thread::sleep(std::time::Duration::from_millis(10));
    }
    // Task sees cancellation and can clean up
    println!("Task cancelled after {} iterations", counter);
});

// Later, drop the future to signal cancellation
drop(future);
```
*/
pub fn continuation_cancel<R, C: FutureCancellation>(
    cancellation: C,
) -> (Sender<R>, FutureCancel<R, C>) {
    let continuation_context =
        logwise::context::child(logwise::context::capture(), "continue.continuation");
    // Recorded before the shared state is built, so the id it hands back can
    // live in it: the id is how every later transition finds its row.
    #[cfg(feature = "exfiltrate")]
    let registry_id = registry::record_created(
        continuation_context.into_parts().0,
        std::panic::Location::caller(),
    );
    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
        continuation_context,
        #[cfg(feature = "exfiltrate")]
        registry_id,
    });
    (
        Sender {
            shared: shared.clone(),
            sent: false,
        },
        FutureCancel {
            future: ManuallyDrop::new(Future { shared }),
            cancellation,
        },
    )
}

impl<R> Sender<R> {
    /**
        Sends the data to the remote side.

        Note that there is no particular guarantee that the remote side will receive this.  For example,
        the remote side may be dropped already, in which case sending has no effect.  Alternatively, the remote
        side may become dropped after sending.

        If you have a particularly good way of handling this, you may want to check [Self::is_cancelled] to avoid doing unnecessary work.
        Note that this is not perfect either (since the remote side may be dropped after the check but before the send).

        # Examples

        Basic usage:
        ```
        use r#continue::continuation;

        let (sender, future) = continuation::<i32>();

        // Send a value
        sender.send(42);

        # macro_rules! run_async {
        #     ($future:expr) => {{
        #         #[cfg(not(target_arch = "wasm32"))]
        #         {
        #             block_on($future);
        #         }
        #         #[cfg(target_arch = "wasm32")]
        #         {
        #             wasm_lite_std::async_doctest!($future);
        #         }
        #     }};
        # }
        # #[cfg(not(target_arch = "wasm32"))]
        # fn block_on<F: std::future::Future>(future: F) -> F::Output {
        #     use std::sync::Arc;
        #     use std::task::{Context, Poll, Wake, Waker};
        #     struct ThreadWaker(std::thread::Thread);
        #     impl Wake for ThreadWaker {
        #         fn wake(self: Arc<Self>) {
        #             self.0.unpark();
        #         }
        #     }
        #     let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        #     let mut context = Context::from_waker(&waker);
        #     let mut future = std::pin::pin!(future);
        #     loop {
        #         match future.as_mut().poll(&mut context) {
        #             Poll::Ready(value) => return value,
        #             Poll::Pending => std::thread::park(),
        #         }
        #     }
        # }
        # run_async!(async {
        assert_eq!(future.await, 42);
        # });
        ```

        Sending to a dropped future:
        ```
        use r#continue::continuation;

        let (sender, future) = continuation::<String>();

        // Drop the future
        drop(future);

        // Sending still completes without panic, but the value is discarded
        sender.send("This won't be received".to_string());
        ```

        # Panics

        It is a programmer error to drop a Sender without calling send:
        ```should_panic
        use r#continue::continuation;

        let (sender, _future) = continuation::<i32>();

        // This will panic!
        drop(sender);
        ```
    */
    pub fn send(mut self, data: R) {
        self.sent = true;

        let completion_context = logwise::context::capture();

        /*
        Safety: Data can only be written by this type. Since the type is unclonable, we're
        the only possible writer.

        It should be ok to write by default.
        */
        unsafe {
            let opt = &mut *self.shared.data.get();
            std::ptr::write(opt.as_mut_ptr(), data); //data is moved here!
        }
        loop {
            let swap = self.shared.state.compare_exchange_weak(
                State::Empty as u8,
                State::Data as u8,
                Ordering::Release,
                Ordering::Relaxed,
            );
            match swap {
                Ok(_) => {
                    if !completion_context.is_none() {
                        logwise::context::link(
                            self.shared.continuation_context,
                            completion_context,
                        );
                    }
                    let _continuation_context =
                        logwise::context::enter(self.shared.continuation_context);
                    #[cfg(feature = "logwise-forensic")]
                    logwise::forensic!("continue.continuation.completed");
                    self.shared.waker.wake();
                    return;
                }
                Err(u) => {
                    match u {
                        u if u == State::Empty as u8 => { /* spurious, go around again */ }
                        u if u == State::Data as u8 || u == State::Gone as u8 => {
                            unreachable!("Continuation already resumed")
                        }
                        u if u == State::FutureHangup as u8 => {
                            //sending to a hungup continuation is a no-op
                            //however, we did write our data, so we need to drop it and return
                            unsafe {
                                //safety: We know that the continuation has been resumed, so we can read the data
                                let data = &mut *self.shared.data.get();
                                //safety: we know the data was initialized and will never be written to again (only
                                //written to in empty state).
                                let _ = data.assume_init_read();
                            }
                            return;
                        }
                        //sender hangup is impossible
                        _ => unreachable!("Invalid state"),
                    }
                }
            }
        }
    }

    /**
    Determines if the underlying future is cancelled.  And thus, that sending data will have no effect.

    Even if this function returns `false`, it is possible that by the time you send data, the future will be cancelled.

    # Examples

    ```
    use r#continue::continuation;

    let (sender, future) = continuation::<String>();

    // Initially not cancelled
    assert!(!sender.is_cancelled());

    // Drop the future
    drop(future);

    // Now the sender reports cancelled
    assert!(sender.is_cancelled());

    // Can still send without panic, but it has no effect
    sender.send("Data".to_string());
    ```

    Using is_cancelled to avoid expensive computation:
    ```
    use r#continue::continuation;
    # #[cfg(not(target_arch = "wasm32"))]
    # use std::thread;
    # #[cfg(target_arch = "wasm32")]
    # use wasm_lite_std as thread;

    fn expensive_computation() -> String {
        // Simulate expensive work
        thread::sleep(std::time::Duration::from_millis(100));
        "Expensive result".to_string()
    }

    let (sender, future) = continuation::<String>();

    // Drop the future
    drop(future);

    // Check before doing expensive work
    if !sender.is_cancelled() {
        let result = expensive_computation();
        sender.send(result);
    } else {
        println!("Skipping expensive computation - future already cancelled");
        sender.send("Default".to_string()); // Still need to send to avoid panic
    }
    ```
    */
    pub fn is_cancelled(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == State::FutureHangup as u8
    }
}

impl<R> Drop for Sender<R> {
    fn drop(&mut self) {
        #[cfg(feature = "exfiltrate")]
        if !self.sent {
            // Recorded *before* the assert. Dropping a sender unsent is defined
            // as a programmer error here, and it otherwise shows up only as a
            // hang somewhere else entirely -- so the record is the useful part
            // even when the panic follows.
            registry::record_outcome(
                self.shared.registry_id,
                registry::Outcome::SenderDroppedUnsent,
            );
        }
        if !std::thread::panicking() {
            assert!(self.sent, "Sender dropped without sending data");
        }
    }
}

/**
The receive side of the continuation, without cancellation support.

See also: [FutureCancel]
*/
#[derive(Debug)]
pub struct Future<R> {
    shared: Arc<Shared<R>>,
}

enum DropState {
    Cancelled,
    NotCancelled,
}
impl<R> Future<R> {
    /**
    implementation detail of drop.

    # Returns
    a value indicating whether, at the time the function ran, the future is dropped before receiving data.

    Note that this is not a guarantee that at any future time – including immediately after this function returns – the data will not be sent.
    */
    fn drop_impl(&mut self) -> DropState {
        let _continuation_context = logwise::context::enter(self.shared.continuation_context);
        #[cfg(feature = "exfiltrate")]
        registry::record_outcome(self.shared.registry_id, registry::Outcome::FutureHungUp);
        let swap = self
            .shared
            .state
            .swap(State::FutureHangup as u8, Ordering::Acquire);
        match swap {
            u if u == State::Empty as u8 => {
                #[cfg(feature = "logwise-forensic")]
                logwise::forensic!("continue.continuation.cancelled");
                DropState::Cancelled
            }
            u if u == State::Data as u8 => {
                //data needs to be dropped here
                unsafe {
                    //safety: We know that the continuation has been resumed, so we can read the data
                    let data = &mut *self.shared.data.get();
                    //safety: we know the data was initialized and will never be written to again (only
                    //written to in empty state.
                    let _ = data.assume_init_read();
                }
                DropState::NotCancelled
            }
            u if u == State::Gone as u8 => DropState::NotCancelled,
            u if u == State::FutureHangup as u8 => {
                // FutureCancel can re-enter this path while unwinding from its callback.
                DropState::NotCancelled
            }
            _ => unreachable!("Invalid state"),
        }
    }

    /// Releases the fields of a [`Future`] after [`Self::drop_impl`] has already run.
    fn drop_after_drop_impl(self) {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            // Safety: `this` will not run `Future::drop`, and `shared` is its only field.
            // Dropping the field directly releases exactly this Future's Arc ownership.
            std::ptr::drop_in_place(&mut this.shared);
        }
    }
}

impl<R> Drop for Future<R> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

impl<R, C: FutureCancellation> Drop for FutureCancel<R, C> {
    fn drop(&mut self) {
        //kill future first
        let mut future = unsafe { ManuallyDrop::take(&mut self.future) };
        match future.drop_impl() {
            DropState::Cancelled => {
                self.cancellation.cancel();
            }
            DropState::NotCancelled => {}
        }
        // Don't run Future::drop - we already ran drop_impl - but still release its fields.
        future.drop_after_drop_impl();
    }
}

enum ReadStatus<R> {
    Data(R),
    Waiting,
    Spurious,
}

impl<R> Future<R> {
    fn interpret_result(
        result: Result<u8, u8>,
        data: &UnsafeCell<MaybeUninit<R>>,
    ) -> ReadStatus<R> {
        match result {
            Ok(..) => {
                unsafe {
                    //safety: We know that the continuation has been resumed, so we can read the data
                    let data = &mut *data.get();
                    /*safety: we know the data was initialized and will never be written to again (only
                    written to in empty state.

                    We know it will never be read again because we set gone before leaving the function.
                    It can only be polled exclusively in this function since we have &mut self.
                     */
                    let r = data.assume_init_read();
                    ReadStatus::Data(r)
                }
            }
            Err(u) => match u {
                u if u == State::Empty as u8 => ReadStatus::Waiting,
                u if u == State::Data as u8 => ReadStatus::Spurious,
                u if u == State::Gone as u8 => {
                    panic!("Continuation already polled")
                }
                _ => {
                    unreachable!("Invalid state")
                }
            },
        }
    }
}

impl<R> std::future::Future for Future<R> {
    type Output = R;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let _continuation_context = logwise::context::enter(self.shared.continuation_context);
        #[cfg(feature = "exfiltrate")]
        registry::record_polled(self.shared.registry_id);
        //optimistic read.
        let state = self.shared.state.compare_exchange_weak(
            State::Data as u8,
            State::Gone as u8,
            Ordering::Acquire,
            Ordering::Relaxed,
        );
        match Self::interpret_result(state, &self.shared.data) {
            ReadStatus::Data(data) => {
                #[cfg(feature = "logwise-forensic")]
                logwise::forensic!("continue.continuation.resumed");
                #[cfg(feature = "exfiltrate")]
                registry::record_outcome(self.shared.registry_id, registry::Outcome::Sent);
                return Poll::Ready(data);
            }
            ReadStatus::Waiting | ReadStatus::Spurious => {}
        }
        //register for wakeup
        self.shared.waker.register(cx.waker());
        loop {
            let state2 = self.shared.state.compare_exchange_weak(
                State::Data as u8,
                State::Gone as u8,
                Ordering::Acquire,
                Ordering::Relaxed,
            );
            match Self::interpret_result(state2, &self.shared.data) {
                ReadStatus::Data(data) => {
                    #[cfg(feature = "logwise-forensic")]
                    logwise::forensic!("continue.continuation.resumed");
                    return Poll::Ready(data);
                }
                ReadStatus::Waiting => return Poll::Pending,
                ReadStatus::Spurious => continue,
            }
        }
    }
}

impl<R, C: FutureCancellation> std::future::Future for FutureCancel<R, C> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        //nothing is unpinned here
        unsafe { self.map_unchecked_mut(|s| &mut s.future as &mut Future<R>) }.poll(cx)
    }
}

//tedious traits

//I think we don't want clone on either type, because it creates problems for implementing Send.
unsafe impl<R: Send> Send for Future<R> {}
unsafe impl<R: Send, C: Send + FutureCancellation> Send for FutureCancel<R, C> {}
unsafe impl<R: Send> Send for Sender<R> {}

/*Since no clone, no copy

I think we don't want Eq/Ord/hash because we don't expect multiple instances, since no clone.

Default does not make a lot of sense because we generate types as a pair.
 */

#[cfg(test)]
mod test {
    use crate::continuation;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        let mut context = Context::from_waker(Waker::noop());
        future.poll(&mut context)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::sync::Arc;
        use std::task::Wake;

        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }

        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_lite::wasm_lite_test)]
    fn test_continue() {
        let (c, mut f) = continuation();
        let mut f = Pin::new(&mut f);
        assert!(poll_once(f.as_mut()).is_pending());
        c.send(23);
        match poll_once(f) {
            Poll::Ready(23) => {}
            x => panic!("Unexpected result {:?}", x),
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_lite::wasm_lite_test)]
    fn test_is_send() {
        fn is_send<T: Send>() {}
        is_send::<crate::Future<i32>>();
        is_send::<crate::Sender<i32>>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_stress() {
        block_on(test_stress_body());
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_lite::wasm_lite_test]
    fn test_stress() {
        wasm_lite_std::async_doctest!(async {
            test_stress_body().await;
        });
    }

    async fn test_stress_body() {
        let mut senders = Vec::new();
        let mut futs = Vec::new();
        #[allow(dead_code)]
        #[derive(Debug)]
        struct Ex {
            a: u64,
            b: u64,
        }
        impl Ex {
            fn new() -> Ex {
                Ex { a: 0, b: 0 }
            }
        }
        for _ in 0..1000 {
            let (s, f) = continuation();
            senders.push(s);
            futs.push(f);
        }

        let (overall_send, overall_fut) = continuation::<()>();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(|| {
            block_on(async move {
                for fut in futs.drain(..) {
                    let r = fut.await;
                    println!("{:?}", r);
                }
                overall_send.send(());
            });
        });
        #[cfg(target_arch = "wasm32")]
        wasm_lite_std::spawn(|| {
            wasm_lite_std::spawn_local(async move {
                for fut in futs.drain(..) {
                    let r = fut.await;
                    println!("{:?}", r);
                }
                overall_send.send(());
            });
        });
        for sender in senders.drain(..) {
            sender.send(Ex::new());
        }
        overall_fut.await;
    }
}
