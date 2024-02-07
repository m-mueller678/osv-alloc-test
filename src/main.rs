use std::alloc::{GlobalAlloc, Layout};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::mem::{align_of, MaybeUninit, replace, size_of};
use std::ops::Add;
use std::ptr;
use std::time::Instant;
use ahash::RandomState;
use libc::*;
use rand::rngs::SmallRng;
use rand::prelude::*;
use rand::distributions::Uniform;
use x86_64::{PhysAddr, VirtAddr};
use x86_64::registers::control::Cr3;
use x86_64::structures::idt::ExceptionVector::Virtualization;
use x86_64::structures::paging::{FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTable, PageTableFlags, PhysFrame, Size2MiB, Size4KiB};
use x86_64::structures::paging::mapper::UnmapError;
use x86_64::structures::paging::page::PageRange;

mod PageMap;

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
        mmap(ptr::null_mut(), count * P::SIZE as usize, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | page_size_flags | init_flags, -1, 0) as *mut u8
    };
    assert!(!p.is_null());
    let p = Page::<P>::from_start_address(VirtAddr::from_ptr(p)).unwrap();
    Page::range(p, p + count as u64)
}

const KB: usize = 1 << 10;
const MB: usize = 1 << 20;
const GB: usize = MB << 10;
const TB: usize = GB << 10;

const PHYS_OFFSET: u64 = 0x0000400000000000;

fn phys_to_virt(p: PhysAddr) -> VirtAddr {
    VirtAddr::new(PHYS_OFFSET + p.as_u64())
}

const HUGE_PAGE_SIZE: usize = 2 * MB;

const VIRT_SIZE: usize = 1 * TB;
const PHYS_SIZE: usize = 2 * GB;

#[derive(Default)]
struct MmapFrameAllocator {
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
    OffsetPageTable::new(&mut *phys_to_virt(Cr3::read().0.start_address()).as_mut_ptr::<PageTable>(), VirtAddr::new(PHYS_OFFSET))
}

unsafe impl FrameAllocator<Size4KiB> for MmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.frames.pop()
    }
}

fn claim_frames<P: PageSize>(count: usize) -> impl Iterator<Item=PhysFrame<P>>
    where for<'a> OffsetPageTable<'a>: Mapper<P> {
    alloc_mmap::<P>(count, false).into_iter().map(|page| {
        unsafe { page.start_address().as_mut_ptr::<u8>().write(0); }
        unsafe { page_table() }.translate_page(page).unwrap()
    })
}

struct MappedPageInfo {
    frame: PhysFrame<Size2MiB>,
    allocation_count: u32,
}

struct PagingAllocator {
    frame_allocator: MmapFrameAllocator,
    available_frames: Vec<PhysFrame<Size2MiB>>,
    mapped_pages: HashMap<VirtAddr, MappedPageInfo,ahash::RandomState>,
    bump: VirtAddr,
    current_page: MappedPageInfo,
}

impl PagingAllocator{
    fn new()->Self{
        assert_eq!(PHYS_SIZE % HUGE_PAGE_SIZE, 0);
        assert_eq!(VIRT_SIZE % HUGE_PAGE_SIZE, 0);
        let phys_pages = alloc_mmap::<Size2MiB>(PHYS_SIZE / HUGE_PAGE_SIZE, false);
        for p in phys_pages {
            unsafe{
                p.start_address().as_mut_ptr::<u8>().write(0);
            }
        }
        let virt_pages = alloc_mmap::<Size2MiB>(VIRT_SIZE / HUGE_PAGE_SIZE, false);

        println!("mmap done");
        println!("unmapping virtual range pages");
        {
            let mut pt= unsafe{ page_table() };
            for p in virt_pages {
                match unsafe{page_table()}.unmap(p) {
                    Ok((f, flush)) => {
                        println!("unmapped {f:?} from virtual range");
                        flush.flush();
                    }
                    Err(UnmapError::PageNotMapped) => { continue; }
                    Err(e) => panic!("cannot unmap {p:?} in virtual range: {e:?}"),
                }
            }
        }

        println!("unmapping complete");
        let ret=PagingAllocator {
            frame_allocator: Default::default(),
            available_frames: phys_pages.into_iter().map(|p|  unsafe{page_table()}.translate_page(p).unwrap()).collect(),
            mapped_pages: HashMap::with_hasher(ahash::RandomState::with_seed(0xee61096f95490820)),
            bump: (virt_pages.last().unwrap() + 1).start_address(),
            current_page: MappedPageInfo { allocation_count: 0, frame: PhysFrame::containing_address(PhysAddr::new(0)) },
        };
        println!("allocator constructed");
        ret
    }
}

unsafe trait TestAlloc {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}

struct OsvAlloc;

unsafe impl TestAlloc for OsvAlloc{
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        malloc(layout.size()) as *mut u8
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        free(ptr as *mut c_void)
    }
}

