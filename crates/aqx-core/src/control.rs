use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mio::{Poll, Token, Waker};

use crate::session::Session;

const WAKE: Token = Token(1);

pub type Posted = Box<dyn FnOnce(&mut Session) + Send>;

pub struct Control {
    pub(crate) poll: Poll,
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    interrupt: Arc<AtomicBool>,
    posted: Arc<Mutex<Vec<Posted>>>,
}

#[derive(Clone)]
pub struct StopHandle {
    waker: Arc<Waker>,
    stop: Arc<AtomicBool>,
    interrupt: Arc<AtomicBool>,
    posted: Arc<Mutex<Vec<Posted>>>,
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

    pub fn post(&self, f: impl FnOnce(&mut Session) + Send + 'static) {
        if let Ok(mut posted) = self.posted.lock() {
            posted.push(Box::new(f));
        }
        let _ = self.waker.wake();
    }
}

impl Control {
    pub fn new() -> io::Result<Self> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKE)?);
        Ok(Control {
            poll,
            waker,
            stop: Arc::new(AtomicBool::new(false)),
            interrupt: Arc::new(AtomicBool::new(false)),
            posted: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn stop_handle(&self) -> StopHandle {
        StopHandle { waker: self.waker.clone(), stop: self.stop.clone(), interrupt: self.interrupt.clone(), posted: self.posted.clone() }
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

    pub(crate) fn take_posted(&self) -> Vec<Posted> {
        self.posted.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
    }

    pub(crate) fn clear_stop(&self) {
        self.stop.store(false, Ordering::SeqCst);
        self.interrupt.store(false, Ordering::SeqCst);
    }
}
