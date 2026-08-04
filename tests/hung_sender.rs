// SPDX-License-Identifier: MIT OR Apache-2.0
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use r#continue::continuation;

// This test verifies that calling send on a Sender with a hung-up future returns promptly,
// preventing an infinite loop.
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_lite::wasm_lite_test)]
fn sender_send_returns_on_future_hangup() {
    #[cfg(not(target_arch = "wasm32"))]
    sender_send_returns_on_future_hangup_native();

    #[cfg(target_arch = "wasm32")]
    wasm_lite_std::async_doctest!(async {
        sender_send_returns_on_future_hangup_wasm().await;
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn sender_send_returns_on_future_hangup_native() {
    // Create a new continuation with unit type payload
    let (sender, future) = continuation::<()>();

    // Drop the future to simulate a hung-up receiver
    drop(future);

    // Use a channel to signal completion from a spawned thread
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        // This call should return quickly without hanging even though the future is gone
        sender.send(());
        // Signal that send has completed
        tx.send(()).expect("failed to send completion signal");
    });

    // Wait for up to 1 second for the send to complete. If it times out, the bug is likely not fixed.
    rx.recv_timeout(Duration::from_secs(1))
        .expect("sender.send hung, bug may not be fixed");
}

#[cfg(target_arch = "wasm32")]
async fn sender_send_returns_on_future_hangup_wasm() {
    use wasm_lite_std::mpsc;
    use wasm_lite_std::time::{Duration, Instant};

    let (sender, future) = continuation::<()>();
    drop(future);

    let (tx, mut rx) = mpsc::channel();
    wasm_lite_std::spawn(move || {
        sender.send(());
        tx.send_sync(()).expect("failed to send completion signal");
    });

    // Generous deadline: this must cover worker-thread startup, not just send().
    let deadline = Instant::now() + Duration::from_secs(5);
    rx.recv_async_timeout(deadline)
        .await
        .expect("sender.send hung, bug may not be fixed");
}