unsafe impl TestAlloc for PagingAllocator {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        assert_ne!(layout.size(), 0);
        let old_page = Page::<Size2MiB>::containing_address(self.bump);
        let aligned_bump = VirtAddr::new(self.bump.as_u64() & !(layout.align() as u64 - 1));
        let max_page = Page::<Size2MiB>::containing_address(aligned_bump - 1u64);
        let new_bump = aligned_bump - layout.size();
        let min_page = Page::<Size2MiB>::containing_address(new_bump);
        if min_page == old_page {
            self.current_page.allocation_count += 1;
        } else {
            let required_frames = old_page - min_page;
            if self.available_frames.len() < required_frames as usize {
                panic!("out of frames");
            }
            if old_page == max_page {
                self.current_page.allocation_count += 1;
            }
            self.frame_allocator.refill();
            let mut pt = unsafe { page_table() };
            for pi in (1..=old_page - min_page).rev() {
                let store_page = min_page + pi;
                //println!("storing page {store_page:?} -> {:?}",self.current_page.frame);
                let was_none = self.mapped_pages.insert(store_page.start_address(), replace(&mut self.current_page, MappedPageInfo {
                    frame: self.available_frames.pop().unwrap(),
                    allocation_count: 1,
                })).is_none();
                //println!("mapping page {:?} -> {:?}",store_page-1,self.current_page.frame);
                pt.map_to(store_page - 1, self.current_page.frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE, &mut self.frame_allocator).unwrap().ignore();
                debug_assert!(was_none);
            }
        }
        self.bump = new_bump;
        self.bump.as_mut_ptr()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let start_addr = VirtAddr::from_ptr(ptr);
        let mut min_page = Page::<Size2MiB>::containing_address(start_addr);
        let max_page = Page::<Size2MiB>::containing_address(start_addr + layout.size() as u64 - 1u64);
        let current_page = Page::<Size2MiB>::containing_address(self.bump);
        if min_page == current_page {
            debug_assert!(self.current_page.allocation_count >= 1);
            self.current_page.allocation_count -= 1;
            if self.current_page.allocation_count == 0 {
                self.bump = current_page.start_address();
            }
            min_page += 1;
        }
        for p in Page::range_inclusive(min_page, max_page) {
            let Entry::Occupied(mut x) = self.mapped_pages.entry(p.start_address()) else {
                panic!("free on untracked page {p:?}");
            };
            let mapped = x.get_mut();
            debug_assert!(mapped.allocation_count >= 1);
            mapped.allocation_count -= 1;
            if mapped.allocation_count == 0 {
                self.available_frames.push(mapped.frame);
                //println!("freeing page{:?}->{:?}",p,mapped.frame);
                x.remove();
            }
        }
    }
}

fn pin() {
    unsafe {
        let mut cpu_set = MaybeUninit::<libc::cpu_set_t>::zeroed().assume_init();
        libc::CPU_ZERO(&mut cpu_set);
        libc::CPU_SET(0, &mut cpu_set);
        let s = libc::pthread_setaffinity_np(libc::pthread_self(), std::mem::size_of::<libc::cpu_set_t>(), &cpu_set);
        assert_eq!(s, 0);
    }
}

fn main() {
    pin();
    unsafe {
        println!("ours:");
        test_alloc::<true,false>(10_000_000, 64*KB, 2*GB, &mut PagingAllocator::new());
        println!("libc malloc:");
        test_alloc::<true,false>(10_000_000, 64*KB, 2*GB, &mut OsvAlloc);
    }
}

fn test_alloc<const VALIDATE: bool,const VALIDATE_FULL:bool>(total_allocs: usize, avg_alloc_size: usize, max_concurrent_size: usize, a: &mut impl TestAlloc) {
    let start = Instant::now();

    assert_eq!(avg_alloc_size % 8, 0);
    let concurrent_allocs =max_concurrent_size/(avg_alloc_size+avg_alloc_size/4);
    let mut rng = SmallRng::seed_from_u64(42);
    let mut allocs = VecDeque::with_capacity(concurrent_allocs);
    let avg_alloc_size = avg_alloc_size / 8;
    let size_range = Uniform::new(avg_alloc_size - avg_alloc_size / 4, avg_alloc_size + avg_alloc_size / 4);
    fn layout(l:usize)->Layout{
        Layout::from_size_align(l*size_of::<usize>(),align_of::<usize>()).unwrap()
    }
    unsafe {
        for i in 0..total_allocs {
            if allocs.len() == concurrent_allocs {
                let (ptr, len) = {
                    let x: &mut [MaybeUninit<usize>] = allocs.pop_front().unwrap();
                    let old_id = i - concurrent_allocs;
                    if VALIDATE {
                        assert!(
                            if VALIDATE_FULL{&x[..]}else{&x[..1]}.iter().all(|x| x.assume_init_read() == old_id)
                        )
                    }
                    (x.as_ptr() as *mut u8, x.len())
                };

                a.dealloc(ptr, layout(len));
            }
            let len = size_range.sample(&mut rng);
            let ptr = a.alloc(layout(len));
            let slice = std::slice::from_raw_parts_mut(ptr as *mut MaybeUninit<usize>,len);
            if VALIDATE{
                for x in if VALIDATE_FULL{&mut slice[..]}else{&mut slice[..1]}{
                    x.write(i);
                }
            }
            allocs.push_back(slice);
        }
    }

    let end = Instant::now();
    let duration = end-start;
    println!("complete. {:.3e} alloc/s", total_allocs as f64 / duration.as_secs_f64());
}
