use crate::frame_list::FrameListTrait;
use x86_64::structures::paging::{PageSize, PhysFrame, Size2MiB, Size4KiB};

pub struct FrameList<S: PageSize, const C: usize>(Box<Vec<PhysFrame<S>>>);

pub type FrameList4K = FrameList<Size4KiB, { 512 - 2 }>;
pub type FrameList2M = FrameList<Size2MiB, { 512 * 512 - 2 }>;

impl<S: PageSize, const C: usize> Default for FrameList<S, C> {
    fn default() -> Self {
        FrameList(Default::default())
    }
}

impl<S: PageSize, const C: usize> FrameListTrait<S> for FrameList<S, C> {
    fn push(&mut self, f: PhysFrame<S>) {
        self.0.push(f);
    }
    fn pop(&mut self) -> Option<PhysFrame<S>> {
        self.0.pop()
    }
    fn merge_into_vec(&mut self, dst: &mut Vec<PhysFrame<S>>) {
        dst.extend(self.0.drain(..));
    }
    fn steal_from_vec(&mut self, src: &mut Vec<PhysFrame<S>>, count: usize) -> Result<(), ()> {
        if src.len() < count {
            return Err(());
        }
        self.0.extend(src.drain(src.len() - count..));
        Ok(())
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
