use crate::myalloc::{GlobalData, LocalData};
use crate::{SystemInterface, TestAlloc};
use std::alloc::{Global, Layout, System};
use std::cell::{RefCell, SyncUnsafeCell};
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

type CLocalData = LocalData<OsvSystemInterface, GlobalGlobal>;

static GLOBAL_INIT_STATE: AtomicUsize = AtomicUsize::new(0);

struct GlobalGlobal;

impl Deref for GlobalGlobal {
    type Target = GlobalData<OsvSystemInterface>;

    fn deref(&self) -> &Self::Target {
        assert_eq!(GLOBAL_INIT_STATE.load(Ordering::Acquire), 2);
        unsafe { (*GLOBAL.get()).assume_init_ref() }
    }
}

static RANDOM_SEED: AtomicU64 = AtomicU64::new(0);

static GLOBAL: SyncUnsafeCell<MaybeUninit<GlobalData<OsvSystemInterface>>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());
thread_local! {
    static LOCAL: RefCell<CLocalData> = RefCell::new(CLocalData::new(RANDOM_SEED.fetch_add(1, Ordering::Relaxed),GlobalGlobal))
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_init(physical_size: u64, virtual_size: u64) {
    assert_eq!(GLOBAL_INIT_STATE.swap(1, Ordering::Relaxed), 0);
    (*GLOBAL.get()).write(GlobalData::new(
        OsvSystemInterface,
        physical_size as usize,
        virtual_size as usize,
    ));
    GLOBAL_INIT_STATE.store(1, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_alloc(size: u64, align: u64) -> *mut libc::c_void {
    let r = LOCAL.with(|l| {
        l.borrow_mut()
            .alloc(Layout::from_size_align_unchecked(
                size as usize,
                align as usize,
            ))
            .map_or(Default::default(), NonNull::as_ptr) as *mut libc::c_void
    });
    r
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_free(size: u64, _align: u64, ptr: *mut libc::c_void) {
    LOCAL.with(|l| {
        l.borrow_mut()
            .dealloc(NonNull::new_unchecked(ptr as *mut u8), size as usize)
    });
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_flush_log(id: u64) {
    todo!()
}

#[no_mangle]
pub unsafe extern "C" fn global_virtual_alloc_log_alloc(x: i64) {
    todo!()
}

#[derive(Clone, Copy)]
struct OsvSystemInterface;
unsafe impl SystemInterface for OsvSystemInterface {
    fn allocate_virtual(self, layout: Layout) -> x86_64::VirtAddr {
        todo!()
    }

    fn allocate_physical(self, layout: Layout) -> x86_64::PhysAddr {
        todo!()
    }

    fn global_tlb_flush(self) {
        todo!()
    }

    fn vaddr(self, addr: x86_64::PhysAddr) -> x86_64::VirtAddr {
        todo!()
    }

    fn paddr(self, addr: x86_64::VirtAddr) -> x86_64::PhysAddr {
        todo!()
    }

    fn allocator(self) -> Self::Alloc {
        todo!()
    }

    type Alloc = System;
}
