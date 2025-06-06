use crate::log_allocs::{flush_alloc_log, log_alloc};
use crate::myalloc::{GlobalData, LocalData};
use crate::TestAlloc;
use ahash::RandomState;
use std::alloc::Layout;
use std::cell::RefCell;
use std::ops::Deref;
use std::sync::OnceLock;
use tracing::info;

type CLocalData = LocalData<GlobalGlobal>;

struct GlobalGlobal;

impl Deref for GlobalGlobal {
    type Target = GlobalData;

    fn deref(&self) -> &Self::Target {
        GLOBAL.get().expect("allocator not initialized")
    }
}

static GLOBAL: OnceLock<GlobalData> = OnceLock::new();
thread_local! {
    static LOCAL: RefCell<CLocalData> = RefCell::new(CLocalData::new(RandomState::with_seed(0).hash_one(std::thread::current().id()),GlobalGlobal).unwrap())
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_init(physical_size: u64, virtual_size: u64) {
    let mut did_init = false;
    GLOBAL.get_or_init(|| {
        did_init = true;
        tracing_subscriber::fmt()
            .event_format(tracing_subscriber::fmt::format().without_time().compact())
            .init();
        GlobalData::new(physical_size as usize, virtual_size as usize)
    });
    info!("init done: {did_init:?}");
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_alloc(size: u64, align: u64) -> *mut libc::c_void {
    let r = LOCAL.with(|l| {
        l.borrow_mut().alloc(Layout::from_size_align_unchecked(
            size as usize,
            align as usize,
        )) as *mut libc::c_void
    });
    r
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_free(size: u64, align: u64, ptr: *mut libc::c_void) {
    LOCAL.with(|l| {
        l.borrow_mut().dealloc(
            ptr as *mut u8,
            Layout::from_size_align_unchecked(size as usize, align as usize),
        )
    });
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_flush_log() {
    flush_alloc_log();
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_log_alloc(x: i64) {
    log_alloc(x as isize)
}
