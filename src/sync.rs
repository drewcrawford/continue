//SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{Sender, Future, FutureCancel, FutureCancellation};
use std::pin::Pin;
use std::task::{Context, Poll};

/// A Sync wrapper around Sender that ensures thread-safe access
/// by requiring &mut self for all operations.
#[derive(Debug)]
pub struct SyncSender<R> {
    inner: Sender<R>,
}

impl<R> SyncSender<R> {
    /// Creates a new SyncSender wrapping the provided Sender
    pub fn new(sender: Sender<R>) -> Self {
        Self { inner: sender }
    }

    /// Sends the data to the remote side.
    /// 
    /// This method takes self to consume the wrapper,
    /// making the Sync implementation safe.
    pub fn send(self, data: R) {
        self.inner.send(data);
    }

    /// Determines if the underlying future is cancelled.
    /// 
    /// This method takes &mut self to ensure exclusive access,
    /// making the Sync implementation safe.
    pub fn is_cancelled(&mut self) -> bool {
        self.inner.is_cancelled()
    }
}

// Safety: SyncSender is safe to share between threads because:
// 1. send() consumes self, preventing multiple sends
// 2. is_cancelled() requires &mut self, ensuring exclusive access
// 3. The underlying Sender<R> is already Send when R: Send
// 4. No shared mutable state is accessible without &mut self
unsafe impl<R: Send> Sync for SyncSender<R> {}

// Re-export Send implementation
unsafe impl<R: Send> Send for SyncSender<R> {}

/// A Sync wrapper around Future that ensures thread-safe access
/// by requiring &mut self for all operations.
#[derive(Debug)]
pub struct SyncFuture<R> {
    inner: Future<R>,
}

impl<R> SyncFuture<R> {
    /// Creates a new SyncFuture wrapping the provided Future
    pub fn new(future: Future<R>) -> Self {
        Self { inner: future }
    }
}

impl<R> std::future::Future for SyncFuture<R> {
    type Output = R;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: We're not moving the inner future, just getting a mutable reference to it
        let inner = unsafe { self.as_mut().map_unchecked_mut(|s| &mut s.inner) };
        inner.poll(cx)
    }
}

// Safety: SyncFuture is safe to share between threads because:
// 1. All access to the inner Future requires &mut self through poll()
// 2. The underlying Future<R> is already Send when R: Send
// 3. No shared mutable state is accessible without &mut self
unsafe impl<R: Send> Sync for SyncFuture<R> {}

// Re-export Send implementation
unsafe impl<R: Send> Send for SyncFuture<R> {}

/// A Sync wrapper around FutureCancel that ensures thread-safe access
/// by requiring &mut self for all operations.
#[derive(Debug)]
pub struct SyncFutureCancel<R, C: FutureCancellation> {
    inner: FutureCancel<R, C>,
}

impl<R, C: FutureCancellation> SyncFutureCancel<R, C> {
    /// Creates a new SyncFutureCancel wrapping the provided FutureCancel
    pub fn new(future: FutureCancel<R, C>) -> Self {
        Self { inner: future }
    }
}

impl<R, C: FutureCancellation> std::future::Future for SyncFutureCancel<R, C> {
    type Output = R;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: We're not moving the inner future, just getting a mutable reference to it
        let inner = unsafe { self.as_mut().map_unchecked_mut(|s| &mut s.inner) };
        inner.poll(cx)
    }
}

// Safety: SyncFutureCancel is safe to share between threads because:
// 1. All access to the inner FutureCancel requires &mut self through poll()
// 2. The underlying FutureCancel<R, C> is already Send when R: Send and C: Send
// 3. No shared mutable state is accessible without &mut self
unsafe impl<R: Send, C: Send + FutureCancellation> Sync for SyncFutureCancel<R, C> {}

// Re-export Send implementation
unsafe impl<R: Send, C: Send + FutureCancellation> Send for SyncFutureCancel<R, C> {}