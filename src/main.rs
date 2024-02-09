use crate::myalloc::LocalData;
use libc::*;
use rand::distributions::Uniform;
use rand::prelude::*;
use rand::rngs::SmallRng;
use std::alloc::{GlobalAlloc, Layout};
use std::collections::VecDeque;
use std::mem::{align_of, size_of, MaybeUninit};
use std::ops::Range;
use std::ptr;
use std::sync::Barrier;
use std::thread::scope;
use std::time::Instant;
use tikv_jemallocator::Jemalloc;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page::PageRange;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTable, PhysFrame, Size2MiB,
    Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

pub mod buddymap;
pub mod frame_list;
pub mod log_alloc;
pub mod myalloc;
pub mod no_frame_allocator;
pub mod page_map;
pub mod paging;

// from osv/libs/mman.cc
const MAP_UNINITIALIZED: i32 = 0x4000000;

fn alloc_mmap<P: PageSize>(count: usize, zeroed: bool) -> PageRange<P> {
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
const MB: usize = KB << 10;
const GB: usize = MB << 10;
const TB: usize = GB << 10;

const PHYS_OFFSET: u64 = 0x0000400000000000;

fn phys_to_virt(p: PhysAddr) -> VirtAddr {
    VirtAddr::new(PHYS_OFFSET + p.as_u64())
}

#[derive(Default)]
pub struct MmapFrameAllocator {
    frames: Vec<PhysFrame>,
}

impl MmapFrameAllocator {
    fn refill(&mut self) {
        if self.frames.len() < 8 {
            self.frames.extend(claim_frames(8))
        }
    }
}

unsafe fn page_table<'a>() -> OffsetPageTable<'a> {
    OffsetPageTable::new(
        &mut *phys_to_virt(Cr3::read().0.start_address()).as_mut_ptr::<PageTable>(),
        VirtAddr::new(PHYS_OFFSET),
    )
}

unsafe impl FrameAllocator<Size4KiB> for MmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.frames.pop()
    }
}

fn claim_frames<P: PageSize>(count: usize) -> impl Iterator<Item = PhysFrame<P>>
where
    for<'a> OffsetPageTable<'a>: Mapper<P>,
{
    alloc_mmap::<P>(count, false).into_iter().map(|page| {
        unsafe {
            page.start_address().as_mut_ptr::<u8>().write(0);
        }
        unsafe { page_table() }.translate_page(page).unwrap()
    })
}

unsafe trait TestAlloc: Send {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}

#[derive(Clone)]
struct LibCAlloc;

unsafe impl TestAlloc for LibCAlloc {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        malloc(layout.size()) as *mut u8
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
        free(ptr as *mut c_void)
    }
}

unsafe impl TestAlloc for Jemalloc {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        GlobalAlloc::alloc(self, layout)
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        GlobalAlloc::dealloc(self, ptr, layout)
    }
}

fn pin() {
    unsafe {
        let mut cpu_set = MaybeUninit::<libc::cpu_set_t>::zeroed().assume_init();
        libc::CPU_ZERO(&mut cpu_set);
        libc::CPU_SET(0, &mut cpu_set);
        let s = libc::pthread_setaffinity_np(
            libc::pthread_self(),
            std::mem::size_of::<libc::cpu_set_t>(),
            &cpu_set,
        );
        assert_eq!(s, 0);
    }
}

fn main() {
    let kernel_version = std::fs::read("/proc/version").unwrap_or_default();
    let kernel_version = String::from_utf8_lossy(&kernel_version);
    println!("/proc/version: {kernel_version:?}");
    pin();
    let mut allocs: Vec<String> = std::env::args().skip(1).collect();
    if allocs.is_empty() {
        allocs = vec!["libc".into(), "jemalloc".into(), "ours".into()]
    }

    let test_mode = AllocTestMode::First;
    let threads = 1;
    let phys_size = 128 * MB;
    let virt_size = 1 * TB;
    let max_use = phys_size - phys_size / 4;
    let avg_alloc_size = 20 * MB;
    let alloc_per_thread = 10_000;

    for alloc in allocs {
        println!("{alloc}:");
        match alloc.as_str() {
            "ours" => {
                test_alloc(
                    alloc_per_thread,
                    avg_alloc_size,
                    max_use,
                    test_mode,
                    &mut LocalData::create(threads, phys_size, virt_size),
                );
            }
            "jemalloc" => test_alloc(
                alloc_per_thread,
                avg_alloc_size,
                max_use,
                test_mode,
                &mut vec![Jemalloc; threads],
            ),
            "libc" => test_alloc(
                alloc_per_thread,
                avg_alloc_size,
                max_use,
                test_mode,
                &mut vec![LibCAlloc; threads],
            ),
            _ => panic!("bad allocator name: {alloc}"),
        }
    }
}

