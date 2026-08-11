// SPDX-License-Identifier: MIT OR Apache-2.0
#![cfg(not(target_arch = "wasm32"))]

use r#continue::sync::SyncFutureCancel;
use r#continue::{FutureCancellation, continuation_cancel};
use std::cell::Cell;
use std::fmt;
use std::future::Future as _;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::task::{Context, Poll, Wake, Waker};

struct NoopCancellation;

impl FutureCancellation for NoopCancellation {
    fn cancel(&mut self) {}
}

struct TrackingWaker {
    _retained: Arc<()>,
}

#[allow(clippy::manual_noop_waker)] // The retained Arc is the resource this test tracks.
impl Wake for TrackingWaker {
    fn wake(self: Arc<Self>) {}
}

#[test]
fn dropping_cancel_future_releases_registered_waker() {
    let retained_by_waker = Arc::new(());
    let waker = Waker::from(Arc::new(TrackingWaker {
        _retained: Arc::clone(&retained_by_waker),
    }));
    let (sender, mut future) = continuation_cancel::<(), _>(NoopCancellation);

    let mut context = Context::from_waker(&waker);
    assert_eq!(Pin::new(&mut future).poll(&mut context), Poll::Pending);
    drop(waker);

    drop(future);
    sender.send(());

    assert_eq!(
        Arc::strong_count(&retained_by_waker),
        1,
        "dropping FutureCancel must release Shared and its registered waker"
    );
}

struct ConcurrentDebugCancellation {
    rendezvous: Arc<Barrier>,
    active_calls: Arc<AtomicUsize>,
    _not_sync: Cell<()>,
}

impl FutureCancellation for ConcurrentDebugCancellation {
    fn cancel(&mut self) {}
}

impl fmt::Debug for ConcurrentDebugCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.active_calls.fetch_add(1, Ordering::SeqCst);
        self.rendezvous.wait();

        assert_eq!(
            self.active_calls.load(Ordering::SeqCst),
            1,
            "Debug for a non-Sync cancellation handler was called concurrently"
        );

        formatter.write_str("ConcurrentDebugCancellation")
    }
}

#[test]
fn sync_cancel_future_does_not_share_non_sync_debug_implementation() {
    let cancellation = ConcurrentDebugCancellation {
        rendezvous: Arc::new(Barrier::new(2)),
        active_calls: Arc::new(AtomicUsize::new(0)),
        _not_sync: Cell::new(()),
    };
    let (sender, future) = continuation_cancel::<(), _>(cancellation);
    sender.send(());
    let future = Arc::new(SyncFutureCancel::new(future));

    let results = std::thread::scope(|scope| {
        let first = Arc::clone(&future);
        let second = Arc::clone(&future);
        let first = scope.spawn(move || format!("{first:?}"));
        let second = scope.spawn(move || format!("{second:?}"));
        [first.join(), second.join()]
    });

    assert!(
        results.into_iter().all(|result| result.is_ok()),
        "SyncFutureCancel exposed a non-Sync Debug implementation concurrently"
    );
}
