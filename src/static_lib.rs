use crate::myalloc::{GlobalData, LocalData};
use crate::TestAlloc;
use libc::abort;
use once_cell::unsync::Lazy;
use static_assertions::{assert_eq_align, assert_eq_size, assert_type_eq_all};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::OnceCell;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, UnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Once};

type PtrLocalData = LocalData<&'static GlobalData>;

assert_eq_align!(PtrLocalData, [u64; 11]);
assert_eq_size!(PtrLocalData, [u64; 11]);

assert_eq_align!(GlobalData, [u64; 409]);
assert_eq_size!(GlobalData, [u64; 409]);

#[no_mangle]
pub unsafe extern "C" fn virtual_alloc_init_global(
    dst: *mut GlobalData,
    physical_size: u64,
    virtual_size: u64,
) {
    catch(|| {
        dst.write(GlobalData::new(
            physical_size as usize,
            virtual_size as usize,
        ));
    })
}

#[no_mangle]
pub unsafe extern "C" fn virtual_alloc_create_handle(
    dst: *mut PtrLocalData,
    global: *const GlobalData,
    seed: u64,
) -> bool {
    catch(|| match LocalData::new(seed, &*global) {
        Ok(x) => {
            dst.write(x);
            true
        }
        Err(()) => false,
    })
}

#[no_mangle]
pub unsafe extern "C" fn virtual_alloc_alloc(
    local: *mut PtrLocalData,
    size: u64,
    align: u64,
) -> *mut libc::c_void {
    catch(|| {
        (*local).alloc(Layout::from_size_align_unchecked(
            size as usize,
            align as usize,
        )) as *mut libc::c_void
    })
}

#[no_mangle]
pub unsafe extern "C" fn virtual_alloc_free(
    local: *mut PtrLocalData,
    size: u64,
    align: u64,
    ptr: *mut libc::c_void,
) {
    catch(|| {
        (*local).dealloc(
            ptr as *mut u8,
            Layout::from_size_align_unchecked(size as usize, align as usize),
        )
    })
}

fn catch<B, F: FnOnce() -> B + UnwindSafe>(f: F) -> B {
    match catch_unwind(f) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("rust panicked: {e:?}");
            std::process::abort()
        }
    }
}
