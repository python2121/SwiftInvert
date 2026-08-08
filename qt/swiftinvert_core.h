/* C ABI of libSwiftInvertCore.so (Sources/CoreBridge/CoreBridge.swift owns
 * the implementations — keep the two files in sync).
 *
 * Conventions: settings are sidecar-format JSON (missing keys = defaults);
 * every returned pointer is malloc'd and must be released with si_free();
 * failures return 0/NULL and si_last_error() describes the most recent one.
 * All functions are thread-safe; si_open and si_render are heavy enough to
 * belong off the UI thread. */
#ifndef SWIFTINVERT_CORE_H
#define SWIFTINVERT_CORE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Decode preview (≤1536px) + analyze + upload to the GPU. ~0.5–1 s. */
int64_t si_open(const char *path);
void si_close(int64_t session);
int32_t si_size(int64_t session, int32_t *width, int32_t *height);

/* Settings JSON → derive → GPU render. RGBA8, width*height*4, alpha 255.
 * ~2 ms at preview size on a discrete GPU. srgb_display != 0 converts the
 * output for an unmanaged sRGB canvas. histogram (nullable) receives the
 * 4×256 bins: R,G,B,Rec.709-luma raw counts in the display domain. */
uint8_t *si_render(int64_t session, const char *settings_json,
                   int32_t srgb_display, int32_t *width, int32_t *height,
                   uint32_t *histogram /* uint32[1024] or NULL */);

/* Frame facts as JSON: width, height, densityRange, defaultGradeRange,
 * character (optional), anchor, castConfidence (optional). NULL on bad
 * handle. Caller frees. */
char *si_session_info(int64_t session);

/* Sidecar IO (same file naming + payload as the Mac app, so edits
 * round-trip): load returns the settings JSON or NULL if no sidecar;
 * save returns 1 on success. */
char *si_sidecar_load(const char *path);
int32_t si_sidecar_save(const char *path, const char *settings_json);

/* Embedded camera JPEG (QImage::fromData decodes it). NULL if absent. */
uint8_t *si_thumbnail(const char *path, int32_t *length);

/* Default ExposureSettings as JSON — the slider defaults, single-sourced. */
char *si_default_settings(void);

/* Vulkan device name ("" until the first render initializes the GPU). */
char *si_device_name(void);

char *si_last_error(void);
void si_free(void *ptr);

#ifdef __cplusplus
}
#endif

#endif /* SWIFTINVERT_CORE_H */
