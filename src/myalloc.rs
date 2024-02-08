use std::alloc::Layout;
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::replace;
use std::ops::Add;
use std::ptr;
use std::sync::{Arc, Mutex};
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size2MiB};
use x86_64::VirtAddr;
use crate::{MmapFrameAllocator, page_table, PHYS_OFFSET};
use crate::page_map::PageMap;

struct MyAlloc;

struct GlobalData {
    mapped_pages: PageMap,
    available_frames: Mutex<Vec<PhysFrame>>,
}


struct MappedPageInfo {
    frame: PhysFrame<Size2MiB>,
    allocation_count: usize,
}

struct LocalData {
    available_frames: Vec<PhysFrame<Size2MiB>>,
    min_address: VirtAddr,
    max_address: VirtAddr,
    bump: VirtAddr,
    current_page: Page<Size2MiB>,
    current_page_info: MappedPageInfo,
    global: Arc<GlobalData>,
}

impl LocalData {
    fn get_frames(&mut self, count: usize) -> Result<(), ()> {
        assert!(self.available_frames == 0);
        let mut gf = self.global.available_frames.lock().unwrap();
        if gf.len() < count {
            return Err(());
        }
        self.available_frames.extend_from_slice(| &gf[gf.len() - count..]);
        gf.truncate(gf.len() - count);
        Ok(())
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return VirtAddr::new(PHYS_OFFSET).as_mut_ptr();
        }
        let aligned_bump = VirtAddr::new(self.bump.as_u64() & !(layout.align() as u64 - 1));
        let max_page = Page::<Size2MiB>::containing_address(aligned_bump - 1u64);
        let new_bump = aligned_bump - layout.size();
        let min_page = Page::<Size2MiB>::containing_address(new_bump);
        if min_page == self.current_page {
            self.current_page_info.allocation_count += 1;
        } else {
            let required_frames = self.current_page - min_page;
            if self.get_frames(required_frames as usize).is_err() {
                return ptr::null_mut();
            }
            if self.current_page == max_page {
                self.current_page_info.allocation_count += 1;
            }
            while self.current_page > min_page{
                self.global.mapped_pages.insert(self.current_page, self.current_page_info.frame, self.current_page_info.allocation_count);
                self.current_page_info.frame=self.available_frames.pop().unwrap();
                self.current_page_info.allocation_count=1;
            }
            for pi in (1..=self.current_page - min_page).rev() {
                self.
                //println!("storing page {store_page:?} -> {:?}",self.current_page.frame);


                self.current_page_info.allocation_count=
                //println!("mapping page {:?} -> {:?}",store_page-1,self.current_page.frame);
                pt.map_to(store_page - 1,self.current_page.frame,PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,&mut self.frame_allocator).unwrap().ignore();
                debug_assert!(was_none);
            }
        }
        self.bump = new_bump;
        self.bump.as_mut_ptr()
    }
}

impl LocalData {
    fn new() -> Self {
        unimplemented!()
    }
}