use libc::{MAP_ANONYMOUS, MAP_HUGETLB, MAP_HUGE_2MB, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page::PageRange;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageSize, PageTable, Size2MiB, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

use crate::myalloc::{GlobalData, LocalData};
use crate::util::VIRTUAL_QUANTUM_BITS;
use crate::{SystemInterface, TestAlloc};
use std::alloc::{Layout, System};
use std::cell::{RefCell, SyncUnsafeCell};
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::{self, NonNull};
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
    if GLOBAL_INIT_STATE.load(Ordering::Relaxed) == 2 {
        return;
    }
    assert_eq!(GLOBAL_INIT_STATE.swap(1, Ordering::Relaxed), 0);
    (*GLOBAL.get()).write(GlobalData::new(
        OsvSystemInterface,
        physical_size as usize,
        virtual_size as usize,
    ));
    GLOBAL_INIT_STATE.store(2, Ordering::Relaxed);
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

pub const PHYS_OFFSET: u64 = 0x0000400000000000;
#[derive(Clone, Copy)]
struct OsvSystemInterface;
unsafe impl SystemInterface for OsvSystemInterface {
    fn allocate_virtual(self, layout: Layout) -> x86_64::VirtAddr {
        assert!(layout.align() <= (1 << VIRTUAL_QUANTUM_BITS));
        // returned range is quantum aligned
        let virt_pages_exclusive =
            alloc_mmap::<Size2MiB>((layout.size() + (1 << VIRTUAL_QUANTUM_BITS)) >> 21, false);
        let virt_pages_inclusive =
            Page::range_inclusive(virt_pages_exclusive.start, virt_pages_exclusive.end - 1);

        let start = virt_pages_inclusive
            .start
            .start_address()
            .as_u64()
            .next_multiple_of(1 << VIRTUAL_QUANTUM_BITS);
        assert!(start + (layout.size() as u64) < 1 << 47);
        VirtAddr::new(start)
    }

    fn allocate_physical(self, layout: Layout) -> x86_64::PhysAddr {
        assert_eq!(layout.size(), layout.align());
        if layout.size() == Size2MiB::SIZE as usize {
            let virt = alloc_mmap::<Size2MiB>(1, false);
            unsafe {
                virt.start
                    .start_address()
                    .as_mut_ptr::<usize>()
                    .write_volatile(0);
            }
            unsafe { page_table() }
                .translate_page(virt.start)
                .unwrap()
                .start_address()
        } else if layout.size() == Size4KiB::SIZE as usize {
            let virt = alloc_mmap::<Size4KiB>(1, false);
            unsafe {
                virt.start
                    .start_address()
                    .as_mut_ptr::<usize>()
                    .write_volatile(0);
            }
            unsafe { page_table() }
                .translate_page(virt.start)
                .unwrap()
                .start_address()
        } else {
            unimplemented!()
        }
    }

    fn global_tlb_flush(self) {
        unsafe {
            libc::syscall(0x1000);
        }
    }

    fn vaddr(self, addr: x86_64::PhysAddr) -> x86_64::VirtAddr {
        VirtAddr::new(addr.as_u64() + PHYS_OFFSET)
    }

    fn paddr(self, addr: x86_64::VirtAddr) -> x86_64::PhysAddr {
        PhysAddr::new(addr.as_u64() - PHYS_OFFSET)
    }

    fn allocator(self) -> Self::Alloc {
        System
    }

    type Alloc = System;
}

pub fn alloc_mmap<P: PageSize>(count: usize, zeroed: bool) -> PageRange<P> {
    // from osv/libs/mman.cc
    const MAP_UNINITIALIZED: i32 = 0x4000000;
    let page_size_flags = match P::SIZE {
        Size4KiB::SIZE => 0,
        Size2MiB::SIZE => MAP_HUGETLB | MAP_HUGE_2MB,
        _ => panic!("bad page size {}", P::DEBUG_STR),
    };
    let init_flags = if zeroed { 0 } else { MAP_UNINITIALIZED };
    let p = unsafe {
        libc::mmap(
            ptr::null_mut(),
            count * P::SIZE as usize,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | page_size_flags | init_flags,
            -1,
            0,
        ) as *mut u8
    };
    if (p as i64) == -1 {
        panic!("mmap failed: {:?}", std::io::Error::last_os_error());
    }

    assert!(!p.is_null());
    let p = Page::<P>::from_start_address(VirtAddr::from_ptr(p)).unwrap();
    Page::range(p, p + count as u64)
}

unsafe fn page_table<'a>() -> OffsetPageTable<'a> {
    OffsetPageTable::new(
        &mut *OsvSystemInterface
            .vaddr(Cr3::read().0.start_address())
            .as_mut_ptr::<PageTable>(),
        VirtAddr::new(PHYS_OFFSET),
    )
}
