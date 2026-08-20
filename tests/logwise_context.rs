// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::Cell;
use std::future::Future as StdFuture;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};

use logwise::{ContextToken, Dispatch, EventRef, Interest, Metadata};

#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(target_arch = "wasm32")]
use wasm_lite_std as thread;

thread_local! {
    static CURRENT: Cell<ContextToken> = const { Cell::new(ContextToken::NONE) };
}

struct Capture {
    next: AtomicU64,
    parents: Mutex<Vec<(ContextToken, ContextToken)>>,
    links: Mutex<Vec<(ContextToken, ContextToken)>>,
    events: Mutex<Vec<(&'static str, ContextToken)>>,
}

impl Dispatch for Capture {
    fn generation(&self) -> usize {
        0
    }

    fn interest(&self, _metadata: &'static Metadata) -> Interest {
        Interest::CORE_LOCAL
    }

    fn emit(&self, event: EventRef<'_>) {
        self.events
            .lock()
            .unwrap()
            .push((event.metadata.event_name, event.context));
    }

    fn capture_context(&self) -> ContextToken {
        CURRENT.with(Cell::get)
    }

    fn create_context(&self, parent: ContextToken, _name: &'static str) -> ContextToken {
        let child = ContextToken::from_parts(self.next.fetch_add(1, Ordering::Relaxed) + 1, 0);
        self.parents.lock().unwrap().push((child, parent));
        child
    }

    fn link_context(&self, context: ContextToken, related: ContextToken) {
        self.links.lock().unwrap().push((context, related));
    }

    fn enter_context(&self, context: ContextToken) -> ContextToken {
        CURRENT.with(|current| current.replace(context))
    }

    fn exit_context(&self, previous: ContextToken) {
        CURRENT.with(|current| current.set(previous));
    }
}

static CAPTURE: Capture = Capture {
    next: AtomicU64::new(100),
    parents: Mutex::new(Vec::new()),
    links: Mutex::new(Vec::new()),
    events: Mutex::new(Vec::new()),
};

fn poll_once<F: StdFuture>(future: Pin<&mut F>) -> Poll<F::Output> {
    let mut context = Context::from_waker(Waker::noop());
    future.poll(&mut context)
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_lite::wasm_lite_test(worker))]
fn creator_is_parent_completer_is_link_and_scopes_restore() {
    logwise::install_dispatcher(&CAPTURE).expect("install capture dispatcher");

    let creator = ContextToken::from_parts(1, 0);
    let poller = ContextToken::from_parts(2, 0);
    let completer = ContextToken::from_parts(3, 0);
    let (sender, mut future) = {
        let _creator = logwise::context::enter(creator);
        r#continue::continuation::<u64>()
    };
    assert!(logwise::context::capture().is_none());

    {
        let _poller = logwise::context::enter(poller);
        assert!(poll_once(Pin::new(&mut future)).is_pending());
        assert_eq!(logwise::context::capture(), poller);
    }
    assert!(logwise::context::capture().is_none());

    thread::spawn(move || {
        let _completer = logwise::context::enter(completer);
        sender.send(42);
        assert_eq!(logwise::context::capture(), completer);
    })
    .join()
    .expect("completion thread");

    let value = {
        let _poller = logwise::context::enter(poller);
        let result = poll_once(Pin::new(&mut future));
        assert_eq!(logwise::context::capture(), poller);
        result
    };
    assert_eq!(value, Poll::Ready(42));
    assert!(logwise::context::capture().is_none());

    let parents = CAPTURE.parents.lock().unwrap();
    assert_eq!(parents.len(), 1);
    let continuation = parents[0].0;
    assert_eq!(parents[0].1, creator);
    drop(parents);
    assert_eq!(
        *CAPTURE.links.lock().unwrap(),
        vec![(continuation, completer)]
    );

    let events = CAPTURE.events.lock().unwrap();
    #[cfg(feature = "logwise-forensic")]
    {
        assert!(events.contains(&("continue.continuation.completed", continuation)));
        assert!(events.contains(&("continue.continuation.resumed", continuation)));
        assert!(events.iter().all(|event| event.1 == continuation));
    }
    #[cfg(not(feature = "logwise-forensic"))]
    assert!(events.is_empty());
}
