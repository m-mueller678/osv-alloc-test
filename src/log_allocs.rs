use crate::LogAllocMessage;
use itertools::Itertools;
use minstant::Anchor;
use std::io::{stdout, BufWriter, Write};
use std::mem;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize};
use thread_local::ThreadLocal;

const BUF_SIZE: usize = 1 << 16;

struct LocalBuffer {
    clean: AtomicBool,
    id: usize,
    anchor: Anchor,
    buf: Box<[(AtomicU64, AtomicIsize); BUF_SIZE]>,
    buf_len: AtomicUsize,
}

static ID: AtomicUsize = AtomicUsize::new(0);

static LOG: ThreadLocal<LocalBuffer> = ThreadLocal::new();

pub fn log_alloc(size: isize) {
    let log = LOG.get_or(|| LocalBuffer {
        clean: AtomicBool::new(false),
        id: ID.fetch_add(1, Relaxed),
        anchor: Default::default(),
        buf: std::iter::repeat_with(Default::default)
            .take(BUF_SIZE)
            .collect_vec()
            .into_boxed_slice()
            .try_into()
            .unwrap(),
        buf_len: Default::default(),
    });
    let pos = log.buf_len.fetch_add(1, Relaxed);
    let time: u64 = unsafe { mem::transmute(minstant::Instant::now()) };
    log.buf[pos].0.store(time, Relaxed);
    log.buf[pos].1.store(size, Relaxed);
}

fn write_logs(l: &LocalBuffer, out: &mut impl Write, flush_id: u64) {
    let len = l.buf_len.load(Relaxed);
    write!(out, "c1f04237,{id},{}", l.id).unwrap();
    if l.clean.swap(true, Relaxed) {
        for (time, event) in &l.buf[..len] {
            let time: u64 = time.load(Relaxed);
            let time: minstant::Instant = unsafe { mem::transmute(time) };
            let time = time.as_unix_nanos(&l.anchor);
            let event = event.load(Relaxed);
            write!(out, ";{time},{event}",).unwrap();
        }
    } else {
        assert_eq!(flush_id, 0);
    }
    writeln!(out).unwrap();
    l.buf_len.store(0, Relaxed);
}

pub fn flush_alloc_log(flush_id: u64) {
    log_alloc(LogAllocMessage::PreFlush as isize);
    let mut out = BufWriter::new(stdout().lock());
    for log in LOG.iter() {
        write_logs(log, &mut out, flush_id)
    }
    out.flush().unwrap();
    log_alloc(LogAllocMessage::PostFlush as isize);
}
