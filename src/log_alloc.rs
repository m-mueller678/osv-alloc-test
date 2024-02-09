use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

struct LogAlloc;

fn rust_alloc_point() {
    dbg!();
}

unsafe impl GlobalAlloc for LogAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        eprintln!("alloc {:8}", layout.size());
        let ptr = libc::malloc(layout.size());
        rust_alloc_point();
        ptr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        eprintln!("free  {:8}", layout.size());
        libc::free(ptr as *mut c_void);
        dbg!()
    }
}

#[cfg(feature = "log_rust_global_alloc")]
#[global_allocator]
static ALLOC: LogAlloc = LogAlloc;
