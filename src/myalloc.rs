use crate::frame_allocator::MmapFrameAllocator;
use crate::frame_list::FrameList2M;
use crate::myalloc::quantum_storage::QuantumStorage;
use crate::page_map::{PageMap, SmallCountHashMap};
use crate::paging::{allocate_l2_tables, map_huge_page, unmap_huge_page};
use crate::profile_function;
use crate::util::{alloc_mmap, page_table, MB, PHYS_OFFSET};
use crate::TestAlloc;
use ahash::RandomState;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::alloc::Layout;

use std::ops::Deref;
use std::ptr;
use std::sync::Mutex;
use x86_64::registers::control::Cr3;

use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::{Mapper, Page, PageSize, PhysFrame, Size2MiB};
use x86_64::VirtAddr;

const VIRTUAL_QUANTUM_BITS: u32 = 24;
const MAX_MID_SIZE: usize = 16 * MB;
const ADDRESS_BIT_MASK: u64 = (!0u64) >> 16;

fn address_to_quantum(a: VirtAddr) -> u32 {
    ((a.as_u64() & ADDRESS_BIT_MASK) >> VIRTUAL_QUANTUM_BITS) as u32
}

mod quantum_storage;

pub struct GlobalData {
    allocs_per_page: PageMap,
    pages_per_quantum:
        SmallCountHashMap<u32, { VIRTUAL_QUANTUM_BITS + 1 - 21 }, 0, { 48 - VIRTUAL_QUANTUM_BITS }>,
    available_frames: Mutex<Vec<PhysFrame<Size2MiB>>>,
    quantum_storage: QuantumStorage,
}

impl GlobalData {
    #[must_use]
    fn map_and_insert(
        &self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        count: usize,
    ) -> usize {
        unsafe {
            map_huge_page(page, frame);
        }
        self.allocs_per_page.insert(page, frame, count)
    }

    fn decrement_quantum(&self, q: u32) {
        if self.pages_per_quantum.decrement(q).is_some() {
            self.quantum_storage.dealloc_dirty(0, q)
        }
    }

    /// returned range is quantum aligned
    #[allow(clippy::ptr_arg)]
    fn claim_virtual_space(
        size: usize,
        _frames: &mut Vec<PhysFrame<Size2MiB>>,
    ) -> PageRangeInclusive<Size2MiB> {
        let virt_pages_exclusive = alloc_mmap((size + (1 << VIRTUAL_QUANTUM_BITS)) >> 21, false);
        let virt_pages_inclusive =
            Page::range_inclusive(virt_pages_exclusive.start, virt_pages_exclusive.end - 1);

        println!("allocating l2 tables");
        {
            let mut frame_allocator = MmapFrameAllocator::default();
            allocate_l2_tables(virt_pages_inclusive, &mut frame_allocator);
        }

        let start = virt_pages_inclusive
            .start
            .start_address()
            .as_u64()
            .next_multiple_of(1 << VIRTUAL_QUANTUM_BITS);
        assert!(start + (size as u64) < 1 << 47);
        Page::range_inclusive(
            Page::from_start_address(VirtAddr::new(start)).unwrap(),
            Page::from_start_address(VirtAddr::new(start + size as u64)).unwrap() - 1,
        )
    }

