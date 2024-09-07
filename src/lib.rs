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
}




#[derive(Debug)]
struct Shared<R> {
    data: UnsafeCell<MaybeUninit<R>>,
    state: AtomicU8,
}


#[derive(Debug)]
pub struct Sender<R> {
    shared: Arc<Shared<R>>,
}



pub fn continuation<R>() -> (Sender<R>,Future<R>) {

    let shared = Arc::new(Shared {
        data: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicU8::new(State::Empty as u8),
    });
    (Sender { shared: shared.clone() }, Future { shared })
}


impl<R> Sender<R> {
    pub fn send(self, data: R) {
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
            _ => unreachable!("Invalid state"),
        }
        unsafe {
            let opt = &mut *self.shared.data.get();
            std::ptr::write(opt.as_mut_ptr(), data);
        }
        self.shared.state.store(State::Data as u8, Ordering::Release);
    }
}







#[derive(Debug)]
pub struct Future<R> {
    shared: Arc<Shared<R>>,
}

impl<R> std::future::Future for Future<R> {
    type Output = R;
    fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let load = self.shared.state.load(Ordering::Acquire);
        match load {
            u if u == State::Empty as u8 => {return Poll::Pending}
            u if u == State::Gone as u8 => {panic!("Continuation already polled")}
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
                Poll::Ready(val)
            }

            _ => unreachable!("Invalid state"),

        }
    }
}

//tedious traits

//I think we don't want clone on either type, because it creates problems for implementing Send.
unsafe impl<R: Send> Send for Future<R> {}
unsafe impl <R: Send> Send for Sender<R> {}
fn warn() {

}
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
        assert_eq!(truntime::poll_once(f.as_mut()), Poll::Pending);
        c.send(23);
        assert_eq!(truntime::poll_once(f), Poll::Ready(23));
    }

    #[test] fn test_is_send() {
        fn is_send<T: Send>() {}
        is_send::<crate::Future<i32>>();
        is_send::<crate::Sender<i32>>();
    }
}