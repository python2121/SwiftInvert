//! Raw bindings to LibRaw's C API (`libraw_r`, the thread-safe build) plus the few field accessors
//! `csrc/negraw_libraw_shim.c` adds, wrapped in two owning handles so the rest of the backend never
//! touches a pointer. The ONLY module in the workspace that may contain `unsafe`.
//!
//! Invariants the wrappers keep:
//! - a [`Handle`] always holds a live `libraw_data_t*` from `libraw_init` and closes it on drop;
//! - a [`Processed`] always holds a live `libraw_processed_image_t*` from one of the
//!   `libraw_dcraw_make_mem_*` calls and frees it with `libraw_dcraw_clear_mem` on drop;
//! - every pointer passed to LibRaw is the handle's own; every slice handed out is bounded by the
//!   length LibRaw reported (`data_size`) and borrows the handle, so it cannot outlive the buffer.
//!
//! No `libraw_data_t` layout is assumed: fields without a C setter/getter go through the shim,
//! compiled against the installed header. The one struct declared here, the processed-image
//! header, has been stable for LibRaw's whole life (`type, height, width, colors, bits, data_size,
//! data[]`), and even its `data` offset is asked of the header (`negraw_libraw_processed_data_offset`).

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uint};
use std::marker::PhantomData;
use std::path::Path;

use crate::{NegRawError, Result};

/// `LIBRAW_SUCCESS`.
pub const LIBRAW_SUCCESS: c_int = 0;
/// `LIBRAW_IMAGE_JPEG` (a `libraw_processed_image_t::type`).
pub const LIBRAW_IMAGE_JPEG: c_int = 1;
/// `LIBRAW_IMAGE_BITMAP`.
pub const LIBRAW_IMAGE_BITMAP: c_int = 2;

/// Opaque `libraw_data_t`.
#[repr(C)]
pub struct LibrawData {
    _opaque: [u8; 0],
    _not_send: PhantomData<*mut u8>,
}

/// The fixed header of `libraw_processed_image_t`; the pixel/JPEG bytes follow at the offset the
/// shim reports.
#[repr(C)]
pub struct LibrawProcessedImage {
    pub kind: c_int,
    pub height: u16,
    pub width: u16,
    pub colors: u16,
    pub bits: u16,
    pub data_size: c_uint,
}

unsafe extern "C" {
    fn libraw_init(flags: c_uint) -> *mut LibrawData;
    fn libraw_close(lr: *mut LibrawData);
    fn libraw_open_file(lr: *mut LibrawData, file: *const c_char) -> c_int;
    fn libraw_unpack(lr: *mut LibrawData) -> c_int;
    fn libraw_unpack_thumb(lr: *mut LibrawData) -> c_int;
    fn libraw_dcraw_process(lr: *mut LibrawData) -> c_int;
    fn libraw_dcraw_make_mem_image(
        lr: *mut LibrawData,
        errc: *mut c_int,
    ) -> *mut LibrawProcessedImage;
    fn libraw_dcraw_make_mem_thumb(
        lr: *mut LibrawData,
        errc: *mut c_int,
    ) -> *mut LibrawProcessedImage;
    fn libraw_dcraw_clear_mem(p: *mut LibrawProcessedImage);
    fn libraw_strerror(code: c_int) -> *const c_char;
    // output-parameter setters LibRaw's C API provides
    fn libraw_set_demosaic(lr: *mut LibrawData, value: c_int);
    fn libraw_set_adjust_maximum_thr(lr: *mut LibrawData, value: c_float);
    fn libraw_set_output_color(lr: *mut LibrawData, value: c_int);
    fn libraw_set_output_bps(lr: *mut LibrawData, value: c_int);
    fn libraw_set_gamma(lr: *mut LibrawData, index: c_int, value: c_float);
    fn libraw_set_no_auto_bright(lr: *mut LibrawData, value: c_int);
    fn libraw_set_user_mul(lr: *mut LibrawData, index: c_int, value: c_float);
    // the shim (csrc/negraw_libraw_shim.c)
    fn negraw_libraw_set_half_size(lr: *mut LibrawData, value: c_int);
    fn negraw_libraw_set_user_flip(lr: *mut LibrawData, value: c_int);
    fn negraw_libraw_set_use_camera_wb(lr: *mut LibrawData, value: c_int);
    fn negraw_libraw_set_use_auto_wb(lr: *mut LibrawData, value: c_int);
    fn negraw_libraw_filters(lr: *const LibrawData) -> c_uint;
    fn negraw_libraw_colors(lr: *const LibrawData) -> c_int;
    fn negraw_libraw_flip(lr: *const LibrawData) -> c_int;
    fn negraw_libraw_width(lr: *const LibrawData) -> c_int;
    fn negraw_libraw_height(lr: *const LibrawData) -> c_int;
    fn negraw_libraw_make(lr: *const LibrawData, dst: *mut c_char, len: usize);
    fn negraw_libraw_model(lr: *const LibrawData, dst: *mut c_char, len: usize);
    fn negraw_libraw_processed_data_offset() -> usize;
}

