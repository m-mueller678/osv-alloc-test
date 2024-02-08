use x86_64::structures::paging::{FrameAllocator, PageSize, PhysFrame};

struct NoFrameAllocator;

unsafe impl<S: PageSize> FrameAllocator<S> for NoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>> {
        None
    }
}
