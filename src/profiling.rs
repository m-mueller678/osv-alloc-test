pub fn init_profiling() {
    #[cfg(feature = "puffin_profiling")]
    {
        let server_addr = format!("0.0.0.0:{}", puffin_http::DEFAULT_PORT);
        std::mem::forget(puffin_http::Server::new(&server_addr).unwrap());
        eprintln!("Serving profile data on {server_addr}. Run with `--forward tcp::8585-:8585` and then run `puffin_viewer --url {server_addr}` to view it.");
        puffin::set_scopes_on(true);
    }
}

pub fn profiling_tick() {
    #[cfg(feature = "puffin_profiling")]
    {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % (1 << 12) == 0 {
            let mut p = puffin::GlobalProfiler::lock();
            p.new_frame();
        }
    }
}
