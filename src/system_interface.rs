use log::warn;
use std::{
    alloc::{Allocator, Layout},
    mem::MaybeUninit,
};
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        page::PageRangeInclusive, page_table::PageTableEntry, FrameAllocator, Page, PageSize,
        PageTable, PageTableFlags, PhysFrame, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

/// # Safety
/// Addresses must be non-zero
pub unsafe trait SystemInterface: Sized + Copy {
    fn allocate_virtual(self, layout: Layout) -> VirtAddr;
    fn allocate_physical(self, layout: Layout) -> PhysAddr;
    fn global_tlb_flush(self);
    fn vaddr(self, addr: PhysAddr) -> VirtAddr;
    fn paddr(self, addr: VirtAddr) -> PhysAddr;
    unsafe fn prepare_page_table(self, range: PageRangeInclusive<Size2MiB>) {
        direct_access_prepare_page_table(self, range);
    }

    unsafe fn map(self, page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>) {
        direct_access_map(self, page, frame);
    }

    unsafe fn unmap(self, page: Page<Size2MiB>) -> PhysFrame<Size2MiB> {
        direct_access_unmap(self, page)
    }
    fn trace_recycle_backoff(self) {}
    fn trace_recycle(self) {}
    fn allocator(self) -> Self::Alloc;
    type Alloc: Allocator + Clone;
}

pub unsafe fn direct_access_map(
    sys: impl SystemInterface,
    page: Page<Size2MiB>,
    frame: PhysFrame<Size2MiB>,
) {
    let (l4_frame, _) = Cr3::read();
    let l4 = sys
        .vaddr(l4_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4.add(page.p4_index().into()).read().frame().unwrap();
    let l3 = sys
        .vaddr(l3_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3.add(page.p3_index().into()).read().frame().unwrap();
    let l2 = sys
        .vaddr(l2_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l2_entry = &mut *l2.add(page.p2_index().into());
    debug_assert!(l2_entry.is_unused());
    l2_entry.set_addr(
        frame.start_address(),
        PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE | PageTableFlags::WRITABLE,
    );
}
pub unsafe fn direct_access_unmap(
    sys: impl SystemInterface,
    page: Page<Size2MiB>,
) -> PhysFrame<Size2MiB> {
    let (l4_frame, _) = Cr3::read();
    let l4 = sys
        .vaddr(l4_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l3_frame = l4
        .add(page.p4_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l3 = sys
        .vaddr(l3_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l2_frame = l3
        .add(page.p3_index().into())
        .read()
        .frame()
        .unwrap_unchecked();
    let l2 = sys
        .vaddr(l2_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    let l2_entry = l2
        .add(page.p2_index().into())
        .replace(PageTableEntry::new());
    debug_assert!(l2_entry
        .flags()
        .contains(PageTableFlags::PRESENT | PageTableFlags::HUGE_PAGE));
    PhysFrame::from_start_address(l2_entry.addr()).unwrap()
}

pub fn direct_access_prepare_page_table(
    sys: impl SystemInterface,
    range: PageRangeInclusive<Size2MiB>,
) {
    struct SystemFrameAllocator<S: SystemInterface>(S);

    unsafe impl<S: SystemInterface> FrameAllocator<Size4KiB> for SystemFrameAllocator<S> {
        fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
            let layout =
                Layout::from_size_align(Size4KiB::SIZE as usize, Size4KiB::SIZE as usize).unwrap();
            Some(PhysFrame::from_start_address(self.0.allocate_physical(layout)).unwrap())
        }
    }
    struct EnsurePresent {
        pte: PageTableEntry,
        is_new: bool,
    }

    /// pe must not be concurrently written to.
    unsafe fn ensure_present(sys: impl SystemInterface, pe: *mut PageTableEntry) -> EnsurePresent {
        let is_new = false;
        {
            let pe = &mut *pe;
            if pe.is_unused() {
                assert!(pe.is_unused(), "unexpected page flags: {:?}", pe.flags());
                let new_frame = SystemFrameAllocator(sys)
                    .allocate_frame()
                    .unwrap()
                    .start_address();
                sys.vaddr(new_frame)
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
    let l4 = sys
        .vaddr(l4_frame.start_address())
        .as_mut_ptr::<PageTableEntry>();
    for i4 in usize::from(range.start.p4_index())..=usize::from(range.end.p4_index()) {
        let l4_entry = unsafe { ensure_present(sys, l4.add(i4)).pte };
        assert!(l4_entry.flags().contains(PageTableFlags::PRESENT));
        let l3 = sys
            .vaddr(unsafe { l4_entry.frame().unwrap_unchecked() }.start_address())
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
            } = unsafe { ensure_present(sys, l3.add(i3)) };
            if !is_new {
                let l2_frame = l3_entry.frame().unwrap();
                let l2 = sys
                    .vaddr(l2_frame.start_address())
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
                            warn!("leaking frame {l2e:?}");
                            leaked_frames += 1;
                        }
                        *l2e = PageTableEntry::new();
                    }
                }
            }
        }
    }
    sys.global_tlb_flush();
    if leaked_frames > 0 {
        warn!("leaked {leaked_frames} frames");
    }
}
