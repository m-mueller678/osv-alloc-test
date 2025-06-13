use crate::LogAllocMessage;
use fast_clock::std_clocks::InstantClock;
use fast_clock::tsc::{CalibratedTsc, Tsc, TscInstant};
use fast_clock::{Clock, ClockSynchronization};
use std::cell::Cell;
use std::io::{stdout, BufWriter, Write};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use std::{hint, mem};

const BUF_SIZE: usize = 1 << 16;

#[repr(C)]
struct LocalBuffer {
    id: usize,
    anchor: ClockSynchronization<InstantClock, CalibratedTsc>,
    buf_len: AtomicUsize,
    clean: AtomicBool,
    buf: [(AtomicU64, AtomicIsize); BUF_SIZE],
}

impl LocalBuffer {
    fn new_box() -> Box<Self> {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let mut b = Box::<Self>::new_zeroed();
        unsafe {
            let ptr = b.as_mut_ptr();
            (*ptr).id = ID.fetch_add(1, Relaxed);
            (&raw mut (*ptr).anchor).write(ClockSynchronization::new_aba(InstantClock, TIME.0));
            b.assume_init()
        }
    }
}

static TIME: LazyLock<(CalibratedTsc, Instant)> = LazyLock::new(|| {
    (
        Tsc::try_new_assume_stable().unwrap().calibrate(),
        Instant::now(),
    )
});

#[thread_local]
static LOCAL: Cell<Option<&'static LocalBuffer>> = Cell::new(None);

static ALL_LOCALS: Mutex<Vec<&'static LocalBuffer>> = Mutex::new(Vec::new());

fn get_local() -> &'static LocalBuffer {
    if LOCAL.get().is_none() {
        hint::cold_path();
        let r = Box::leak(LocalBuffer::new_box());
        ALL_LOCALS.lock().unwrap().push(r);
        LOCAL.set(Some(r));
    }
    LOCAL.get().unwrap()
}

pub fn log_alloc(size: isize) {
    let log = get_local();
    let pos = log.buf_len.fetch_add(1, Relaxed);
    let time: u64 = unsafe { mem::transmute(log.anchor.b().now()) };
    log.buf[pos].0.store(time, Relaxed);
    log.buf[pos].1.store(size, Relaxed);
}

fn write_logs(l: &LocalBuffer, out: &mut impl Write, flush_id: u64) {
    let len = l.buf_len.load(Relaxed);
    write!(out, "c1f04237,{flush_id},{}", l.id).unwrap();
    if l.clean.swap(true, Relaxed) {
        for (time, event) in &l.buf[..len] {
            let time: u64 = time.load(Relaxed);
            let time: TscInstant = unsafe { mem::transmute(time) };
            let time = l.anchor.to_a(time).duration_since(TIME.1).as_nanos();
            let event = event.load(Relaxed);
            write!(out, ";{time},{event}",).unwrap();
        }
    } else {
        let msg = LogAllocMessage::Dirty as isize;
        write!(out, ";{msg},{msg}").unwrap();
    }
    writeln!(out).unwrap();
    l.buf_len.store(0, Relaxed);
}

pub fn flush_alloc_log(flush_id: u64) {
    log_alloc(LogAllocMessage::PreFlush as isize);
    let mut out = BufWriter::new(stdout().lock());
    for log in ALL_LOCALS.lock().unwrap().iter() {
        write_logs(log, &mut out, flush_id)
    }
    out.flush().unwrap();
    log_alloc(LogAllocMessage::PostFlush as isize);
}
