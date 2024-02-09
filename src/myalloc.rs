use crate::buddymap::BuddyTower;
use crate::page_map::{PageMap, SmallCountHashMap};
use crate::paging::{allocate_l2_tables, map_huge_page, unmap_huge_page};
use crate::{alloc_mmap, page_table, MmapFrameAllocator, TestAlloc, PHYS_OFFSET, TB, MB};
use ahash::RandomState;
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::alloc::Layout;
use std::ptr;
use std::sync::{Arc, Mutex};
use x86_64::structures::paging::mapper::{MapperFlushAll, UnmapError};
use x86_64::structures::paging::{Mapper, Page, PageSize, PhysFrame, Size2MiB};
use x86_64::VirtAddr;

const VIRTUAL_QUANTUM_BITS: u32 = 24;
const MAX_MID_SIZE: usize = 16*MB;
const ADDRESS_BIT_MASK: u64 = (!0u64) >> 16;

struct GlobalData {
    allocs_per_page: PageMap,
    pages_per_quantum:
        SmallCountHashMap<u32, { VIRTUAL_QUANTUM_BITS + 1 - 21 }, 0, { 48 - VIRTUAL_QUANTUM_BITS }>,
    available_frames: Mutex<Vec<PhysFrame<Size2MiB>>>,
    available_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    released_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
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
            self.released_quanta.insert(0, q)
        }
    }

    fn decrement_page(&self, p: Page<Size2MiB>) {
        if let Some(x) = self.allocs_per_page.decrement(p) {
            self.available_frames.lock().unwrap().push(x);
            self.decrement_quantum(
                ((p.start_address().as_u64() & ADDRESS_BIT_MASK) >> VIRTUAL_QUANTUM_BITS) as u32,
            )
        }
    }
}

#[derive(Clone)]
pub struct LocalData {
    rng: SmallRng,
    available_frames: Vec<PhysFrame<Size2MiB>>,
    // these are sign extended virtual addresses. be careful around the half of the address space
    min_address: u64,
    bump: u64,
    current_page_index: usize,
    current_page: Page<Size2MiB>,
    current_quantum_index: usize,
    global: Arc<GlobalData>,
}

unsafe impl TestAlloc for LocalData {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return VirtAddr::new(PHYS_OFFSET).as_mut_ptr();
        }
        if layout.size() > MAX_MID_SIZE {
            return self.alloc_large(layout);
        }
        let aligned_bump = self.bump & !(layout.align() as u64 - 1);
        let new_bump = aligned_bump - layout.size() as u64;
        if new_bump < self.min_address {
            self.global.decrement_page(self.current_page);
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
            if max_page != self.current_page {
                self.global.decrement_page(self.current_page)
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
            let current_qunatum = ((self.current_page.start_address().as_u64() & ADDRESS_BIT_MASK)
                >> VIRTUAL_QUANTUM_BITS) as u32;
            self.global
                .pages_per_quantum
                .increment_at(self.current_quantum_index, current_qunatum);
            debug_assert!(self.available_frames.is_empty());
        }
        self.bump = new_bump;
        VirtAddr::new_unsafe(self.bump).as_mut_ptr()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        if layout.size() > MAX_MID_SIZE {
            return self.dealloc_large(ptr, layout)
        }
        let start_addr = VirtAddr::from_ptr(ptr);
        let min_page = Page::<Size2MiB>::containing_address(start_addr);
        let max_page =
            Page::<Size2MiB>::containing_address(start_addr + layout.size() as u64 - 1u64);
        for p in Page::range_inclusive(min_page, max_page) {
            self.global.decrement_page(p);
        }
    }
}

