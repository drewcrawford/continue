// SPDX-License-Identifier: MIT OR Apache-2.0

use std::future::Future as StdFuture;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

fn poll_once<F: StdFuture>(future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.poll(&mut context)
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_lite::wasm_lite_test)]
fn continuation_remains_a_noop_without_a_dispatcher() {
    assert!(logwise::context::capture().is_none());
    let (sender, mut future) = r#continue::continuation();
    assert!(poll_once(Pin::new(&mut future)).is_pending());
    sender.send(7_u8);
    assert_eq!(poll_once(Pin::new(&mut future)), Poll::Ready(7));
    assert!(logwise::context::capture().is_none());
}
