/*!
Adds a simple continuation type to Rust
*/


use std::cell::UnsafeCell;
use std::fmt::{Debug};
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Poll;

#[repr(u8)]
enum State {
    Empty,
    Data,
    Gone,
    FutureHangup,
}

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("The continuation was hung up")]
    Hangup,
}





#[derive(Debug)]
struct Shared<R> {
    data: UnsafeCell<MaybeUninit<R>>,
    state: AtomicU8,
    waker: atomic_waker::AtomicWaker,
}


#[derive(Debug)]
pub struct Sender<R> {
    shared: Arc<Shared<R>>,
    sent: bool,
}



pub fn continuation<R>() -> (Sender<R>,Future<R>) {

    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
    });
    (Sender { shared: shared.clone(), sent: false }, Future { shared })
}


impl<R> Sender<R> {
    /**
    Sends the data to the remote side.

    Note that there is no particular guarantee that the remote side will receive this.  For example,
    the remote side may be dropped already, in which case sending has no effect.  Alternatively, the remote
    side may become dropped after sending.

    If you have a particularly good way of handling this, you may want to check [is_cancelled] before sending.
    Note that this is not perfect either (since the remote side may be dropped after the check but before the send).



*/
    pub fn send(mut self, data: R)  {
        self.sent = true;

        /*
        Safety: Data can only be written by this type.  Since the type is !Sync,
        it can only be written by this thread.

        Read is guarded by the state flag.

        */
        //misuse check
        let state = self.shared.state.load(Ordering::Relaxed);
        match state {
            u if u == State::Empty as u8 => {}
            u if u == State::Data as u8 || u == State::Gone as u8 => {panic!("Continuation already resumed")},
            u if u == State::FutureHangup as u8 => {
                //sending to a hungup continuation is a no-op
                return;
            },
            //sender hangup is impossible
            _ => unreachable!("Invalid state"),
        }
        unsafe {
            let opt = &mut *self.shared.data.get();
            std::ptr::write(opt.as_mut_ptr(), data);
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
                        u if u == State::Data as u8 || u == State::Gone as u8 => {panic!("Continuation already resumed")}
                        u if u == State::FutureHangup as u8 => {
                            //sending to a hungup continuation is a no-op
                            //however, we did write our data, so we need to drop it.
                            unsafe {
                                //safety: We know that the continuation has been resumed, so we can read the data
                                let data = &mut *self.shared.data.get();
                                //safety: we know the data was initialized and will never be written to again (only
                                //written to in empty state.
                                let _ = data.assume_init_read();
                            }
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
    */
    pub fn is_cancelled(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == State::FutureHangup as u8
    }
}

impl<R> Drop for Sender<R> {
    fn drop(&mut self) {
        let state = self.shared.state.load(Ordering::Relaxed);
        assert!(self.sent, "Sender dropped without sending data");
    }
}







#[derive(Debug)]
pub struct Future<R> {
    shared: Arc<Shared<R>>,
}

impl<R> Drop for Future<R> {
    fn drop(&mut self) {
        let swap = self.shared.state.swap(State::FutureHangup as u8, Ordering::Acquire);
        match swap {
            u if u == State::Empty as u8 => {}
            u if u == State::Data as u8 => {
                //data needs to be dropped here
                unsafe {
                    //safety: We know that the continuation has been resumed, so we can read the data
                    let data = &mut *self.shared.data.get();
                    //safety: we know the data was initialized and will never be written to again (only
                    //written to in empty state.
                    let _ = data.assume_init_read();
                }
            }
            u if u == State::Gone as u8 => {}
            _ => unreachable!("Invalid state"),
        }
    }
}

enum ReadStatus<R> {
    Data(R),
    Waiting,
    Spurious,
    Hangup
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
                    return ReadStatus::Data(r);
                }
            }
            Err(u) => {
                match u {
                    u if u == State::Empty as u8 => { return ReadStatus::Waiting }
                    u if u == State::Data as u8 => { return ReadStatus::Spurious }
                    u if u == State::Gone as u8 => { panic!("Continuation already polled") }
                    //future hangup is impossible
                    _ => { unreachable!("Invalid state") }
                }
            }
        }
    }
}



impl<R> std::future::Future for Future<R> {
    type Output = Result<R,Error>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        //optimistic read.
        let state = self.shared.state.compare_exchange_weak(State::Data as u8, State::Gone as u8, Ordering::Acquire, Ordering::Relaxed);
        match Self::interpret_result(state, &self.shared.data) {
            ReadStatus::Data(data) => {return Poll::Ready(Ok(data))}
            ReadStatus::Waiting => {}
            ReadStatus::Hangup => {return Poll::Ready(Err(Error::Hangup))}
            ReadStatus::Spurious => {}
        }
        //register for wakeup
        self.shared.waker.register(cx.waker());
        loop {
            let state2 = self.shared.state.compare_exchange_weak(State::Data as u8, State::Gone as u8, Ordering::Acquire, Ordering::Relaxed);
            match Self::interpret_result(state2, &self.shared.data) {
                ReadStatus::Data(data) => {return Poll::Ready(Ok(data))}
                ReadStatus::Waiting => {return Poll::Pending}
                ReadStatus::Hangup => {return Poll::Ready(Err(Error::Hangup))}
                ReadStatus::Spurious => {continue}
            }
        }
    }
}

//tedious traits

//I think we don't want clone on either type, because it creates problems for implementing Send.
unsafe impl<R: Send> Send for Future<R> {}
unsafe impl <R: Send> Send for Sender<R> {}

/*Since no clone, no copy

I think we don't want Eq/Ord/hash because we don't expect multiple instances, since no clone.

Default does not make a lot of sense because we generate types as a pair.
 */




#[cfg(test)]
mod test {
    use std::pin::Pin;
    use std::task::Poll;
    use crate::continuation;

    #[test]
    fn test_continue() {
        let(c,mut f) = continuation();
        let mut f = Pin::new(&mut f);
        assert!(truntime::poll_once(f.as_mut()).is_pending());
        c.send(23);
        match truntime::poll_once(f) {
            Poll::Ready(Ok(23)) => {}
            x => panic!("Unexpected result {:?}",x),
        }
    }

    #[test] fn test_is_send() {
        fn is_send<T: Send>() {}
        is_send::<crate::Future<i32>>();
        is_send::<crate::Sender<i32>>();
    }

    #[test] fn test_sender_hangup() {
        let(c,mut f) = continuation::<i32>();
        let f = Pin::new(&mut f);
        drop(c);
        match truntime::poll_once(f) {
            Poll::Ready(Err(crate::Error::Hangup)) => {}
            x => panic!("Unexpected result {:?}",x),
        }
    }
}