/// `libraw_strerror(code)` as an owned string.
fn strerror(code: c_int) -> String {
    // SAFETY: libraw_strerror returns a pointer to a static NUL-terminated string for any code
    // (unknown codes map to "Unknown error code"); it is never NULL and never freed.
    let p = unsafe { libraw_strerror(code) };
    if p.is_null() {
        return format!("error {code}");
    }
    // SAFETY: non-NULL static C string, see above.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn check(stage: &'static str, rc: c_int) -> Result<()> {
    if rc == LIBRAW_SUCCESS {
        Ok(())
    } else {
        Err(NegRawError::LibRaw {
            stage,
            code: rc,
            message: strerror(rc),
        })
    }
}

/// An owned `libraw_data_t`.
pub struct Handle(*mut LibrawData);

impl Handle {
    /// `libraw_init(0)`.
    pub fn new() -> Result<Self> {
        // SAFETY: libraw_init takes only flags; it returns NULL on allocation failure, checked below.
        let p = unsafe { libraw_init(0) };
        if p.is_null() {
            return Err(NegRawError::LibRaw {
                stage: "init",
                code: -1,
                message: "libraw_init returned NULL".into(),
            });
        }
        Ok(Self(p))
    }

    /// `libraw_open_file`. The path is passed in the OS encoding (UTF-8 on Unix; LibRaw's
    /// narrow-char opener on Windows takes the ANSI code page, so non-ASCII paths there need
    /// `libraw_open_wfile` — not wired yet).
    pub fn open(&mut self, path: &Path) -> Result<()> {
        let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| NegRawError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains a NUL byte",
            ),
        })?;
        // SAFETY: self.0 is a live handle; `c` is a valid NUL-terminated string that outlives the call.
        check("open", unsafe { libraw_open_file(self.0, c.as_ptr()) })
    }

    /// `libraw_unpack`.
    pub fn unpack(&mut self) -> Result<()> {
        // SAFETY: live handle.
        check("unpack", unsafe { libraw_unpack(self.0) })
    }

    /// `libraw_unpack_thumb`.
    pub fn unpack_thumb(&mut self) -> Result<()> {
        // SAFETY: live handle.
        check("unpack_thumb", unsafe { libraw_unpack_thumb(self.0) })
    }

    /// `libraw_dcraw_process`.
    pub fn process(&mut self) -> Result<()> {
        // SAFETY: live handle; parameters were set through the setters below.
        check("process", unsafe { libraw_dcraw_process(self.0) })
    }

    /// `params.gamm[index] = value` (`libraw_set_gamma`).
    pub fn set_gamma(&mut self, index: i32, value: f32) {
        // SAFETY: live handle; LibRaw bounds-checks the index itself.
        unsafe { libraw_set_gamma(self.0, index, value) }
    }
    /// `params.no_auto_bright`.
    pub fn set_no_auto_bright(&mut self, value: i32) {
        // SAFETY: live handle.
        unsafe { libraw_set_no_auto_bright(self.0, value) }
    }
    /// `params.adjust_maximum_thr`.
    pub fn set_adjust_maximum_thr(&mut self, value: f32) {
        // SAFETY: live handle.
        unsafe { libraw_set_adjust_maximum_thr(self.0, value) }
    }
    /// `params.output_bps`.
    pub fn set_output_bps(&mut self, value: i32) {
        // SAFETY: live handle.
        unsafe { libraw_set_output_bps(self.0, value) }
    }
    /// `params.output_color` (0 = raw / sensor-native).
    pub fn set_output_color(&mut self, value: i32) {
        // SAFETY: live handle.
        unsafe { libraw_set_output_color(self.0, value) }
    }
    /// `params.user_mul[index] = value`.
    pub fn set_user_mul(&mut self, index: i32, value: f32) {
        // SAFETY: live handle; LibRaw bounds-checks the index itself.
        unsafe { libraw_set_user_mul(self.0, index, value) }
    }
    /// `params.user_qual` (`libraw_set_demosaic`; 0 = linear).
    pub fn set_demosaic(&mut self, value: i32) {
        // SAFETY: live handle.
        unsafe { libraw_set_demosaic(self.0, value) }
    }
    /// `params.half_size` (shim).
    pub fn set_half_size(&mut self, value: i32) {
        // SAFETY: live handle; the shim writes one int field.
        unsafe { negraw_libraw_set_half_size(self.0, value) }
    }
    /// `params.user_flip` (shim; 0 = sensor orientation, we bake the flip ourselves).
    pub fn set_user_flip(&mut self, value: i32) {
        // SAFETY: live handle; the shim writes one int field.
        unsafe { negraw_libraw_set_user_flip(self.0, value) }
    }
    /// `params.use_camera_wb` (shim).
    pub fn set_use_camera_wb(&mut self, value: i32) {
        // SAFETY: live handle; the shim writes one int field.
        unsafe { negraw_libraw_set_use_camera_wb(self.0, value) }
    }
    /// `params.use_auto_wb` (shim).
    pub fn set_use_auto_wb(&mut self, value: i32) {
        // SAFETY: live handle; the shim writes one int field.
        unsafe { negraw_libraw_set_use_auto_wb(self.0, value) }
    }

    /// `idata.filters` (valid after `open`).
    pub fn filters(&self) -> u32 {
        // SAFETY: live handle; the shim reads one field.
        unsafe { negraw_libraw_filters(self.0) }
    }
    /// `idata.colors`.
    pub fn colors(&self) -> i32 {
        // SAFETY: live handle; the shim reads one field.
        unsafe { negraw_libraw_colors(self.0) }
    }
    /// `sizes.flip` — the dcraw flip code of the recorded orientation.
    pub fn flip(&self) -> i32 {
        // SAFETY: live handle; the shim reads one field.
        unsafe { negraw_libraw_flip(self.0) }
    }
    /// `sizes.width` / `sizes.height` (image area before flip / half_size).
    pub fn size(&self) -> (usize, usize) {
        // SAFETY: live handle; the shim reads two fields.
        let (w, h) = unsafe { (negraw_libraw_width(self.0), negraw_libraw_height(self.0)) };
        (
            usize::try_from(w).unwrap_or(0),
            usize::try_from(h).unwrap_or(0),
        )
    }
    /// `idata.make`, empty → `None`.
    pub fn make(&self) -> Option<String> {
        let mut buf = [0 as c_char; 64];
        // SAFETY: live handle; the shim copies at most `len - 1` bytes and NUL-terminates.
        unsafe { negraw_libraw_make(self.0, buf.as_mut_ptr(), buf.len()) };
        c_buf_to_string(&buf)
    }
    /// `idata.model`, empty → `None`.
    pub fn model(&self) -> Option<String> {
        let mut buf = [0 as c_char; 64];
        // SAFETY: live handle; the shim copies at most `len - 1` bytes and NUL-terminates.
        unsafe { negraw_libraw_model(self.0, buf.as_mut_ptr(), buf.len()) };
        c_buf_to_string(&buf)
    }

    /// `libraw_dcraw_make_mem_image` (after `process`).
    pub fn make_mem_image(&mut self) -> Result<Processed<'_>> {
        let mut err: c_int = 0;
        // SAFETY: live handle; `err` is a valid out-pointer for the call's duration.
        let p = unsafe { libraw_dcraw_make_mem_image(self.0, &mut err) };
        Processed::wrap(p, err, "make_mem_image")
    }

    /// `libraw_dcraw_make_mem_thumb` (after `unpack_thumb`).
    pub fn make_mem_thumb(&mut self) -> Result<Processed<'_>> {
        let mut err: c_int = 0;
        // SAFETY: live handle; `err` is a valid out-pointer for the call's duration.
        let p = unsafe { libraw_dcraw_make_mem_thumb(self.0, &mut err) };
        Processed::wrap(p, err, "make_mem_thumb")
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 came from libraw_init and is closed exactly once, here.
        unsafe { libraw_close(self.0) }
    }
}

