use crate::frame_allocator::MmapFrameAllocator;
use crate::util::PHYS_OFFSET;
use std::mem::MaybeUninit;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::{
    FrameAllocator, Page, PageTable, PageTableFlags, PhysFrame, Size2MiB,
};
use x86_64::{PhysAddr, VirtAddr};

pub fn allocate_l2_tables(
    range: PageRangeInclusive<Size2MiB>,
    frame_allocator: &mut MmapFrameAllocator,
) {
    let mut leaked_frames = 0;
    let (l4_frame, _) = Cr3::read();
    let l4 = VirtAddr::new(l4_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    for i4 in usize::from(range.start.p4_index())..=usize::from(range.end.p4_index()) {
        let l4_entry = unsafe { ensure_present(l4.add(i4), frame_allocator).pte };
        assert!(l4_entry.flags().contains(PageTableFlags::PRESENT));
        let l3 = VirtAddr::new(
            unsafe { l4_entry.frame().unwrap_unchecked() }
                .start_address()
                .as_u64()
                + PHYS_OFFSET,
        )
        .as_mut_ptr::<PageTableEntry>();
        let i3_start = if i4 == usize::from(range.start.p4_index()) {
            usize::from(range.start.p3_index())
        } else {
            0
        };
        let i3_end = if i4 == usize::from(range.end.p4_index()) {
            usize::from(range.end.p3_index())
        } else {
            511
        };
        for i3 in i3_start..=i3_end {
            let EnsurePresent {
                pte: l3_entry,
                is_new,
            } = unsafe { ensure_present(l3.add(i3), frame_allocator) };
            if !is_new {
                let l2_frame = l3_entry.frame().unwrap();
                let l2 = VirtAddr::new(l2_frame.start_address().as_u64() + PHYS_OFFSET)
                    .as_mut_ptr::<PageTableEntry>();
                let i2_start = if i3 == i3_start {
                    usize::from(range.start.p2_index())
                } else {
                    0
                };
                let i2_end = if i3 == i3_end {
                    usize::from(range.end.p2_index())
                } else {
                    511
                };
                for i2 in i2_start..=i2_end {
                    unsafe {
                        let l2e = &mut *(l2.add(i2));
                        if l2e.flags().contains(PageTableFlags::PRESENT) {
                            eprintln!("leaking frame {l2e:?}");
                            leaked_frames += 1;
                        }
                        *l2e = PageTableEntry::new();
                    }
                }
            }
        }
    }
    println!("global tlb flush");
    tlb_flush_global();
    println!("leaked {leaked_frames} frames");
}

pub fn tlb_flush_global() {
    unsafe {
        libc::syscall(0x1000);
    }
}

pub unsafe fn map_huge_page(page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>) {
    let (l4_frame, _) = Cr3::read();
    let l4 = VirtAddr::new(l4_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4.add(page.p4_index().into()).read().frame().unwrap();
    let l3 = VirtAddr::new(l3_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3.add(page.p3_index().into()).read().frame().unwrap();
    let l2 = VirtAddr::new(l2_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l2_entry = &mut *l2.add(page.p2_index().into());
    debug_assert!(l2_entry.is_unused());
    l2_entry.set_addr(
        frame.start_address(),
        PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE | PageTableFlags::WRITABLE,
    );
}

pub unsafe fn unmap_huge_page(page: Page<Size2MiB>) -> PhysFrame<Size2MiB> {
    let (l4_frame, _) = Cr3::read();
    let l4 = VirtAddr::new(l4_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4
        .add(page.p4_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l3 = VirtAddr::new(l3_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3
        .add(page.p3_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l2 = VirtAddr::new(l2_frame.start_address().as_u64() + PHYS_OFFSET)
        .as_mut_ptr::<PageTableEntry>();
    let l2_entry = l2
        .add(page.p2_index().into())
        .replace(PageTableEntry::new());
    debug_assert!(l2_entry
        .flags()
        .contains(PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE));
    PhysFrame::from_start_address(l2_entry.addr()).unwrap()
}

pub fn vaddr(x: PhysAddr) -> VirtAddr {
    VirtAddr::new(x.as_u64() + PHYS_OFFSET)
}

pub fn paddr(x: VirtAddr) -> PhysAddr {
    PhysAddr::new(x.as_u64() - PHYS_OFFSET)
}

struct EnsurePresent {
    pte: PageTableEntry,
    is_new: bool,
}

/// pe must not be concurrently written to.
/// Hopefully osv does not do that.
unsafe fn ensure_present(
    pe: *mut PageTableEntry,
    frame_allocator: &mut MmapFrameAllocator,
) -> EnsurePresent {
    let is_new = false;
    {
        frame_allocator.refill();
        let pe = &mut *pe;
        if pe.is_unused() {
            assert!(pe.is_unused(), "unexpected page flags: {:?}", pe.flags());
            let new_frame = frame_allocator.allocate_frame().unwrap().start_address();
            vaddr(new_frame)
                .as_mut_ptr::<MaybeUninit<PageTable>>()
                .write(MaybeUninit::zeroed());
            pe.set_addr(
                new_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            )
        } else {
            assert!(pe.flags().contains(PageTableFlags::PRESENT));
        }
    }
    EnsurePresent {
        pte: pe.read(),
        is_new,
    }
}
