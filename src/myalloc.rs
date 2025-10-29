use crate::frame_list::{FrameList, FrameList2M};
use crate::myalloc::quantum_storage::QuantumStorage;
use crate::page_map::{PageMap, SmallCountHashMap};
use crate::{SystemInterface, TestAlloc};
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::alloc::Layout;
use std::ops::Deref;
use std::ptr;
use std::sync::Mutex;
use tracing::error;
use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::{Page, PageSize, PhysFrame, Size2MiB};
use x86_64::{align_down, VirtAddr};

const VIRTUAL_QUANTUM_BITS: u32 = 24;
const VIRTYAL_QUANTUM_SIZE: usize = 1 << VIRTUAL_QUANTUM_BITS;
const MAX_MID_SIZE: usize = 16 << 20;
const ADDRESS_BIT_MASK: u64 = (!0u64) >> 16;

fn address_to_quantum(a: VirtAddr) -> u32 {
    ((a.as_u64() & ADDRESS_BIT_MASK) >> VIRTUAL_QUANTUM_BITS) as u32
}

mod quantum_storage;

pub struct GlobalData<S: SystemInterface> {
    allocs_per_page: PageMap<S::Alloc>,
    pages_per_quantum: SmallCountHashMap<
        u32,
        S::Alloc,
        { VIRTUAL_QUANTUM_BITS + 1 - 21 },
        0,
        { 48 - VIRTUAL_QUANTUM_BITS },
    >,
    available_frames: Mutex<Vec<PhysFrame<Size2MiB>, S::Alloc>>,
    quantum_storage: QuantumStorage<S>,
    sys: S,
}

impl<S: SystemInterface> GlobalData<S> {
    #[must_use]
    fn map_and_insert(
        &self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        count: usize,
    ) -> usize {
        unsafe {
            self.sys.map(page, frame);
        }
        self.allocs_per_page.insert(page, frame, count)
    }

    fn decrement_quantum(&self, q: u32) {
        if self.pages_per_quantum.decrement(q).is_some() {
            self.quantum_storage.dealloc_dirty(0, q)
        }
    }

    pub fn new(sys: S, physical_size: usize, virt_size: usize) -> Self {
        assert!(virt_size.is_multiple_of(VIRTYAL_QUANTUM_SIZE));
        assert!(physical_size.is_multiple_of(Size2MiB::SIZE as usize));
        let frame_count = physical_size / Size2MiB::SIZE as usize;
        let page_count = virt_size / Size2MiB::SIZE as usize;
        let virt_start = Page::from_start_address(
            sys.allocate_virtual(Layout::from_size_align(virt_size, VIRTYAL_QUANTUM_SIZE).unwrap()),
        )
        .unwrap();
        let virt_end = virt_start + page_count as u64;

        assert!(virt_size <= 1 << 46);

        let mut frames = Vec::with_capacity_in(frame_count, sys.allocator());
        for _ in 0..frame_count {
            frames.push(
                PhysFrame::<Size2MiB>::from_start_address(
                    sys.allocate_physical(
                        Layout::from_size_align(Size2MiB::SIZE as usize, Size2MiB::SIZE as usize)
                            .unwrap(),
                    ),
                )
                .unwrap(),
            )
        }
        unsafe {
            sys.prepare_page_table(PageRangeInclusive {
                start: virt_start,
                end: virt_end - 1,
            })
        };

        GlobalData {
            allocs_per_page: PageMap::new_in(
                frame_count + frame_count / 4,
                virt_start,
                sys.allocator(),
            ),
            pages_per_quantum: SmallCountHashMap::with_num_slots_in(1 << 16, sys.allocator()),
            quantum_storage: {
                let start = ((virt_start.start_address().as_u64() & ADDRESS_BIT_MASK)
                    >> VIRTUAL_QUANTUM_BITS) as u32;
                let end = ((virt_end.start_address().as_u64() & ADDRESS_BIT_MASK)
                    >> VIRTUAL_QUANTUM_BITS) as u32;
                QuantumStorage::from_range(sys, start..end)
            },
            available_frames: Mutex::new(frames),
            sys,
        }
    }
}

