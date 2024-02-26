use x86_64::structures::paging::{PageSize, PhysFrame};

pub mod frame_list_no_mem;
pub mod frame_list_real;

#[cfg(feature = "no_mem")]
pub use frame_list_no_mem::*;

#[cfg(not(feature = "no_mem"))]
pub use frame_list_real::*;

pub trait FrameListTrait<S: PageSize> {
    fn push(&mut self, f: PhysFrame<S>);
    fn pop(&mut self) -> Option<PhysFrame<S>>;
    fn merge_into_vec(&mut self, dst: &mut Vec<PhysFrame<S>>);
    fn steal_from_vec(&mut self, src: &mut Vec<PhysFrame<S>>, count: usize) -> Result<(), ()>;
    fn is_empty(&self) -> bool;
}