#[derive(Clone, Copy)]
enum AllocTestMode {
    None,
    First,
    Full,
}

impl AllocTestMode {
    fn index_range(self, size: usize) -> Range<usize> {
        match self {
            AllocTestMode::None => 0..0,
            AllocTestMode::First => 0..1,
            AllocTestMode::Full => 0..size,
        }
    }
}

fn test_alloc<A: TestAlloc>(
    allocs_per_thread: usize,
    avg_alloc_size: usize,
    max_concurrent_size: usize,
    mode: AllocTestMode,
    allocs: &mut [A],
) {
    fn layout(l: usize) -> Layout {
        Layout::from_size_align(l * size_of::<usize>(), align_of::<usize>()).unwrap()
    }

    assert_eq!(avg_alloc_size % 8, 0);
    let concurrent_allocs_per_thread =
        max_concurrent_size / (avg_alloc_size + avg_alloc_size / 4) / allocs.len();
    let avg_alloc_size = avg_alloc_size / 8;
    let size_range = Uniform::new(
        avg_alloc_size - avg_alloc_size / 4,
        avg_alloc_size + avg_alloc_size / 4,
    );

    let barrier = &Barrier::new(allocs.len() + 1);

    let duration = scope(|s| {
        for (tid, a) in allocs.iter_mut().enumerate() {
            s.spawn(move || {
                let mut rng = SmallRng::seed_from_u64(tid as u64);
                let mut allocs = VecDeque::with_capacity(concurrent_allocs_per_thread);
                let mut next_id = (tid * allocs_per_thread) << 16;

                unsafe {
                    while allocs.len() < concurrent_allocs_per_thread {
                        if allocs.len() >= 102500 {
                            dbg!(allocs.len() * avg_alloc_size * 8);
                        }
                        let size = size_range.sample(&mut rng);
                        let ptr = a.alloc(layout(size)) as *mut usize;
                        assert!(!ptr.is_null());
                        if allocs.len() >= 102500 {
                            dbg!(ptr);
                        }
                        for i in mode.index_range(size) {
                            ptr.add(i).write(next_id + i);
                        }
                        next_id += 1 << 16;
                        allocs.push_back((ptr, size))
                    }

                    barrier.wait();
                    barrier.wait();
                    for _ in 0..allocs_per_thread {
                        {
                            let (ptr, size) = allocs.pop_front().unwrap();
                            let expected_id = next_id - (concurrent_allocs_per_thread << 16);
                            for i in mode.index_range(size) {
                                assert_eq!(ptr.add(i).read(), expected_id + i);
                            }
                            a.dealloc(ptr as *mut u8, layout(size))
                        }
                        {
                            let size = size_range.sample(&mut rng);
                            let ptr = a.alloc(layout(size)) as *mut usize;
                            assert!(!ptr.is_null());
                            for i in mode.index_range(size) {
                                ptr.add(i).write(next_id + i);
                            }
                            next_id += 1 << 16;
                            allocs.push_back((ptr, size))
                        }
                    }

                    barrier.wait();

                    while let Some((ptr, size)) = allocs.pop_front() {
                        let expected_id = next_id - (concurrent_allocs_per_thread << 16);
                        for i in mode.index_range(size) {
                            assert_eq!(ptr.add(i).read(), expected_id + i);
                        }
                        a.dealloc(ptr as *mut u8, layout(size));
                        next_id += 1 << 16;
                    }
                }
            });
        }
        barrier.wait();
        let start = Instant::now();
        barrier.wait();
        barrier.wait();
        let end = Instant::now();
        end - start
    });

    println!(
        "complete. {:.3e} alloc/s/thread",
        allocs_per_thread as f64 / duration.as_secs_f64()
    );
}
