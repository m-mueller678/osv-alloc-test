use crate::frame_list::{FrameList, FrameList2M};
use crate::myalloc::large_allocator::{alloc_large, dealloc_large};
use crate::myalloc::medium_allocator::MediumAllocator;
use crate::myalloc::quantum_storage::QuantumStorage;
use crate::myalloc::small_allocator::SmallAllocator;
use crate::quantum_address::QuantumAddress;
use crate::util::{PAGE_SIZE, VIRTUAL_QUANTUM_SIZE};
use crate::{SystemInterface, TestAlloc};
use rand::rngs::SmallRng;
use rand::SeedableRng;
use std::alloc::Layout;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::Mutex;
use x86_64::structures::paging::page::PageRangeInclusive;
use x86_64::structures::paging::{Page, PageSize, PhysFrame, Size2MiB};

mod large_allocator;
mod medium_allocator;
mod quantum_storage;
mod small_allocator;

pub struct GlobalData<S: SystemInterface> {
    available_frames: Mutex<Vec<PhysFrame<Size2MiB>, S::Alloc>>,
    quantum_storage: QuantumStorage<S>,
    sys: S,
}

impl<S: SystemInterface> GlobalData<S> {
    pub fn new(sys: S, physical_size: usize, virt_size: usize) -> Self {
        assert!(virt_size.is_multiple_of(VIRTUAL_QUANTUM_SIZE));
        assert!(physical_size.is_multiple_of(Size2MiB::SIZE as usize));
        let frame_count = physical_size / Size2MiB::SIZE as usize;
        let page_count = virt_size / Size2MiB::SIZE as usize;
        let virt_start = Page::from_start_address(
            sys.allocate_virtual(Layout::from_size_align(virt_size, VIRTUAL_QUANTUM_SIZE).unwrap()),
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
            quantum_storage: {
                let start =
                    QuantumAddress::from_start(virt_start.start_address().as_u64() as usize);
                let end = QuantumAddress::from_start(virt_end.start_address().as_u64() as usize);
                QuantumStorage::from_range(sys, start..end)
            },
            available_frames: Mutex::new(frames),
            sys,
        }
    }
}

pub struct LocalData<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> {
    common: LocalCommon<S, G>,
    small: SmallAllocator<S, G>,
    medium: MediumAllocator<S, G>,
}

struct LocalCommon<S: SystemInterface, G: Deref<Target = GlobalData<S>>> {
    global: G,
    rng: SmallRng,
    available_frames: FrameList2M<S>,
}

const MAX_MEDIUM_SIZE: usize = (VIRTUAL_QUANTUM_SIZE * PAGE_SIZE).isqrt();
const MAX_SMALL_SIZE: usize = PAGE_SIZE / 16;

unsafe impl<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> TestAlloc
    for LocalData<S, G>
{
    #[inline]
    unsafe fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if std::hint::likely(layout.size() <= MAX_SMALL_SIZE) {
            if std::hint::likely(layout.size() != 0) {
                self.small.alloc(&mut self.common, layout)
            } else {
                Some(NonNull::new_unchecked(layout.dangling().as_ptr()))
            }
        } else if std::hint::likely(layout.size() < MAX_MEDIUM_SIZE) {
            panic!();
            self.medium.alloc(&mut self.common, layout)
        } else {
            panic!();
            alloc_large(&mut self.common, layout)
        }
    }

    #[inline]
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, size: usize) {
        if std::hint::likely(size <= MAX_SMALL_SIZE) {
            if std::hint::likely(size != 0) {
                SmallAllocator::dealloc(&mut self.common, ptr.as_ptr());
            }
        } else if std::hint::likely(size < MAX_MEDIUM_SIZE) {
            MediumAllocator::dealloc(&mut self.common, ptr.as_ptr(), size);
        } else {
            dealloc_large(&mut self.common, ptr.as_ptr(), size);
        }
    }
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> LocalData<S, G> {
    pub fn new(seed: u64, global: G) -> Self {
        LocalData {
            common: LocalCommon {
                available_frames: FrameList::new(global.sys),
                global,
                rng: SmallRng::seed_from_u64(seed),
            },
            small: SmallAllocator::new(),
            medium: MediumAllocator::new(),
        }
    }
}
impl<S: SystemInterface, G: Deref<Target = GlobalData<S>> + Send> Drop for LocalData<S, G> {
    fn drop(&mut self) {
        self.small.deinit(&mut self.common);
        self.medium.deinit(&mut self.common);
    }
}
