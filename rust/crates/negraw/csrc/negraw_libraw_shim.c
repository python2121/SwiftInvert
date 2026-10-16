/*
 * negraw LibRaw shim — accessors for the libraw_data_t fields that LibRaw's C API offers no
 * setter/getter for. Compiled by build.rs against the host's own <libraw/libraw.h>, so the struct
 * layout is the installed library's and no offset is hard-coded on the Rust side. Every function
 * is a plain field read/write; the Rust wrapper in src/libraw/ffi.rs owns the pointer discipline.
 *
 * Mirrors Sources/RawDecodeKit/RawDecoder.swift, which touches exactly these fields directly.
 */
#include <stddef.h>
#include <string.h>
#include <libraw/libraw.h>

void negraw_libraw_set_half_size(libraw_data_t *lr, int v) { lr->params.half_size = v; }
void negraw_libraw_set_user_flip(libraw_data_t *lr, int v) { lr->params.user_flip = v; }
void negraw_libraw_set_use_camera_wb(libraw_data_t *lr, int v) { lr->params.use_camera_wb = v; }
void negraw_libraw_set_use_auto_wb(libraw_data_t *lr, int v) { lr->params.use_auto_wb = v; }

/* idata.filters: 9 = Fujifilm X-Trans, 0 = no CFA (linear / monochrome), 1 = Leaf Catchlight 16x16,
 * else a 2x2 Bayer bitmask. */
unsigned negraw_libraw_filters(const libraw_data_t *lr) { return lr->idata.filters; }
int negraw_libraw_colors(const libraw_data_t *lr) { return lr->idata.colors; }
/* sizes.flip: dcraw flip code of the camera's recorded orientation (0 / 3 / 5 / 6, ...). */
int negraw_libraw_flip(const libraw_data_t *lr) { return lr->sizes.flip; }
/* sizes.width/height: the output image area (masked borders removed), before flip and half_size. */
int negraw_libraw_width(const libraw_data_t *lr) { return lr->sizes.width; }
int negraw_libraw_height(const libraw_data_t *lr) { return lr->sizes.height; }

/* Bounded copies of idata.make / idata.model (char[64], NUL-terminated by LibRaw; forced here). */
static void copy_bounded(char *dst, size_t len, const char *src, size_t src_cap) {
    size_t n = 0;
    if (dst == NULL || len == 0) return;
    while (n + 1 < len && n < src_cap && src[n] != '\0') { dst[n] = src[n]; n++; }
    dst[n] = '\0';
}
void negraw_libraw_make(const libraw_data_t *lr, char *dst, size_t len) {
    copy_bounded(dst, len, lr->idata.make, sizeof lr->idata.make);
}
void negraw_libraw_model(const libraw_data_t *lr, char *dst, size_t len) {
    copy_bounded(dst, len, lr->idata.model, sizeof lr->idata.model);
}

/* Byte offset of the flexible `data` member of libraw_processed_image_t (16 on every known
 * LibRaw, but asked of the header rather than assumed). */
size_t negraw_libraw_processed_data_offset(void) { return offsetof(libraw_processed_image_t, data); }
