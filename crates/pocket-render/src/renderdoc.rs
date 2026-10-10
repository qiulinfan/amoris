//! One-frame RenderDoc captures from inside the renderer (docs/bench/dx12.md, "Per-frame
//! profiling"): with `POCKET_RENDERDOC_FRAME=N`, when the process runs under RenderDoc (its library
//! already loaded, as `renderdoccmd capture` and `tools/renderdoc_frame.py` launch it), the N-th
//! `Renderer::render` (from 1) is bracketed with the in-application API's `StartFrameCapture` and
//! `EndFrameCapture` for any device and window. Headless examples, which never present a frame for
//! RenderDoc to delimit, are captured the same way as windowed ones.
//!
//! wgpu's `Device::start_graphics_debugger_capture` does the same but only in builds with debug
//! assertions (wgpu-hal 30 gates RenderDoc on them), never in the release builds that are measured.

use std::ffi::c_void;

use renderdoc_sys::RENDERDOC_API_1_6_0 as Api;

pub const ENV: &str = "POCKET_RENDERDOC_FRAME";

#[cfg(windows)]
const LIBRARY: &str = "renderdoc.dll";
#[cfg(not(windows))]
const LIBRARY: &str = "librenderdoc.so";

/// The API table: function pointers RenderDoc never changes, valid while its library is loaded.
struct Table(*const Api);

// SAFETY: the table is immutable and process-wide; RenderDoc's API may be called from any thread.
unsafe impl Send for Table {}
// SAFETY: as above.
unsafe impl Sync for Table {}

/// The frame to capture and, once the library was looked up, its API.
pub struct FrameCapture {
    frame: u64,
    rendered: u64,
    /// The API table and the library that keeps it valid.
    api: Option<(Table, libloading::Library)>,
    capturing: bool,
}

impl FrameCapture {
    /// From `POCKET_RENDERDOC_FRAME`: `None` when unset, invalid or not running under RenderDoc.
    pub fn from_env() -> Option<FrameCapture> {
        let v = std::env::var(ENV).ok()?;
        let Some(frame) = v.trim().parse::<u64>().ok().filter(|f| *f > 0) else {
            eprintln!("{ENV}={v}: expected a frame number from 1; no capture");
            return None;
        };
        match load() {
            Ok((table, lib)) => Some(FrameCapture {
                frame,
                rendered: 0,
                api: Some((Table(table), lib)),
                capturing: false,
            }),
            Err(e) => {
                eprintln!("{ENV}: {e}; launch the process under RenderDoc (renderdoccmd capture)");
                None
            }
        }
    }

    /// Before a frame is encoded: starts the capture when it is the chosen one.
    pub fn before_frame(&mut self) {
        self.rendered += 1;
        if self.rendered != self.frame {
            return;
        }
        if let Some((Table(api), _)) = &self.api {
            // SAFETY: `api` came from RENDERDOC_GetAPI for version 1.6.0 and the library is held
            // loaded; null device and window are the API's documented wildcards.
            unsafe {
                if let Some(start) = (**api).StartFrameCapture {
                    start(
                        std::ptr::null_mut::<c_void>(),
                        std::ptr::null_mut::<c_void>(),
                    );
                    self.capturing = true;
                }
            }
        }
    }

    /// After the frame was submitted: ends the capture it started.
    pub fn after_frame(&mut self) {
        if !std::mem::take(&mut self.capturing) {
            return;
        }
        if let Some((Table(api), _)) = &self.api {
            // SAFETY: as in `before_frame`.
            let ok = unsafe {
                (**api)
                    .EndFrameCapture
                    .is_some_and(|end| end(std::ptr::null_mut(), std::ptr::null_mut()) == 1)
            };
            eprintln!(
                "renderdoc: frame {} {}",
                self.frame,
                if ok { "captured" } else { "not captured" }
            );
        }
    }
}

/// The API of the RenderDoc library loaded in this process (never loads it).
fn load() -> Result<(*const Api, libloading::Library), String> {
    #[cfg(windows)]
    let lib = libloading::os::windows::Library::open_already_loaded(LIBRARY)
        .map(libloading::Library::from)
        .map_err(|e| format!("{LIBRARY} is not loaded ({e})"))?;
    #[cfg(not(windows))]
    // SAFETY: RTLD_NOLOAD only returns a library that is already loaded; nothing is initialized.
    let lib = unsafe {
        libloading::os::unix::Library::open(
            Some(LIBRARY),
            libloading::os::unix::RTLD_NOW | 0x4, /* RTLD_NOLOAD */
        )
    }
    .map(libloading::Library::from)
    .map_err(|e| format!("{LIBRARY} is not loaded ({e})"))?;
    type GetApi = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
    let mut api: *mut c_void = std::ptr::null_mut();
    // SAFETY: RENDERDOC_GetAPI has this signature; version 1.6.0 (10600) fills `api` with a
    // pointer to a RENDERDOC_API_1_6_0 table that lives as long as the library.
    let ok = unsafe {
        let get: libloading::Symbol<'_, GetApi> = lib
            .get(b"RENDERDOC_GetAPI\0")
            .map_err(|e| format!("RENDERDOC_GetAPI: {e}"))?;
        get(10600, &mut api)
    };
    if ok != 1 || api.is_null() {
        return Err("RenderDoc refused API version 1.6.0".into());
    }
    Ok((api.cast::<Api>().cast_const(), lib))
}
