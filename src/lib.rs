//SPDX-License-Identifier: MIT OR Apache-2.0
/*!
# continue

![logo](art/logo.png)

continue is a Rust implementation of a Swift-style continuation API.

# For those more familiar with Rust

A continuation is a type of single-use channel.  The sender side of the channel sends a value.  The receiver of the channel is a `Future`, that `Output`s said value.  (It is a programmer error to drop a sender before sending).

A common usecase for the continuation type is when you need to implement a custom Future based on being signaled by an external source when the `Future` is complete.

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
use std::fmt::{Debug};
use std::mem::{ManuallyDrop, MaybeUninit};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Context, Poll};

#[repr(u8)]
enum State {
    Empty,
    Data,
    Gone,
    FutureHangup,
}

#[derive(Debug)]
struct Shared<R> {
    data: UnsafeCell<MaybeUninit<R>>,
    state: AtomicU8,
    waker: atomic_waker::AtomicWaker,
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
pub struct FutureCancel<R,C: FutureCancellation> {
    future: ManuallyDrop<Future<R>>,
    cancellation: C,
}

pub trait FutureCancellation {
    fn cancel(&mut self);
}


/**
Creates a new continuation.

If you need to provide a custom cancel implementation, use [continuation_cancel] instead.
*/
pub fn continuation<R>() -> (Sender<R>,Future<R>) {
    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
    });
    (Sender { shared: shared.clone(), sent: false }, Future { shared })
}

/**
Creates a new continuation.  Allows for a custom cancel implementation.

# Parameters
- `cancellation` - The cancellation implementation to use.  You can use the [crate::FutureCancellation] trait to react to cancel events, or Drop to react to drop events
  (regardless of whether the future is cancelled).
*/
pub fn continuation_cancel<R,C: FutureCancellation>(cancellation: C) -> (Sender<R>,FutureCancel<R,C>) {
    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
    });
    (Sender { shared: shared.clone(), sent: false }, FutureCancel { future: ManuallyDrop::new(Future { shared }), cancellation })
}