    pub fn new(physical_size: usize, virt_size: usize) -> Self {
        assert_eq!(physical_size % Size2MiB::SIZE as usize, 0);
        assert_eq!(virt_size % (1 << VIRTUAL_QUANTUM_BITS), 0);
        assert!(virt_size <= 1 << 46);

        let phys_pages = alloc_mmap::<Size2MiB>(physical_size / Size2MiB::SIZE as usize, false);
        dbg!(&phys_pages);
        dbg!(phys_pages.start.start_address().as_u64().trailing_zeros());
        for p in phys_pages {
            unsafe {
                p.start_address().as_mut_ptr::<u8>().write(0);
            }
        }
        let mut available_frames: Vec<_> = phys_pages
            .into_iter()
            .filter_map(|p| {
                let x = (unsafe { page_table() }).translate_page(p);
                x.map_err(|e| {
                    eprintln!("failed to claim frame for {p:?}: {e:?}");
                    unsafe {
                        dbg!(p.p4_index(), p.p3_index(), p.p2_index());
                        let (l4_frame, _) = Cr3::read();
                        let l4 = VirtAddr::new(l4_frame.start_address().as_u64() + PHYS_OFFSET)
                            .as_mut_ptr::<PageTableEntry>();
                        let l4e = l4.add(p.p4_index().into()).read();
                        let l3_frame = l4e.frame().unwrap_unchecked();
                        let l3 = VirtAddr::new(l3_frame.start_address().as_u64() + PHYS_OFFSET)
                            .as_mut_ptr::<PageTableEntry>();
                        let l3e = l3.add(p.p3_index().into()).read();
                        let l2_frame = l3e.frame().unwrap_unchecked();
                        let l2 = VirtAddr::new(l2_frame.start_address().as_u64() + PHYS_OFFSET)
                            .as_mut_ptr::<PageTableEntry>();
                        let l2e = l2.add(p.p2_index().into()).read();
                        dbg!(l4, l3, l2, l4e, l3e, l2e);
                    }
                    e
                })
                .ok()
            })
            .collect();

        let virt_pages_inclusive = Self::claim_virtual_space(virt_size, &mut available_frames);

        GlobalData {
            allocs_per_page: PageMap::new(
                phys_pages.count() + phys_pages.count() / 4,
                virt_pages_inclusive.start,
            ),
            pages_per_quantum: SmallCountHashMap::with_num_slots(1 << 16),
            quantum_storage: QuantumStorage::from_range(
                ((virt_pages_inclusive.start.start_address().as_u64() & ADDRESS_BIT_MASK)
                    >> VIRTUAL_QUANTUM_BITS) as u32
                    ..(((virt_pages_inclusive.end + 1).start_address().as_u64() & ADDRESS_BIT_MASK)
                        >> VIRTUAL_QUANTUM_BITS) as u32,
            ),
            available_frames: Mutex::new(available_frames),
        }
    }
}

pub struct LocalData<G: Deref<Target = GlobalData> + Send> {
    rng: SmallRng,
    available_frames: FrameList2M,
    // these are sign extended virtual addresses. be careful around the half of the address space
    min_address: u64,
    bump: u64,
    current_page_index: usize,
    current_page: Page<Size2MiB>,
    current_quantum_index: usize,
    global: G,
}

unsafe impl<G: Deref<Target = GlobalData> + Send> TestAlloc for LocalData<G> {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        profile_function!();
        if layout.size() == 0 {
            return VirtAddr::new(PHYS_OFFSET).as_mut_ptr();
        }
        if layout.size() > MAX_MID_SIZE {
            return self.alloc_large(layout);
        }
        let aligned_bump = self.bump & !(layout.align() as u64 - 1);
        let new_bump = aligned_bump - layout.size() as u64;
        if new_bump < self.min_address {
            self.decrement_page(self.current_page);
            self.release_frames();
            self.claim_quantum().unwrap();
            return self.alloc(layout);
        }
        let min_page = Page::<Size2MiB>::containing_address(VirtAddr::new_unsafe(new_bump));
        if min_page == self.current_page {
            self.global
                .allocs_per_page
                .increment_at(self.current_page_index, self.current_page);
        } else {
            let max_page = Page::<Size2MiB>::containing_address(VirtAddr::new(aligned_bump) - 1u64);
            let required_frames = self.current_page - min_page;
            if self.get_frames(required_frames as usize).is_err() {
                eprintln!("out of memory");
                return ptr::null_mut();
            }
            let current_quantum = address_to_quantum(self.current_page.start_address());
            debug_assert!(current_quantum == address_to_quantum(min_page.start_address()));
            self.global.pages_per_quantum.increment_at(
                self.current_quantum_index,
                current_quantum,
                (self.current_page - min_page) as u32,
            );
            if max_page != self.current_page {
                self.decrement_page(self.current_page)
            }
            for p in Page::range(min_page, self.current_page).skip(1) {
                let _ = self
                    .global
                    .map_and_insert(p, self.available_frames.pop().unwrap(), 1);
            }
            self.current_page = min_page;
            self.current_page_index =
                self.global
                    .map_and_insert(min_page, self.available_frames.pop().unwrap(), 2);
            self.release_frames();
        }
        self.bump = new_bump;
        VirtAddr::new_unsafe(self.bump).as_mut_ptr()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        profile_function!();
        if layout.size() == 0 {
            return;
        }
        if layout.size() > MAX_MID_SIZE {
            return self.dealloc_large(ptr, layout);
        }
        let start_addr = VirtAddr::from_ptr(ptr);
        let min_page = Page::<Size2MiB>::containing_address(start_addr);
        let max_page =
            Page::<Size2MiB>::containing_address(start_addr + layout.size() as u64 - 1u64);
        for p in Page::range_inclusive(min_page, max_page) {
            self.decrement_page(p);
        }
        self.release_frames();
    }
}

