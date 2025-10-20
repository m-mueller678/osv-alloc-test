use crate::SystemInterface;
use ahash::HashMap;
use std::{alloc::Global, collections::HashSet, ptr, sync::Mutex};
use x86_64::{
    structures::paging::{
        page::{PageRange, PageRangeInclusive},
        Page, PageSize, PhysFrame, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

pub struct TestSystemInterface(Mutex<TestSystemInterfaceInner>);

struct TestSystemInterfaceInner {
    identity_mapped_4k: HashSet<u64>,
    free_physical: HashSet<u64>,
    next_physical: u64,
    prepared: Option<PageRangeInclusive<Size2MiB>>,
    unmapped_virtual: HashSet<u64>,
    mapped_virtual: HashMap<u64, u64>,
    dirty_virtual: HashSet<u64>,
}

unsafe impl SystemInterface for &TestSystemInterface {
    fn allocate_virtual(self, layout: std::alloc::Layout) -> x86_64::VirtAddr {
        let mut this = self.0.lock().unwrap();
        assert!(layout.align() == Size2MiB::SIZE as usize);
        assert!(layout.size().is_multiple_of(Size2MiB::SIZE as usize));
        unsafe {
            let start = Page::<Size2MiB>::from_start_address(VirtAddr::from_ptr(libc::mmap(
                ptr::null_mut(),
                layout.size(),
                libc::PROT_NONE,
                libc::MAP_PRIVATE
                    | libc::MAP_ANONYMOUS
                    | libc::MAP_NORESERVE
                    | libc::MAP_HUGETLB
                    | libc::MAP_HUGE_2MB,
                -1,
                0,
            )))
            .unwrap();
            assert!(!start.start_address().is_null());
            let range = PageRange {
                start,
                end: start + layout.size() as u64 / Size2MiB::SIZE,
            };
            for page in range {
                this.unmapped_virtual.insert(page.start_address().as_u64());
            }
            start.start_address()
        }
    }

    fn allocate_physical(self, layout: std::alloc::Layout) -> x86_64::PhysAddr {
        let mut this = self.0.lock().unwrap();
        this.next_physical = this.next_physical.next_multiple_of(layout.align() as u64);
        let ret = this.next_physical;
        this.next_physical += layout.size() as u64;
        if layout.size() == Size4KiB::SIZE as usize && layout.size() == Size4KiB::SIZE as usize {
            this.identity_mapped_4k.insert(ret);
        } else if layout.size() == Size2MiB::SIZE as usize
            && layout.size() == Size2MiB::SIZE as usize
        {
            this.free_physical.insert(ret);
        } else {
            panic!("unsupported phys alloc");
        }
        PhysAddr::new(ret as u64)
    }

    fn global_tlb_flush(self) {}

    fn vaddr(self, addr: x86_64::PhysAddr) -> x86_64::VirtAddr {
        todo!()
    }

    fn paddr(self, addr: x86_64::VirtAddr) -> x86_64::PhysAddr {
        todo!()
    }

    fn allocator(self) -> Self::Alloc {
        Global
    }

    type Alloc = Global;

    unsafe fn prepare_page_table(
        self,
        range: x86_64::structures::paging::page::PageRangeInclusive<Size2MiB>,
    ) {
        assert!(self.0.lock().unwrap().prepared.replace(range).is_none());
    }

    unsafe fn map(
        self,
        page: x86_64::structures::paging::Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
    ) {
        crate::system_interface::direct_access_map(self, page, frame);
    }

    unsafe fn unmap(self, page: x86_64::structures::paging::Page<Size2MiB>) -> PhysFrame<Size2MiB> {
        crate::system_interface::direct_access_unmap(self, page)
    }

    fn trace_recycle_backoff(self) {}

    fn trace_recycle(self) {}
}
