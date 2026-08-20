// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit tests for the continuation primitive.

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
