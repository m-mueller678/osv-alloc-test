use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

#[derive(Default)]
pub struct MmapFrameAllocator {
    frames: Vec<PhysFrame>,
}

impl MmapFrameAllocator {
    pub fn refill(&mut self) {
        if self.frames.len() < 8 {
            self.frames.extend(crate::util::claim_frames(8))
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for MmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.frames.pop()
    }
}
