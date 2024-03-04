use crate::myalloc::{GlobalData, LocalData};
use crate::TestAlloc;
use ahash::RandomState;
use std::alloc::Layout;
use std::cell::RefCell;
use std::ops::Deref;
use std::panic::{catch_unwind, UnwindSafe};
use std::sync::OnceLock;

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
    catch(|| {
        let mut did_init = false;
        eprintln!("init start");
        GLOBAL.get_or_init(|| {
            did_init = true;
            GlobalData::new(physical_size as usize, virtual_size as usize)
        });
        eprintln!("init done: {did_init:?}");
    })
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_alloc(size: u64, align: u64) -> *mut libc::c_void {
    catch(|| {
        eprintln!("alloc {size}");
        let r = LOCAL.with(|l| {
            l.borrow_mut().alloc(Layout::from_size_align_unchecked(
                size as usize,
                align as usize,
            )) as *mut libc::c_void
        });
        eprintln!("done alloc {size}");
        for i in 0..size {
            (r as *mut u8).add(i as usize).write(0);
        }
        eprintln!("done zero {size}");
        r
    })
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_free(size: u64, align: u64, ptr: *mut libc::c_void) {
    catch(|| {
        eprintln!("dealloc {size}");
        LOCAL.with(|l| {
            l.borrow_mut().dealloc(
                ptr as *mut u8,
                Layout::from_size_align_unchecked(size as usize, align as usize),
            )
        });
        eprintln!("done dealloc {size}");
    })
}

fn catch<B, F: FnOnce() -> B + UnwindSafe>(f: F) -> B {
    match catch_unwind(f) {
        Ok(a) => a,
        Err(_) => std::process::abort(),
    }
}
