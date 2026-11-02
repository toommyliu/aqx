use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mio::{Poll, Token, Waker};

const WAKE: Token = Token(1);

pub struct Control {
    pub(crate) poll: Poll,
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    interrupt: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct StopHandle {
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    interrupt: Arc<AtomicBool>,
}

impl StopHandle {
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.waker.wake();
    }

    pub fn interrupt(&self) {
        self.interrupt.store(true, Ordering::SeqCst);
        let _ = self.waker.wake();
    }

    pub fn clear_interrupt(&self) {
        self.interrupt.store(false, Ordering::SeqCst);
    }
}

impl Control {
    pub fn new() -> io::Result<Self> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKE)?);
        Ok(Control { poll, waker, stop: Arc::new(AtomicBool::new(false)), interrupt: Arc::new(AtomicBool::new(false)) })
    }

    pub fn stop_handle(&self) -> StopHandle {
        StopHandle { waker: self.waker.clone(), stop: self.stop.clone(), interrupt: self.interrupt.clone() }
    }

    pub(crate) fn waker(&self) -> Arc<Waker> {
        self.waker.clone()
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub(crate) fn take_interrupt(&self) -> bool {
        self.interrupt.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn clear_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
        self.interrupt.store(false, Ordering::SeqCst);
    }
}