pub struct LocalData<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> {
    rng: SmallRng,
    available_frames: FrameList2M<S>,
    // these are sign extended virtual addresses. be careful around the half of the address space
    min_address: u64,
    bump: u64,
    current_page_index: usize,
    current_page: Page<Size2MiB>,
    current_quantum_index: usize,
    global: G,
}

fn wrapping_less_than(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) as i64) < 0
}

unsafe impl<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> TestAlloc
    for LocalData<S, G>
{
    #[inline]
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.dangling().as_ptr();
        }
        if layout.size() > MAX_MID_SIZE {
            return self.alloc_large(layout);
        }
        let size = layout.size() as u64;
        let new_bump = self.bump.wrapping_sub(size) & !(layout.align() as u64 - 1);
        if wrapping_less_than(new_bump, self.min_address) {
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
            let max_page =
                Page::<Size2MiB>::containing_address(VirtAddr::new(new_bump + size) - 1u64);
            let required_frames = self.current_page - min_page;
            if self.get_frames(required_frames as usize).is_err() {
                error!(size = layout.size(), "allocation failed");
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

    #[inline]
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        if std::hint::unlikely(layout.size() == 0) {
            return;
        }
        if std::hint::unlikely(layout.size() > MAX_MID_SIZE) {
            return self.dealloc_large(ptr, layout);
        }
        let start_addr = ptr.addr();
        let end_addr = start_addr + layout.size();
        let mut page = align_down(start_addr as u64, Size2MiB::SIZE);
        loop {
            self.decrement_page(Page::from_start_address_unchecked(VirtAddr::new_unsafe(
                page,
            )));
            page += Size2MiB::SIZE;
            if page >= end_addr as u64 {
                break;
            }
        }
        self.release_frames();
    }
}

unsafe fn panic_or_ub<A, B>(a: A) -> B {
    if cfg!(debug_assertions) {
        panic!()
    } else {
        std::hint::unreachable_unchecked()
    }
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> LocalData<S, G> {
    fn get_frames(&mut self, count: usize) -> Result<(), ()> {
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
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let Some(quantum) = self.global.quantum_storage.alloc(level, &mut self.rng) else {
            return ptr::null_mut();
        };
        if self.get_frames(frame_count).is_err() {
            self.global.quantum_storage.dealloc_clean(level, quantum);
            error!(size = layout.size(), "allocation failed");
            return ptr::null_mut();
        }
        let first_page = Page::<Size2MiB>::containing_address(VirtAddr::new(
            (quantum as u64) << VIRTUAL_QUANTUM_BITS,
        ));
        for i in 0..frame_count {
            unsafe {
                self.global
                    .sys
                    .map(first_page + i as u64, self.available_frames.pop().unwrap());
            }
        }
        first_page.start_address().as_mut_ptr()
    }

    pub fn new(seed: u64, global: G) -> Result<Self, ()> {
        let mut r = LocalData {
            rng: SmallRng::seed_from_u64(seed),
            available_frames: FrameList::new(global.sys),
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
        debug_assert!(layout.align() <= (1 << 21));
        let (level, frame_count) = Self::large_alloc_info(layout.size());
        let address = VirtAddr::from_ptr(ptr);
        let first_page = Page::<Size2MiB>::from_start_address(address).unwrap();
        for i in 0..frame_count {
            unsafe {
                self.available_frames
                    .push(self.global.sys.unmap(first_page + i as u64));
            }
        }
        self.release_frames();
        self.global
            .quantum_storage
            .dealloc_dirty(level, address_to_quantum(address));
    }

    fn release_frames(&mut self) {
        if !self.available_frames.is_empty() {
            self.available_frames
                .merge_into_vec(&mut self.global.available_frames.lock().unwrap());
        }
    }

    fn decrement_page(&mut self, p: Page<Size2MiB>) {
        if let Some(x) = self.global.allocs_per_page.decrement(p) {
            self.available_frames.push(x);
            unsafe { self.global.sys.unmap(p) };
            self.global
                .decrement_quantum(address_to_quantum(p.start_address()))
        }
    }
}
