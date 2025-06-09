use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use r#continue::continuation;

// This test verifies that calling send on a Sender with a hung-up future returns promptly,
// preventing an infinite loop.
#[test]
fn sender_send_returns_on_future_hangup() {
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
    rx.recv_timeout(Duration::from_secs(1)).expect("sender.send hung, bug may not be fixed");
}