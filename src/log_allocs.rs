use crate::LogAllocMessage;
use array_init::array_init;
use minstant::Anchor;
use std::io::{stdout, BufWriter, Write};
use std::mem;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize};
use thread_local::ThreadLocal;

struct LocalBuffer {
    clean: AtomicBool,
    id: usize,
    anchor: Anchor,
    buf: [(AtomicU64, AtomicIsize); 1 << 16],
    buf_len: AtomicUsize,
}

static ID: AtomicUsize = AtomicUsize::new(0);

static LOG: ThreadLocal<LocalBuffer> = ThreadLocal::new();

pub fn log_alloc(size: isize) {
    let log = LOG.get_or(|| LocalBuffer {
        clean: AtomicBool::new(false),
        id: ID.fetch_add(1, Relaxed),
        anchor: Default::default(),
        buf: array_init(|_| Default::default()),
        buf_len: Default::default(),
    });
    let pos = log.buf_len.load(Relaxed);
    let time: u64 = unsafe { mem::transmute(minstant::Instant::now()) };
    log.buf[pos].0.store(time, Relaxed);
    log.buf[pos].1.store(size, Relaxed);
}

fn write_logs(l: &LocalBuffer, out: &mut impl Write) {
    let len = l.buf_len.load(Relaxed);
    write!(out, "c1f04237,{}", l.id).unwrap();
    if l.clean.swap(true, Relaxed) {
        for (time, event) in &l.buf[..len] {
            let time: u64 = time.load(Relaxed);
            let time: minstant::Instant = unsafe { mem::transmute(time) };
            let time = time.as_unix_nanos(&l.anchor);
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

pub fn flush_alloc_log() {
    log_alloc(LogAllocMessage::PreFlush as isize);
    let mut out = BufWriter::new(stdout().lock());
    for log in LOG.iter() {
        write_logs(log, &mut out)
    }
    out.flush().unwrap();
    log_alloc(LogAllocMessage::PostFlush as isize);
}