impl<R> Sender<R> {
    /**
    Sends the data to the remote side.

    Note that there is no particular guarantee that the remote side will receive this.  For example,
    the remote side may be dropped already, in which case sending has no effect.  Alternatively, the remote
    side may become dropped after sending.

    If you have a particularly good way of handling this, you may want to check [Self::is_cancelled] to avoid doing unnecessary work.
    Note that this is not perfect either (since the remote side may be dropped after the check but before the send).
*/
    pub fn send(mut self, data: R)  {
        self.sent = true;

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
            let swap = self.shared.state.compare_exchange_weak(State::Empty as u8, State::Data as u8, Ordering::Release, Ordering::Relaxed);
            match swap {
                Ok(_) => {
                    self.shared.waker.wake();
                    return
                }
                Err(u) => {
                    match u {
                        u if u == State::Empty as u8 => {/* spurious, go around again */}
                        u if u == State::Data as u8 || u == State::Gone as u8 => {unreachable!("Continuation already resumed")}
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
    */
    pub fn is_cancelled(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == State::FutureHangup as u8
    }

}

impl<R> Drop for Sender<R> {
    fn drop(&mut self) {
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
        let swap = self.shared.state.swap(State::FutureHangup as u8, Ordering::Acquire);
        match swap {
            u if u == State::Empty as u8 => {
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
            u if u == State::Gone as u8 => {
                DropState::NotCancelled
            }
            _ => unreachable!("Invalid state"),
        }
    }
}

impl<R> Drop for Future<R> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

impl<R,C: FutureCancellation> Drop for FutureCancel<R,C> {
    fn drop(&mut self) {
        //kill future first
        let mut future = unsafe{ManuallyDrop::take(&mut self.future)};
        match future.drop_impl() {
            DropState::Cancelled => {
                self.cancellation.cancel();
            }
            DropState::NotCancelled => {}
        }
        //don't run drop - we already ran drop_impl
        std::mem::forget(future);
    }
}

enum ReadStatus<R> {
    Data(R),
    Waiting,
    Spurious,
}

impl<R> Future<R> {
    fn interpret_result(result: Result<u8, u8>, data: &UnsafeCell<MaybeUninit<R>>) -> ReadStatus<R> {
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
            Err(u) => {
                match u {
                    u if u == State::Empty as u8 => { ReadStatus::Waiting }
                    u if u == State::Data as u8 => { ReadStatus::Spurious }
                    u if u == State::Gone as u8 => { panic!("Continuation already polled") }
                    _ => { unreachable!("Invalid state") }
                }
            }
        }
    }
}



impl<R> std::future::Future for Future<R> {
    type Output = R;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        //optimistic read.
        let state = self.shared.state.compare_exchange_weak(State::Data as u8, State::Gone as u8, Ordering::Acquire, Ordering::Relaxed);
        match Self::interpret_result(state, &self.shared.data) {
            ReadStatus::Data(data) => {return Poll::Ready(data)}
            ReadStatus::Waiting | ReadStatus::Spurious => {}
        }
        //register for wakeup
        self.shared.waker.register(cx.waker());
        loop {
            let state2 = self.shared.state.compare_exchange_weak(State::Data as u8, State::Gone as u8, Ordering::Acquire, Ordering::Relaxed);
            match Self::interpret_result(state2, &self.shared.data) {
                ReadStatus::Data(data) => {return Poll::Ready(data)}
                ReadStatus::Waiting => {return Poll::Pending}
                ReadStatus::Spurious => {continue}
            }
        }
    }
}

impl<R,C: FutureCancellation> std::future::Future for FutureCancel<R,C> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        //nothing is unpinned here
       unsafe{self.map_unchecked_mut(|s| &mut s.future as &mut Future<R> )}.poll(cx)
    }
}

//tedious traits

//I think we don't want clone on either type, because it creates problems for implementing Send.
unsafe impl<R: Send> Send for Future<R> {}
unsafe impl<R: Send, C: Send + FutureCancellation> Send for FutureCancel<R,C> {}
unsafe impl <R: Send> Send for Sender<R> {}
/*
Implementing this allows us to appear in sync types.

I think this is ok because:
1.  The main API is a self method, so we don't particularly care about &self situations
2.  The one &self method is `is_cancelled`, which only acceses the underlying boolean.
 */
unsafe impl <R: Sync> Sync for Sender<R> {}

/*Since no clone, no copy

I think we don't want Eq/Ord/hash because we don't expect multiple instances, since no clone.

Default does not make a lot of sense because we generate types as a pair.
 */




#[cfg(test)]
mod test {
    use std::pin::Pin;
    use std::task::Poll;
    use crate::continuation;
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn test_continue() {
        let(c,mut f) = continuation();
        let mut f = Pin::new(&mut f);
        assert!(test_executors::poll_once(f.as_mut()).is_pending());
        c.send(23);
        match test_executors::poll_once(f) {
            Poll::Ready(23) => {}
            x => panic!("Unexpected result {:?}",x),
        }
    }

    #[test] fn test_is_send() {
        fn is_send<T: Send>() {}
        is_send::<crate::Future<i32>>();
        is_send::<crate::Sender<i32>>();
    }

    #[test_executors::async_test] async fn test_stress() {
        #[cfg(target_arch = "wasm32")]
        use wasm_thread as thread;
        #[cfg(not(target_arch = "wasm32"))]
        use std::thread as thread;

        let mut senders = Vec::new();
        let mut futs = Vec::new();
        #[allow(dead_code)]
        #[derive(Debug)]
        struct Ex {
            a: u64,
            b: u64
        }
        impl Ex { fn new() -> Ex { Ex { a: 0, b: 0 } }}
        for _ in 0..1000 {
            let (s,f) = continuation();
            senders.push(s);
            futs.push(f);
        }
        let (overall_send,overall_fut) = continuation();
        thread::spawn(|| {
            test_executors::spawn_local(async move {
               for (_f,fut) in futs.drain(..).enumerate() {
                   let r = fut.await;
                   println!("{:?}", r);
               }
               overall_send.send(());
            }, "test_stress");
        });
        for sender in senders.drain(..) {
            sender.send(Ex::new());
        }
        overall_fut.await;
    }


}