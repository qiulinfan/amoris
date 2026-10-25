//! A minimal logger for native hosts that writes warnings and errors to stderr.
//!
//! The renderer reports pipeline and validation errors only through `log` (gpu.rs's uncaptured-error
//! hook writes `wgpu: ...`), and pipelines are built on threads, so without a logger a failed
//! pipeline on Direct3D 12 or Vulkan is silent: a capture just lacks the pass. Hosts and drawing
//! examples install this once at start. wgpu's and naga's own warnings stay out; their errors show.

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        let noisy = m.target().starts_with("wgpu") || m.target().starts_with("naga");
        let least = if noisy {
            log::Level::Error
        } else {
            log::Level::Warn
        };
        m.level() <= least
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

/// Installs the logger; a second call, or a host that installed its own, keeps the first.
pub fn init() {
    static LOGGER: Stderr = Stderr;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}