impl LocalData {
    fn get_frames(&mut self, count: usize) -> Result<(), ()> {
        assert!(self.available_frames.is_empty());
        let mut gf = self.global.available_frames.lock().unwrap();
        if gf.len() < count {
            return Err(());
        }
        let new_len = gf.len() - count;
        self.available_frames.extend_from_slice(&gf[new_len..]);
        gf.truncate(new_len);
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
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let Some(quantum) = self.global.available_quanta.remove(level, &mut self.rng) else {
            return ptr::null_mut();
        };
        if self.get_frames(frame_count).is_err() {
            self.global.available_quanta.insert(level, quantum);
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

    pub fn create(threads: usize, physical_size: usize,virt_size:usize) -> Vec<Self> {
        assert_eq!(physical_size % Size2MiB::SIZE as usize, 0);
        assert_eq!(virt_size % (1<<VIRTUAL_QUANTUM_BITS), 0);
        assert!(virt_size <= 1<<46);

        let phys_pages = alloc_mmap::<Size2MiB>(physical_size / Size2MiB::SIZE as usize, false);
        for p in phys_pages {
            unsafe {
                p.start_address().as_mut_ptr::<u8>().write(0);
            }
        }
        // these must be quantum aligned
        let virt_area_start = 1u64 << 47;
        let virt_area_end = virt_area_start + virt_size as u64;

        let virt_pages = Page::range(
            Page::containing_address(VirtAddr::new(virt_area_start)),
            Page::containing_address(VirtAddr::new(virt_area_end)),
        );
        println!("allocating l2 tables");
        unsafe {
            let mut frame_allocator = MmapFrameAllocator::default();
            allocate_l2_tables(
                Page::range_inclusive(virt_pages.start, virt_pages.end - 1),
                &mut frame_allocator,
            );
        }
        println!("unmapping virtual range pages");
        {
            let _pt = unsafe { page_table() };
            for (i, p) in virt_pages.into_iter().enumerate() {
                if cfg!(debug_assertions) && i % (1 << 18) == 0 {
                    eprintln!(
                        "{i:8}/{:8} (({:.2})%)",
                        virt_pages.count(),
                        i as f64 / virt_pages.count() as f64 * 100.0
                    );
                }
                match unsafe { page_table() }.unmap(p) {
                    Ok((f, flush)) => {
                        println!("unmapped {f:?} from virtual range");
                        flush.ignore();
                    }
                    Err(UnmapError::PageNotMapped) => {
                        continue;
                    }
                    Err(e) => panic!("cannot unmap {p:?} in virtual range: {e:?}"),
                }
            }
            MapperFlushAll::new().flush_all();
        }
        println!("unmapping complete");

        let virtual_quanta = BuddyTower::from_range(
            (virt_area_start >> VIRTUAL_QUANTUM_BITS) as u32
                ..(virt_area_end >> VIRTUAL_QUANTUM_BITS) as u32,
        );
        virtual_quanta.print_counts();

        let global = Arc::new(GlobalData {
            allocs_per_page: PageMap::new(
                phys_pages.count() + phys_pages.count() / 4,
                virt_pages.start,
            ),
            pages_per_quantum: SmallCountHashMap::with_num_slots(1 << 16),
            released_quanta: BuddyTower::from_range(0..0),
            available_frames: Mutex::new(
                phys_pages
                    .into_iter()
                    .map(|p| unsafe { page_table() }.translate_page(p).unwrap())
                    .collect(),
            ),
            available_quanta: virtual_quanta,
        });

        let ret = (0..threads)
            .map(|i| {
                let mut r = LocalData {
                    rng: SmallRng::seed_from_u64(
                        RandomState::with_seed(0xee61096f95490820).hash_one(i),
                    ),
                    available_frames: Vec::new(),
                    min_address: 1u64 << 40,
                    bump: 1 << 40,
                    current_page_index: usize::MAX,
                    current_quantum_index: usize::MAX,
                    current_page: Page::containing_address(VirtAddr::new(1 << 40)),
                    global: global.clone(),
                };
                r.claim_quantum().unwrap();
                r
            })
            .collect();
        println!("allocator constructed");
        ret
    }

    fn claim_quantum(&mut self) -> Result<(), ()> {
        self.get_frames(1)?;
        let q = self
            .global
            .available_quanta
            .remove(0, &mut self.rng)
            .ok_or(())
            .map_err(|_| eprintln!("out of virtual memory quanta"))?;
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
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let address = VirtAddr::from_ptr(ptr);
        self.global
            .released_quanta
            .insert(level, (address.as_u64() >> VIRTUAL_QUANTUM_BITS) as u32);
        let first_page = Page::<Size2MiB>::from_start_address(address).unwrap();
        for i in 0..frame_count {
            unsafe {
                self.available_frames
                    .push(unmap_huge_page(first_page + i as u64));
            }
        }
        self.global
            .available_frames
            .lock()
            .unwrap()
            .extend(self.available_frames.drain(..));
    }
}
