#![allow(clippy::result_unit_err)]
#![allow(clippy::missing_safety_doc)]

use libc::*;
use rand::distributions::Uniform;
use rand::prelude::*;
use rand::rngs::SmallRng;
use std::alloc::Layout;
use std::collections::VecDeque;
use std::mem::{align_of, size_of, MaybeUninit};
use std::ops::Range;
use std::sync::{Arc, Barrier};
use std::thread::scope;
use std::time::Instant;
use tikv_jemallocator::Jemalloc;
use virtual_alloc::myalloc::{GlobalData, LocalData};
use virtual_alloc::profiling::profiling_tick;
use virtual_alloc::util::{GB, MB, TB};
use virtual_alloc::TestAlloc;

pub mod log_alloc;

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
    virtual_alloc::profiling::init_profiling();

    let kernel_version = std::fs::read("/proc/version").unwrap_or_default();
    let kernel_version = String::from_utf8_lossy(&kernel_version);
    println!("/proc/version: {kernel_version:?}");
    pin();
    let mut allocs: Vec<String> = std::env::args().skip(1).collect();
    if allocs.is_empty() {
        allocs = vec!["libc".into(), "jemalloc".into(), "ours".into()]
    }

    // 1.664e5, 1.519e5, 1.524e5
    let test_mode = AllocTestMode::First;
    let threads = 2;
    let phys_size = 4 * GB;
    let virt_size = TB;
    let max_use = phys_size - phys_size / 4;
    let avg_alloc_size = 16 * MB;
    let alloc_per_thread = 100_000_000_000;

    for alloc in allocs {
        println!("{alloc}:");
        match alloc.as_str() {
            "ours" => {
                test_alloc(alloc_per_thread, avg_alloc_size, max_use, test_mode, &mut {
                    let global = Arc::new(GlobalData::new(phys_size, virt_size));
                    (0..threads as u64)
                        .map(|i| LocalData::new(i, global.clone()).unwrap())
                        .collect::<Vec<_>>()
                });
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
#[allow(dead_code)]
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
                        profiling_tick();
                        let size = size_range.sample(&mut rng);
                        let ptr = a.alloc(layout(size)) as *mut usize;
                        assert!(!ptr.is_null());
                        for i in mode.index_range(size) {
                            ptr.add(i).write(next_id + i);
                        }
                        next_id += 1 << 16;
                        allocs.push_back((ptr, size))
                    }

                    barrier.wait();
                    barrier.wait();
                    for _ in 0..allocs_per_thread {
                        profiling_tick();
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
                            allocs.push_back((ptr, size));
                        }
                    }

                    barrier.wait();

                    while let Some((ptr, size)) = allocs.pop_front() {
                        profiling_tick();
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
