//! Shared ABI between bunzod's wasmtime host and the WASM skills it loads.
//!
//! # Wire
//!
//! A skill is a `wasm32-unknown-unknown` module exporting:
//!
//! * `bunzo_alloc(len: u32) -> u32` — allocates `len` bytes in skill memory
//!   and returns a pointer. The host calls this both to stage the input buffer
//!   and to stage the results of host imports (e.g. a file's contents).
//! * `bunzo_dealloc(ptr: u32, len: u32)` — frees a previously allocated
//!   buffer. The host calls this on skill-owned buffers it consumed.
//! * `run(input_ptr: u32, input_len: u32) -> u64` — entry point. The two
//!   arguments describe the JSON-encoded input buffer inside skill memory.
//!   The return value packs `(out_ptr << 32) | out_len`. The output bytes are
//!   JSON-encoded too. A zero return signals a skill-internal error.
//!
//! And imports, all from module `bunzo`:
//!
//! * `bunzo_fs_read(path_ptr: u32, path_len: u32) -> u64` — returns a packed
//!   `(ptr << 32) | len` pointing at a buffer *the host has allocated in
//!   skill memory* via `bunzo_alloc`, containing the file's contents. Zero
//!   means the read failed (permission denied, path not in capability list,
//!   I/O error). The host logs the reason to journald.
//! * `bunzo_log(ptr: u32, len: u32)` — emits a UTF-8 message to bunzod's
//!   stderr. Useful for skill-side debugging.
//!
//! # Usage on the skill side
//!
//! Skills depend on this crate and use [`bunzo_skill!`] to plug a strongly
//! typed Rust function into the `run` export:
//!
//! ```ignore
//! use bunzo_skill_abi::bunzo_skill;
//!
//! #[derive(serde::Deserialize)]
//! struct In { path: String }
//!
//! #[derive(serde::Serialize)]
//! struct Out { content: String }
//!
//! bunzo_skill!(run_skill);
//!
//! fn run_skill(input: In) -> Result<Out, String> {
//!     // ...
//! #   Ok(Out { content: String::new() })
//! }
//! ```

#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(target_arch = "wasm32")]
extern crate alloc;

#[cfg(target_arch = "wasm32")]
use alloc::vec::Vec;

/// Pack a (pointer, length) pair into the u64 return value used by `run` and
/// by the `bunzo_fs_read` host import.
#[inline]
pub fn pack(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | (len as u64)
}

/// Unpack the (pointer, length) encoding used by `pack`.
#[inline]
pub fn unpack(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Sentinel returned by `run` or `bunzo_fs_read` on error.
pub const ERR: u64 = 0;

/// Allocator trampoline — exported by every skill.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn bunzo_alloc(len: u32) -> u32 {
    let mut buf: Vec<u8> = Vec::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr() as u32;
    core::mem::forget(buf);
    ptr
}

/// Deallocator trampoline — symmetric with `bunzo_alloc`. Frees a Vec of the
/// given size that was previously leaked by `bunzo_alloc`.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn bunzo_dealloc(ptr: u32, len: u32) {
    unsafe {
        let _ = Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize);
    }
}

/// Host imports available to skills. Only callable from `wasm32` targets.
#[cfg(target_arch = "wasm32")]
pub mod host {
    #[link(wasm_import_module = "bunzo")]
    extern "C" {
        pub fn bunzo_fs_read(path_ptr: u32, path_len: u32) -> u64;
        pub fn bunzo_log(ptr: u32, len: u32);
    }
}

/// Safe wrapper around `bunzo_fs_read`. Returns the file contents, or an error
/// string if the host denied or failed the read.
#[cfg(target_arch = "wasm32")]
pub fn fs_read(path: &str) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let packed = unsafe { host::bunzo_fs_read(path.as_ptr() as u32, path.len() as u32) };
    if packed == ERR {
        return Err("host denied or failed fs read");
    }
    let (ptr, len) = unpack(packed);
    let bytes = unsafe {
        core::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec()
    };
    // The host allocated this buffer in our memory via bunzo_alloc; free it.
    unsafe {
        let _ = alloc::vec::Vec::from_raw_parts(
            ptr as *mut u8,
            len as usize,
            len as usize,
        );
    }
    Ok(bytes)
}

/// Safe wrapper around `bunzo_log`.
#[cfg(target_arch = "wasm32")]
pub fn log(msg: &str) {
    unsafe { host::bunzo_log(msg.as_ptr() as u32, msg.len() as u32) };
}

/// Plug a strongly typed Rust fn `(Input) -> Result<Output, E>` into the
/// `run` export. Input and Output must be serde types; `E` must implement
/// `core::fmt::Display`.
#[macro_export]
macro_rules! bunzo_skill {
    ($skill_fn:ident) => {
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn run(input_ptr: u32, input_len: u32) -> u64 {
            let input_bytes = unsafe {
                core::slice::from_raw_parts(input_ptr as *const u8, input_len as usize)
            };
            let input = match serde_json::from_slice(input_bytes) {
                Ok(v) => v,
                Err(_) => return $crate::ERR,
            };
            let result = $skill_fn(input);
            let output = match result {
                Ok(out) => out,
                Err(_) => return $crate::ERR,
            };
            let bytes = match serde_json::to_vec(&output) {
                Ok(b) => b,
                Err(_) => return $crate::ERR,
            };
            let len = bytes.len() as u32;
            let ptr = $crate::bunzo_alloc(len);
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, len as usize);
            }
            $crate::pack(ptr, len)
        }
    };
}

/// Minimal panic handler for WASM skills. Calls `bunzo_log` with the panic
/// message and then aborts via `unreachable!()`.
#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    let mut buf = alloc::string::String::new();
    let _ = write!(&mut buf, "{info}");
    log(&buf);
    core::arch::wasm32::unreachable()
}

/// Global allocator for WASM skills — lean (~1 KiB) and dependency-free.
#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: MiniAlloc = MiniAlloc;

/// Bump allocator with a single linear arena. Good enough for short-lived
/// skill invocations; leaked memory is reclaimed when wasmtime drops the
/// `Store`. Not thread safe — skills run single-threaded inside their Store.
#[cfg(target_arch = "wasm32")]
struct MiniAlloc;

#[cfg(target_arch = "wasm32")]
unsafe impl core::alloc::GlobalAlloc for MiniAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        use core::sync::atomic::{AtomicUsize, Ordering};
        // 1 MiB arena, aligned at 16. Carved out of the linear memory's static
        // data segment by the compiler, not reserved via `memory.grow`.
        static ARENA: [u8; 1 << 20] = [0; 1 << 20];
        static CURSOR: AtomicUsize = AtomicUsize::new(0);
        let align = layout.align().max(1);
        let size = layout.size();
        loop {
            let cur = CURSOR.load(Ordering::Relaxed);
            let base = ARENA.as_ptr() as usize;
            let aligned = (base + cur + align - 1) & !(align - 1);
            let new = aligned - base + size;
            if new > ARENA.len() {
                return core::ptr::null_mut();
            }
            if CURSOR
                .compare_exchange(cur, new, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return aligned as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {
        // Arena lives for the entire skill invocation; freed when wasmtime
        // drops the Store after `run` returns.
    }
}
