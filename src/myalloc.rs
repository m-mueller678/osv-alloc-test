use std::cell::RefCell;
use x86_64::structures::paging::PhysFrame;
use x86_64::VirtAddr;

struct MyAlloc;
struct LocalData{
    available_frames:Vec<PhysFrame>,
    bump:VirtAddr,
}

thread_local! {
    //static LOCAL: LocalData=RefCell::new(LocalData::new());
}

impl LocalData{
    fn new()->Self{
        unimplemented!()
    }

}