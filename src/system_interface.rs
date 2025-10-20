use std::{alloc::Layout, marker::PhantomData, mem::MaybeUninit};

use tracing::warn;
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        page::PageRangeInclusive, page_table::PageTableEntry, FrameAllocator, Page, PageSize,
        PageTable, PageTableFlags, PhysFrame, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

pub unsafe trait SystemInterface: Sized {
    fn allocate_virtual(layout: Layout) -> VirtAddr;
    fn allocate_physical(layout: Layout) -> PhysAddr;
    fn global_tlb_flush();
    fn vaddr(addr: PhysAddr) -> VirtAddr;
    fn paddr(addr: VirtAddr) -> PhysAddr;
    unsafe fn prepare_page_table(range: PageRangeInclusive<Size2MiB>) {
        direct_access_prepare_page_table::<Self>(range);
    }

    unsafe fn map(page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>) {
        direct_access_map::<Self>(page, frame);
    }

    unsafe fn unmap(page: Page<Size2MiB>) -> PhysFrame<Size2MiB> {
        direct_access_unmap::<Self>(page)
    }
    fn trace_recycle_backoff() {}
    fn trace_recycle() {}
}

pub unsafe fn direct_access_map<S: SystemInterface>(
    page: Page<Size2MiB>,
    frame: PhysFrame<Size2MiB>,
) {
    let (l4_frame, _) = Cr3::read();
    let l4 = S::vaddr(l4_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4.add(page.p4_index().into()).read().frame().unwrap();
    let l3 = S::vaddr(l3_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3.add(page.p3_index().into()).read().frame().unwrap();
    let l2 = S::vaddr(l2_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l2_entry = &mut *l2.add(page.p2_index().into());
    debug_assert!(l2_entry.is_unused());
    l2_entry.set_addr(
        frame.start_address(),
        PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE | PageTableFlags::WRITABLE,
    );
}
pub unsafe fn direct_access_unmap<S: SystemInterface>(page: Page<Size2MiB>) -> PhysFrame<Size2MiB> {
    let (l4_frame, _) = Cr3::read();
    let l4 = S::vaddr(l4_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4
        .add(page.p4_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l3 = S::vaddr(l3_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3
        .add(page.p3_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l2 = S::vaddr(l2_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    let l2_entry = l2
        .add(page.p2_index().into())
        .replace(PageTableEntry::new());
    debug_assert!(l2_entry
        .flags()
        .contains(PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE));
    PhysFrame::from_start_address(l2_entry.addr()).unwrap()
}

pub fn direct_access_prepare_page_table<S: SystemInterface>(range: PageRangeInclusive<Size2MiB>) {
    struct SystemFrameAllocator<S: SystemInterface>(PhantomData<S>);

    unsafe impl<S: SystemInterface> FrameAllocator<Size4KiB> for SystemFrameAllocator<S> {
        fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
            let layout =
                Layout::from_size_align(Size4KiB::SIZE as usize, Size4KiB::SIZE as usize).unwrap();
            Some(PhysFrame::from_start_address(S::allocate_physical(layout)).unwrap())
        }
    }
    struct EnsurePresent {
        pte: PageTableEntry,
        is_new: bool,
    }

    /// pe must not be concurrently written to.
    unsafe fn ensure_present<S: SystemInterface>(pe: *mut PageTableEntry) -> EnsurePresent {
        let is_new = false;
        {
            let pe = &mut *pe;
            if pe.is_unused() {
                assert!(pe.is_unused(), "unexpected page flags: {:?}", pe.flags());
                let new_frame = SystemFrameAllocator::<S>(PhantomData)
                    .allocate_frame()
                    .unwrap()
                    .start_address();
                S::vaddr(new_frame)
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

    let mut leaked_frames = 0;
    let (l4_frame, _) = Cr3::read();
    let l4 = S::vaddr(l4_frame.start_address()).as_mut_ptr::<PageTableEntry>();
    for i4 in usize::from(range.start.p4_index())..=usize::from(range.end.p4_index()) {
        let l4_entry = unsafe { ensure_present::<S>(l4.add(i4)).pte };
        assert!(l4_entry.flags().contains(PageTableFlags::PRESENT));
        let l3 = S::vaddr(unsafe { l4_entry.frame().unwrap_unchecked() }.start_address())
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
            } = unsafe { ensure_present::<S>(l3.add(i3)) };
            if !is_new {
                let l2_frame = l3_entry.frame().unwrap();
                let l2 = S::vaddr(l2_frame.start_address()).as_mut_ptr::<PageTableEntry>();
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
                            warn!("leaking frame {l2e:?}");
                            leaked_frames += 1;
                        }
                        *l2e = PageTableEntry::new();
                    }
                }
            }
        }
    }
    S::global_tlb_flush();
    if leaked_frames > 0 {
        warn!("leaked {leaked_frames} frames");
    }
}