impl<G: Deref<Target = GlobalData> + Send> LocalData<G> {
    fn get_frames(&mut self, count: usize) -> Result<(), ()> {
        profile_function!();
        self.available_frames
            .steal_from_vec(&mut self.global.available_frames.lock().unwrap(), count)?;
        Ok(())
    }

    fn large_alloc_info(size: usize) -> (u32, usize) {
        let level = size
            .next_power_of_two()
            .trailing_zeros()
            .saturating_sub(VIRTUAL_QUANTUM_BITS);
        let frame_count = size.next_multiple_of(Size2MiB::SIZE as usize) / Size2MiB::SIZE as usize;
        (level, frame_count)
    }

    fn alloc_large(&mut self, layout: Layout) -> *mut u8 {
        profile_function!();
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let Some(quantum) = self.global.quantum_storage.alloc(level, &mut self.rng) else {
            return ptr::null_mut();
        };
        if self.get_frames(frame_count).is_err() {
            self.global.quantum_storage.dealloc_clean(level, quantum);
            return ptr::null_mut();
        }
        let first_page = Page::<Size2MiB>::containing_address(VirtAddr::new(
            (quantum as u64) << VIRTUAL_QUANTUM_BITS,
        ));
        for i in 0..frame_count {
            unsafe {
                map_huge_page(first_page + i as u64, self.available_frames.pop().unwrap());
            }
        }
        first_page.start_address().as_mut_ptr()
    }

    pub fn new(seed: u64, global: G) -> Result<Self, ()> {
        let mut r = LocalData {
            rng: SmallRng::seed_from_u64(RandomState::with_seed(0xee61096f95490820).hash_one(seed)),
            available_frames: Default::default(),
            min_address: 1u64 << 40,
            bump: 1 << 40,
            current_page_index: usize::MAX,
            current_quantum_index: usize::MAX,
            current_page: Page::containing_address(VirtAddr::new(1 << 40)),
            global,
        };
        r.claim_quantum()?;
        Ok(r)
    }

    fn claim_quantum(&mut self) -> Result<(), ()> {
        profile_function!();
        let q = self
            .global
            .quantum_storage
            .alloc(0, &mut self.rng)
            .ok_or(())?;
        self.get_frames(1)
            .map_err(|_| self.global.quantum_storage.dealloc_clean(0, q))?;
        self.min_address = VirtAddr::new((q as u64) << VIRTUAL_QUANTUM_BITS).as_u64();
        self.bump = VirtAddr::new((q as u64 + 1) << VIRTUAL_QUANTUM_BITS).as_u64();
        debug_assert!(self.min_address | ADDRESS_BIT_MASK == self.bump | ADDRESS_BIT_MASK);
        self.current_quantum_index = self.global.pages_per_quantum.insert(q, 0, 1);
        self.current_page = Page::from_start_address(VirtAddr::new(self.bump)).unwrap() - 1;
        self.current_page_index =
            self.global
                .map_and_insert(self.current_page, self.available_frames.pop().unwrap(), 1);
        Ok(())
    }

    fn dealloc_large(&mut self, ptr: *mut u8, layout: Layout) {
        profile_function!();
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let address = VirtAddr::from_ptr(ptr);
        let first_page = Page::<Size2MiB>::from_start_address(address).unwrap();
        for i in 0..frame_count {
            unsafe {
                self.available_frames
                    .push(unmap_huge_page(first_page + i as u64));
            }
        }
        self.release_frames();
        self.global
            .quantum_storage
            .dealloc_dirty(level, address_to_quantum(address));
    }

    fn release_frames(&mut self) {
        profile_function!();
        if !self.available_frames.is_empty() {
            self.available_frames
                .merge_into_vec(&mut self.global.available_frames.lock().unwrap());
        }
    }

    fn decrement_page(&mut self, p: Page<Size2MiB>) {
        profile_function!();
        if let Some(x) = self.global.allocs_per_page.decrement(p) {
            self.available_frames.push(x);
            unsafe { unmap_huge_page(p) };
            self.global
                .decrement_quantum(address_to_quantum(p.start_address()))
        }
    }
}