fn c_buf_to_string(buf: &[c_char]) -> Option<String> {
    // c_char may be i8 or u8 per platform: go through the byte view
    let bytes: Vec<u8> = buf.iter().map(|&c| c as u8).collect();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(bytes.get(..end).unwrap_or(&[]))
        .trim()
        .to_string();
    (!s.is_empty()).then_some(s)
}

/// An owned `libraw_processed_image_t`, borrowing the handle that made it.
pub struct Processed<'a> {
    ptr: *mut LibrawProcessedImage,
    _handle: PhantomData<&'a Handle>,
}

impl Processed<'_> {
    fn wrap(ptr: *mut LibrawProcessedImage, err: c_int, stage: &'static str) -> Result<Self> {
        if ptr.is_null() {
            return Err(NegRawError::LibRaw {
                stage,
                code: err,
                message: strerror(err),
            });
        }
        Ok(Self {
            ptr,
            _handle: PhantomData,
        })
    }

    fn header(&self) -> &LibrawProcessedImage {
        // SAFETY: non-NULL pointer to a libraw_processed_image_t that LibRaw allocated and that
        // lives until `libraw_dcraw_clear_mem` in Drop; the header fields are the C struct's own.
        unsafe { &*self.ptr }
    }
    /// `LIBRAW_IMAGE_JPEG` / `LIBRAW_IMAGE_BITMAP`.
    pub fn kind(&self) -> i32 {
        self.header().kind
    }
    pub fn width(&self) -> usize {
        usize::from(self.header().width)
    }
    pub fn height(&self) -> usize {
        usize::from(self.header().height)
    }
    pub fn colors(&self) -> u16 {
        self.header().colors
    }
    pub fn bits(&self) -> u16 {
        self.header().bits
    }
    /// The payload: `data_size` bytes starting at the flexible `data` member (u16 pixels in host
    /// byte order for a bitmap, an encoded file for a JPEG thumbnail).
    pub fn data(&self) -> &[u8] {
        let len = self.header().data_size as usize;
        // SAFETY: the shim's offsetof puts `base + offset` at the flexible array member, which
        // LibRaw allocated with `data_size` bytes in the same block (sizeof header + data_size); the
        // slice borrows `self`, so it cannot outlive the buffer.
        unsafe {
            let base = self
                .ptr
                .cast::<u8>()
                .add(negraw_libraw_processed_data_offset());
            std::slice::from_raw_parts(base, len)
        }
    }
}

impl Drop for Processed<'_> {
    fn drop(&mut self) {
        // SAFETY: non-NULL pointer from libraw_dcraw_make_mem_*, freed exactly once, here.
        unsafe { libraw_dcraw_clear_mem(self.ptr) }
    }
}
