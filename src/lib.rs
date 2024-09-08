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
    Hangup,
}

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
}



pub fn continuation<R>() -> (Sender<R>,Future<R>) {

    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
        waker: atomic_waker::AtomicWaker::new(),
    });
    (Sender { shared: shared.clone() }, Future { shared })
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
    pub fn send(self, data: R)  {
        /*
        Safety: Data can only be written by this type.  Since the type is !Sync,
        it can only be written by this thread.

        Read is guarded by the state flag.

        */
        //misuse check
        let state = self.shared.state.load(Ordering::Acquire);
        match state {
            u if u == State::Empty as u8 => {}
            u if u == State::Data as u8 || u == State::Gone as u8 => {panic!("Continuation already resumed")},
            u if u == State::Hangup as u8 => {
                //sending to a hungup continuation is a no-op
                return;
            },
            _ => unreachable!("Invalid state"),
        }
        unsafe {
            let opt = &mut *self.shared.data.get();
            std::ptr::write(opt.as_mut_ptr(), data);
        }
        self.shared.state.store(State::Data as u8, Ordering::Release);
    }

    /**
    Determines if the underlying future is cancelled.  And thus, that sending data will have no effect.
    */
    pub fn is_cancelled(&self) -> bool {
        self.shared.state.load(Ordering::Relaxed) == State::Hangup as u8
    }
}

impl<R> Drop for Sender<R> {
    fn drop(&mut self) {
        let state = self.shared.state.load(Ordering::Relaxed);
        match state {
            u if u == State::Empty as u8 => {
                self.shared.state.store(State::Hangup as u8, Ordering::Relaxed);
            }
            u if u == State::Data as u8 => {
                //do nothing
            }
            u if u == State::Gone as u8 => {
                //do nothing
            }
            u if u == State::Hangup as u8 => {
                //do nothing
            }
            _ => unreachable!("Invalid state"),
        }
    }
}







#[derive(Debug)]
pub struct Future<R> {
    shared: Arc<Shared<R>>,
}

impl<R> Drop for Future<R> {
    fn drop(&mut self) {
        self.shared.state.store(State::Hangup as u8, Ordering::Relaxed);
    }
}

enum ReadStatus<R> {
    Data(R),
    Waiting,
    Hangup
}

impl<R> Future<R> {
    fn read_if_available(&self) -> ReadStatus<R> {
        let state = self.shared.state.load(Ordering::Acquire);
        match state {
            u if u == State::Empty as u8 => {ReadStatus::Waiting}
            u if u == State::Data as u8 => {
                let val = unsafe {
                    //safety: We know that the continuation has been resumed, so we can read the data
                    let data = &mut *self.shared.data.get();
                    /*safety: we know the data was initialized and will never be written to again (only
                    written to in empty state.

                    We know it will never be read again because we set gone before leaving the function.
                    It can only be polled exclusively in this function since we have &mut self.
                     */
                    let r = data.assume_init_read();
                    //and will never be read again because
                    self.shared.state.store(State::Gone as u8, Ordering::Relaxed);
                    r
                };
                ReadStatus::Data(val)
            }
            u if u == State::Hangup as u8 => {ReadStatus::Hangup}
            u if u == State::Gone as u8 => {panic!("Continuation already polled")}
            _ => unreachable!("Invalid state"),
        }
    }
}



impl<R> std::future::Future for Future<R> {
    type Output = Result<R,Error>;
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        //optimistic read.
        match self.read_if_available() {
            ReadStatus::Data(data) => {return Poll::Ready(Ok(data))}
            ReadStatus::Waiting => {}
            ReadStatus::Hangup => {return Poll::Ready(Err(Error::Hangup))}
        }
        //register for wakeup
        self.shared.waker.register(cx.waker());
        //recheck
        match self.read_if_available() {
            ReadStatus::Data(data) => {Poll::Ready(Ok(data))}
            ReadStatus::Waiting => {Poll::Pending}
            ReadStatus::Hangup => {Poll::Ready(Err(Error::Hangup))}
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
        let mut f = Pin::new(&mut f);
        drop(c);
        match truntime::poll_once(f) {
            Poll::Ready(Err(crate::Error::Hangup)) => {}
            x => panic!("Unexpected result {:?}",x),
        }
    }
}