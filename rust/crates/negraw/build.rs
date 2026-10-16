//! Build script: a no-op for the default (pure-Rust) backend. With the `libraw` feature it finds
//! LibRaw's thread-safe library (`libraw_r`) through pkg-config (or the `LIBRAW_*` overrides and
//! the Homebrew / system prefixes), compiles `csrc/negraw_libraw_shim.c` against the real
//! `<libraw/libraw.h>` — the shim reaches the `libraw_data_t` fields that have no C-API setter or
//! getter (`params.half_size`, `params.user_flip`, `idata.filters`, `sizes.flip`, …) with the
//! compiler knowing the exact struct layout, so no version-specific offset lives in Rust — and
//! emits the link line.
//!
//! When the library or header is missing the script WARNS and skips the shim instead of failing:
//! `cargo clippy --all-features` then still type-checks the FFI module on a host without LibRaw
//! (this SteamOS box); an actual `cargo build --features libraw` fails at link time with
//! undefined `libraw_*` / `negraw_libraw_*` symbols, which the warning explains.
//!
//! Only the LINK path and `-lraw_r` are taken from the `.pc` file. Homebrew's `libraw_r.pc` carries
//! `-Xpreprocessor -fopenmp` in its Cflags (which SwiftPM refused to forward); the shim does not use
//! OpenMP, so those flags are never passed to the C compiler.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "libraw")]
    libraw::configure();
}

#[cfg(feature = "libraw")]
mod libraw {
    use std::path::{Path, PathBuf};

    pub fn configure() {
        println!("cargo:rerun-if-changed=csrc/negraw_libraw_shim.c");
        for var in [
            "LIBRAW_INCLUDE_DIR",
            "LIBRAW_LIB_DIR",
            "LIBRAW_NO_PKG_CONFIG",
            "HOMEBREW_PREFIX",
        ] {
            println!("cargo:rerun-if-env-changed={var}");
        }
        let (includes, found) = locate();
        let header_ok = includes
            .iter()
            .any(|dir| dir.join("libraw").join("libraw.h").is_file());
        if !found || !header_ok {
            println!(
                "cargo:warning=negraw: LibRaw (libraw_r + <libraw/libraw.h>) not found; the `libraw` \
                 backend is type-checked only and will not link. Install libraw (brew/apt libraw-dev) \
                 or set LIBRAW_INCLUDE_DIR / LIBRAW_LIB_DIR."
            );
            return;
        }
        let mut build = cc::Build::new();
        build.file("csrc/negraw_libraw_shim.c").warnings(true);
        for dir in &includes {
            build.include(dir);
        }
        build.compile("negraw_libraw_shim");
        println!("cargo:rustc-link-lib=raw_r");
    }

    /// Include directories to try, and whether a library was found and its link search path emitted.
    fn locate() -> (Vec<PathBuf>, bool) {
        let mut includes = Vec::new();
        let mut found = false;
        if let Ok(dir) = std::env::var("LIBRAW_INCLUDE_DIR") {
            includes.push(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var("LIBRAW_LIB_DIR") {
            println!("cargo:rustc-link-search=native={dir}");
            found = true;
        }
        if !found && std::env::var_os("LIBRAW_NO_PKG_CONFIG").is_none() {
            // `cargo_metadata(false)`: Homebrew's `libraw_r.pc` lists `-lstdc++ -llcms2` (and a
            // `-fopenmp` cflag) that must NOT be forwarded — macOS has no libstdc++ (the link
            // failed with "library 'stdc++' not found" on the Mac, 2026-10-15) and the dylib already
            // records its own dependencies. We emit the search paths only and `-lraw_r` below.
            let probe = pkg_config::Config::new()
                .cargo_metadata(false)
                .print_system_libs(false)
                .probe("libraw_r");
            if let Ok(lib) = probe {
                for dir in &lib.link_paths {
                    println!("cargo:rustc-link-search=native={}", dir.display());
                }
                includes.extend(lib.include_paths);
                found = true;
            }
        }
        if !found {
            let mut prefixes: Vec<PathBuf> = Vec::new();
            if let Ok(p) = std::env::var("HOMEBREW_PREFIX") {
                prefixes.push(PathBuf::from(p));
            }
            prefixes.extend(["/opt/homebrew", "/usr/local", "/usr"].map(PathBuf::from));
            for prefix in prefixes {
                if has_library(&prefix.join("lib")) {
                    println!(
                        "cargo:rustc-link-search=native={}",
                        prefix.join("lib").display()
                    );
                    includes.push(prefix.join("include"));
                    found = true;
                    break;
                }
            }
        }
        if includes.is_empty() {
            // pkg-config leaves `include_paths` empty when the header sits in the default include path
            for dir in [
                "/usr/include",
                "/usr/local/include",
                "/opt/homebrew/include",
            ] {
                includes.push(PathBuf::from(dir));
            }
        }
        (includes, found)
    }

    fn has_library(lib: &Path) -> bool {
        [
            "libraw_r.so",
            "libraw_r.dylib",
            "libraw_r.a",
            "libraw_r.lib",
        ]
        .iter()
        .any(|name| lib.join(name).exists())
    }
}
