use crate::page_map::PageMap;
use crate::paging::{allocate_l2_tables, map_huge_page};
use crate::{alloc_mmap, page_table, MmapFrameAllocator, TestAlloc, PHYS_OFFSET};
use std::alloc::Layout;
use std::ptr;
use std::sync::{Arc, Mutex};
use x86_64::structures::paging::mapper::UnmapError;
use x86_64::structures::paging::{Mapper, Page, PageSize, PhysFrame, Size2MiB};
use x86_64::VirtAddr;

struct GlobalData {
    mapped_pages: PageMap,
    available_frames: Mutex<Vec<PhysFrame<Size2MiB>>>,
}

impl GlobalData {
    unsafe fn map_and_insert(
        &self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        count: usize,
    ) -> usize {
        map_huge_page(page, frame);
        self.mapped_pages.insert(page, frame, count)
    }
}

#[derive(Clone)]
pub struct LocalData {
    available_frames: Vec<PhysFrame<Size2MiB>>,
    min_address: VirtAddr,
    bump: VirtAddr,
    current_page_index: usize,
    current_page: Page<Size2MiB>,
    global: Arc<GlobalData>,
}

unsafe impl TestAlloc for LocalData {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return VirtAddr::new(PHYS_OFFSET).as_mut_ptr();
        }
        let aligned_bump = VirtAddr::new(self.bump.as_u64() & !(layout.align() as u64 - 1));
        let new_bump = aligned_bump - layout.size();
        assert!(new_bump >= self.min_address);
        let min_page = Page::<Size2MiB>::containing_address(new_bump);
        if min_page == self.current_page {
            self.global
                .mapped_pages
                .increment_at(self.current_page_index, self.current_page);
        } else {
            let max_page = Page::<Size2MiB>::containing_address(aligned_bump - 1u64);
            let required_frames = self.current_page - min_page;
            if self.get_frames(required_frames as usize).is_err() {
                eprintln!("out of memory");
                return ptr::null_mut();
            }
            let mut freed_frame = None;
            if max_page != self.current_page {
                freed_frame = self.global.mapped_pages.decrement(self.current_page);
            }
            for p in Page::range(min_page, self.current_page).skip(1) {
                self.global
                    .map_and_insert(p, self.available_frames.pop().unwrap(), 1);
            }
            self.current_page = min_page;
            self.current_page_index =
                self.global
                    .map_and_insert(min_page, self.available_frames.pop().unwrap(), 2);
            if let Some(p) = freed_frame {
                self.global.available_frames.lock().unwrap().push(p);
            }
            debug_assert!(self.available_frames.is_empty());
        }
        self.bump = new_bump;
        self.bump.as_mut_ptr()
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        debug_assert!(self.available_frames.is_empty());
        if layout.size() == 0 {
            return;
        }
        let start_addr = VirtAddr::from_ptr(ptr);
        let min_page = Page::<Size2MiB>::containing_address(start_addr);
        let max_page =
            Page::<Size2MiB>::containing_address(start_addr + layout.size() as u64 - 1u64);
        self.global.available_frames.lock().unwrap().extend(
            Page::range_inclusive(min_page, max_page)
                .filter_map(|p| self.global.mapped_pages.decrement(p))
                .inspect(|f| {}),
        );
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

    pub fn create(
        threads: usize,
        physical_size: usize,
        virtual_size_per_thread: usize,
    ) -> Vec<Self> {
        assert_eq!(physical_size % Size2MiB::SIZE as usize, 0);
        assert_eq!(virtual_size_per_thread % Size2MiB::SIZE as usize, 0);
        let pages_per_thread = virtual_size_per_thread / Size2MiB::SIZE as usize;
        let total_pages = pages_per_thread * threads;

        let phys_pages = alloc_mmap::<Size2MiB>(physical_size / Size2MiB::SIZE as usize, false);
        for p in phys_pages {
            unsafe {
                p.start_address().as_mut_ptr::<u8>().write(0);
            }
        }
        let virt_pages = alloc_mmap::<Size2MiB>(total_pages, false);
        unsafe {
            let mut frame_allocator = MmapFrameAllocator::default();
            allocate_l2_tables(
                Page::range_inclusive(virt_pages.start, virt_pages.end - 1),
                &mut frame_allocator,
            );
        }
        println!("mmap done");
        println!("unmapping virtual range pages");
        {
            let _pt = unsafe { page_table() };
            for p in virt_pages {
                match unsafe { page_table() }.unmap(p) {
                    Ok((f, flush)) => {
                        println!("unmapped {f:?} from virtual range");
                        flush.flush();
                    }
                    Err(UnmapError::PageNotMapped) => {
                        continue;
                    }
                    Err(e) => panic!("cannot unmap {p:?} in virtual range: {e:?}"),
                }
            }
        }
        println!("unmapping complete");
        dbg!(virt_pages.end - virt_pages.start, virt_pages);
        let global = Arc::new(GlobalData {
            mapped_pages: PageMap::with_num_slots(
                phys_pages.count() + phys_pages.count() / 4,
                virt_pages.start,
            ),
            available_frames: Mutex::new(
                phys_pages
                    .into_iter()
                    .map(|p| unsafe { page_table() }.translate_page(p).unwrap())
                    .collect(),
            ),
        });

        let ret = (0..threads)
            .map(|i| {
                let end_page = virt_pages.start + (i as u64 + 1) * pages_per_thread as u64;
                LocalData {
                    available_frames: Vec::new(),
                    min_address: (virt_pages.start + i as u64 * pages_per_thread as u64)
                        .start_address(),
                    bump: end_page.start_address(),
                    current_page_index: unsafe {
                        global.map_and_insert(
                            end_page - 1,
                            global.available_frames.lock().unwrap().pop().unwrap(),
                            1,
                        )
                    },
                    current_page: end_page - 1,
                    global: global.clone(),
                }
            })
            .collect();
        println!("allocator constructed");
        ret
    }
}
