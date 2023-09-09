use std::{
    future::Future,
    sync::{Arc, Condvar, Mutex},
    task::{Context, Poll, Wake, Waker, RawWaker, RawWakerVTable},
    collections::VecDeque,
    cell::RefCell, time::Duration,
};

use futures::{
    future::LocalBoxFuture, FutureExt,
};
use scoped_tls::scoped_thread_local;

struct Target1demo;
impl Future for Target1demo {
    type Output = ();
    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        println!("target1: hello world!");
        std::task::Poll::Ready(())
    }
}

fn dummy_waker() -> Waker {
    static DATA: () = ();
    unsafe { Waker::from_raw(RawWaker::new(&DATA, &VTABLE))}
}

const VTABLE: RawWakerVTable = RawWakerVTable::new(vtable_clone, vtable_wake, vtable_wake_by_ref, vtable_drop);
unsafe fn vtable_clone(_p: *const ()) -> RawWaker {
    RawWaker::new(_p, &VTABLE)
}
unsafe fn vtable_wake(_p: *const ()) {}
unsafe fn vtable_wake_by_ref(_p: *const ()) {}
unsafe fn vtable_drop(_p: *const ()) {}

struct Signal {
    state: Mutex<State>,
    cond: Condvar,
}

enum State {
    Empty,
    Waiting,
    Notified,
}

impl Signal {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::Empty),
            cond: Condvar::new(),
        }
    }
    fn wait(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            State::Notified => *state = State::Empty,
            State::Waiting => {
                panic!("multiple wait");
            }
            State::Empty => {
                *state = State::Waiting;
                while let State::Waiting = *state {
                    state = self.cond.wait(state).unwrap();
                }
            }
        }
    }
    fn notify(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            State::Notified => {}
            State::Empty => *state = State::Notified,
            State::Waiting => {
                *state = State::Empty;
                self.cond.notify_one();
            }
        }
    }
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
}

scoped_thread_local!(static SIGNAL: Arc<Signal>);
scoped_thread_local!(static RUNNABLE: Mutex<VecDeque<Arc<Task>>>);

struct Task {
    future: RefCell<LocalBoxFuture<'static, ()>>,
    signal: Arc<Signal>,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        RUNNABLE.with(|runnable| runnable.lock().unwrap().push_back(self.clone()));
        self.signal.notify();
    }
}

fn target1_block_on<F: Future>(future: F) -> F::Output {
    let mut fut = std::pin::pin!(future);
    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

fn target2_block_on<F: Future>(future: F) -> F::Output {
    let mut fut = std::pin::pin!(future);
    let signal = Arc::new(Signal::new());
    let waker = Waker::from(signal.clone());
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
            return output;
        }
        signal.wait();
    }
}

pub fn target3_block_on<F: Future>(future: F) -> F::Output {
    let mut main_fut = std::pin::pin!(future);
    let signal = Arc::new(Signal::new());
    let waker = Waker::from(signal.clone());
    let mut cx = Context::from_waker(&waker);
    let runnable = Mutex::new(VecDeque::with_capacity(1024));
    SIGNAL.set(&signal, || {
        RUNNABLE.set(&runnable, || {
            loop {
                if let Poll::Ready(output) = main_fut.as_mut().poll(&mut cx) {
                    return output;
                }
                while let Some(task) = runnable.lock().unwrap().pop_back() {
                    let waker = Waker::from(task.clone());
                    let mut cx = Context::from_waker(&waker);
                    let _ = task.future.borrow_mut().as_mut().poll(&mut cx);
                }
                signal.wait();
            }
        })
    })
}

pub fn my_spawn(fut: impl Future<Output = ()> + 'static) {
    let t = Arc::new(Task{
        future: RefCell::new(fut.boxed_local()),
        signal: Arc::new(Signal::new()),
    });
    RUNNABLE.with(|runnable| {
        runnable.lock().unwrap().push_back(t)
    });
}



fn main() {
    target1_block_on(Target1demo);
    target2_block_on(target2_demo());
    target3_block_on(target3_demo());
}

async fn target2_demo() {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        tx.send_blocking(())
    });
    let _ = rx.recv().await;
    println!("target2: hello world!");
}

async fn target3_demo() {
    let (tx, rx) = async_channel::bounded(1);
    my_spawn(target3_demo2(tx));
    println!("target3: hello world!");
    let _ = rx.recv().await;
}

async fn target3_demo2(tx: async_channel::Sender<()>) {
    println!("target3: hello world2!");
    let _ = tx.send(()).await;
}