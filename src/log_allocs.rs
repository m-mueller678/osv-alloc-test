use std::io::{stdout, BufWriter, Write};
use std::sync::Mutex;

static LOG:Mutex<Vec<isize>>=Mutex::new(Vec::new());

pub fn log_alloc(size:isize){
    const LIMIT:usize=1<<17;
    let mut lock = LOG.lock().unwrap();
    if lock.len()>=LIMIT{
        write_logs(&mut *lock);
    }
    lock.push(size);
}

fn write_logs(l:&mut Vec<isize>){
    let out = stdout().lock();
    let mut out = BufWriter::new(out);
    for x in &*l{
        writeln!(out, "5c0ce10c {}", x).unwrap();
    }
    out.flush().unwrap();
    l.clear();
}

pub fn flush_alloc_log(){
    let mut lock = LOG.lock().unwrap();
    write_logs(&mut *lock);
}