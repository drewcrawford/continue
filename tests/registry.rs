// SPDX-License-Identifier: MIT OR Apache-2.0

//! The continuation registry, end to end through exfiltrate's `snapshot`.
//!
//! Its own binary on purpose. The registry is process-global, and the crate's
//! unit tests create continuations of their own — `test_stress` creates
//! thousands — so anything asserting on what the registry holds is not
//! deterministic in that process. Here nothing else writes to it.

use r#continue::continuation;
use exfiltrate_internal::command::{Command, Response};
use exfiltrate_internal::snapshot::{SnapshotRow, SnapshotSet, SnapshotValue};

fn ask(args: &[&str]) -> SnapshotSet {
    r#continue::exfiltrate_provider::install();
    let response = exfiltrate::provider::Snapshot
        .execute(args.iter().map(|arg| arg.to_string()).collect())
        .expect("snapshot should not fail");
    match response {
        Response::Bytes(bytes) => rmp_serde::from_slice(&bytes).expect("decode"),
        other => panic!("expected a structured response, got {other:?}"),
    }
}

/// The row for the continuation created most recently.
///
/// Deterministic only because every scenario below runs inside one `#[test]`,
/// sequentially. Cases in one binary otherwise run in parallel, and "newest"
/// becomes whichever test got there last. Selecting by creation line was the
/// obvious alternative and does not work: `Location::caller()` inside a
/// `macro_rules!` body reports the macro's own line, not the invocation's.
fn newest() -> SnapshotRow {
    let set = ask(&["--subsystem", "continuations"]);
    set.snapshots[0]
        .rows
        .last()
        .cloned()
        .expect("at least one continuation")
}

fn row_for(id: u64) -> SnapshotRow {
    let set = ask(&["--subsystem", "continuations", "--id", &id.to_string()]);
    assert_eq!(set.snapshots[0].returned, 1, "{:?}", set.snapshots[0]);
    set.snapshots[0].rows[0].clone()
}

fn u64_of(row: &SnapshotRow, name: &str) -> u64 {
    match row.get(name).unwrap().value {
        Some(SnapshotValue::U64(value)) => value,
        ref other => panic!("{name} was {other:?}"),
    }
}

fn str_of(row: &SnapshotRow, name: &str) -> String {
    match row.get(name).unwrap().value {
        Some(SnapshotValue::String(ref value)) => value.clone(),
        ref other => panic!("{name} was {other:?}"),
    }
}

/// Every scenario, in sequence. See [`newest`] for why they are not separate
/// `#[test]`s.
#[wasm_lite::wasm_lite_test]
fn the_registry_reports_what_happened_to_each_continuation() {
    // A live continuation is outstanding, and names its creation site -- the
    // thing an anonymous id could not tell you, and the reason `continuation()`
    // captures `Location::caller()` rather than requiring a labelled
    // constructor.
    let (sender, future) = continuation::<u32>();
    let row = newest();
    let outstanding_id = u64_of(&row, "id");
    assert_eq!(str_of(&row, "outcome"), "outstanding");
    assert!(
        str_of(&row, "created_at_file").ends_with("registry.rs"),
        "the creation site should be this file, not inside the crate: {:?}",
        str_of(&row, "created_at_file")
    );
    assert!(u64_of(&row, "created_at_line") > 0);
    assert_eq!(
        row.get("polled").unwrap().value,
        Some(SnapshotValue::Bool(false)),
        "nobody is awaiting it yet, which is a different bug from a signal that never came"
    );
    // The join column is always present. It is the continuation's logwise
    // context, and is 0 here because this test installs no runtime to mint one
    // -- which is exactly why `id` exists separately.
    assert!(row.get("context").is_some());

    // Polling and completion are both visible.
    sender.send(7);
    assert_eq!(wasm_lite_std::block_on(future), 7);
    let row = row_for(outstanding_id);
    assert_eq!(
        row.get("polled").unwrap().value,
        Some(SnapshotValue::Bool(true))
    );
    assert_eq!(str_of(&row, "outcome"), "sent");

    // A second continuation is tellable apart from the first.
    let (sender, future) = continuation::<u32>();
    let second_id = u64_of(&newest(), "id");
    assert_ne!(second_id, outstanding_id);

    // A future dropped before its value arrives is reported as hung up, which
    // is what the sender will find.
    drop(future);
    assert_eq!(str_of(&row_for(second_id), "outcome"), "future-hung-up");
    sender.send(1);

    // Dropping a sender unsent is defined as a programmer error here, and it
    // otherwise shows up only as a hang somewhere else entirely -- so it is
    // recorded *before* the panic.
    //
    // Native only: the wasm32 profile builds std with `panic_abort`, so there
    // is nothing to catch.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (sender, _future) = continuation::<u32>();
        let id = u64_of(&newest(), "id");

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(sender)));
        std::panic::set_hook(previous);
        assert!(caught.is_err(), "dropping a sender unsent should panic");

        assert_eq!(str_of(&row_for(id), "outcome"), "sender-dropped-unsent");
    }
}
