use crate::page_map::BetterAtom;
use crate::paging::vaddr;
use crate::util;
use libc::{mmap, MAP_ANONYMOUS, MAP_HUGETLB, MAP_HUGE_2MB, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use std::ptr;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page::PageRange;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageSize, PageTable, PhysFrame, Size2MiB, Size4KiB,
};
use x86_64::VirtAddr;

pub fn alloc_mmap<P: PageSize>(count: usize, zeroed: bool) -> PageRange<P> {
    // from osv/libs/mman.cc
    const MAP_UNINITIALIZED: i32 = 0x4000000;

    let page_size_flags = match P::SIZE {
        Size4KiB::SIZE => 0,
        Size2MiB::SIZE => MAP_HUGETLB | MAP_HUGE_2MB,
        _ => panic!("bad page size {}", P::SIZE_AS_DEBUG_STR),
    };
    let init_flags = if zeroed { 0 } else { MAP_UNINITIALIZED };
    let p = unsafe {
        mmap(
            ptr::null_mut(),
            count * P::SIZE as usize,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | page_size_flags | init_flags,
            -1,
            0,
        ) as *mut u8
    };
    assert!(!p.is_null());
    let p = Page::<P>::from_start_address(VirtAddr::from_ptr(p)).unwrap();
    Page::range(p, p + count as u64)
}

const KB: usize = 1 << 10;
pub const MB: usize = KB << 10;
pub const GB: usize = MB << 10;
pub const TB: usize = GB << 10;

pub const PHYS_OFFSET: u64 = 0x0000400000000000;

pub unsafe fn page_table<'a>() -> OffsetPageTable<'a> {
    OffsetPageTable::new(
        &mut *vaddr(Cr3::read().0.start_address()).as_mut_ptr::<PageTable>(),
        VirtAddr::new(PHYS_OFFSET),
    )
}

pub fn claim_frames<P: PageSize>(count: usize) -> impl Iterator<Item = PhysFrame<P>>
where
    for<'a> OffsetPageTable<'a>: Mapper<P>,
{
    util::alloc_mmap::<P>(count, false).into_iter().map(|page| {
        unsafe {
            page.start_address().as_mut_ptr::<u8>().write(0);
        }
        unsafe { util::page_table() }.translate_page(page).unwrap()
    })
}

pub fn mask<T: BetterAtom>(bits: u32) -> T {
    (T::from(1) << bits) - T::from(1)
}
