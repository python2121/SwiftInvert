#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! SwiftInvert desktop binary: the eframe shell from `negui` over a wgpu device the engine
//! shares (`neggpu::context::required_limits` raised so the full tier and export fit), with
//! `--selftest out.png [folder]` rendering the whole app offscreen to a PNG.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
use eframe::wgpu;

const APP_NAME: &str = "SwiftInvert";

/// The SwiftInvert app icon (`assets/icon_1024.png`, the artwork the original Swift app shipped),
/// with macOS's standard transparent margin, so the
/// Dock and ⌘Tab show it at the size every other app has.
const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icon_1024.png");

/// The window/app icon. Inside a macOS `.app` this is the EMPTY icon: LaunchServices draws the
/// bundle's `AppIcon.icns` in the Dock and ⌘Tab, and a runtime `applicationIconImage` would only
/// duplicate it — AppKit caches that image at 2× in a deep-colour format, 32 MB for the 1024 px
/// artwork. (`None` is NOT "no icon" to eframe: it substitutes the egui logo, which then covers
/// the bundle icon. An empty `IconData` is the documented "set nothing" — eframe's icon setter
/// skips it and egui-winit passes no window icon.) Unbundled on macOS (`cargo run`) a 512 px copy
/// is handed over instead (the Dock and ⌘Tab draw at most ~128 pt = 256 px on Retina, so 512 is
/// still 2× oversampled); X11/Windows get 256 px (window-manager icon property). On Wayland the
/// compositor uses the `swiftinvert.desktop` entry matching the app id instead —
/// `packaging/linux/install-desktop.sh` installs it. `None` (logged) if the PNG can't be decoded.
fn app_icon() -> Option<std::sync::Arc<egui::IconData>> {
    #[cfg(target_os = "macos")]
    if runs_from_bundle() {
        return Some(std::sync::Arc::new(egui::IconData::default()));
    }
    let img = match image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("app icon: {e}");
            return None;
        }
    };
    let side = if cfg!(target_os = "macos") { 512 } else { 256 };
    let img = img.resize_exact(side, side, image::imageops::FilterType::Lanczos3);
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some(std::sync::Arc::new(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    }))
}

/// True when the executable runs from inside a macOS `.app` bundle (`…/Contents/MacOS/<bin>`),
/// where the bundle's `AppIcon.icns` already owns the Dock and ⌘Tab icon.
#[cfg(target_os = "macos")]
fn runs_from_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.ends_with("Contents/MacOS")))
        .unwrap_or(false)
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: swiftinvert [folder]\n       swiftinvert --selftest <out.png> [folder] [--crop] [--dialogs] [--export <dir>] [--latency] [--teststrip] [--zones]"
    );
    ExitCode::from(2)
}

/// The device request both the window and the selftest harness use: the OS backend, limits
/// raised so the full tier and full-size export fit (the engine shares this device).
fn device_descriptor()
-> Arc<dyn Fn(&wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> + Send + Sync> {
    Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
        label: Some("swiftinvert"),
        required_limits: neggpu::context::required_limits(&adapter.limits()),
        ..Default::default()
    })
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--selftest") {
        return selftest_cli(args);
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        return usage();
    }
    let folder = args.first().map(PathBuf::from);

    let mut wgpu_options = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(n) = &mut wgpu_options.wgpu_setup {
        n.instance_descriptor.backends = neggpu::context::default_backends();
        n.device_descriptor = device_descriptor();
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder {
            icon: app_icon(),
            ..Default::default()
        }
        .with_title(APP_NAME)
        .with_app_id("swiftinvert")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([1000.0, 700.0]),
        wgpu_options,
        ..Default::default()
    };
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            // macOS: the menus go in the system menu bar (opt out with SWIFTINVERT_NO_NATIVE_MENU=1);
            // every other platform keeps the in-window egui menu bar.
            let native_menu = cfg!(target_os = "macos")
                && std::env::var_os("SWIFTINVERT_NO_NATIVE_MENU").is_none();
            let app = negui::NegApp::new(
                cc,
                negui::AppOptions {
                    initial_folder: folder,
                    native_menu,
                    // SWIFTINVERT_FONT=bundled keeps the bundled Inter instead of the system font
                    system_font: std::env::var("SWIFTINVERT_FONT").map_or(true, |v| v != "bundled"),
                    ..Default::default()
                },
            );
            #[cfg(target_os = "macos")]
            if native_menu {
                return Ok(
                    match negui::native_menu::NativeMenuApp::new(app, &cc.egui_ctx) {
                        Ok(wrapped) => Box::new(wrapped) as Box<dyn eframe::App>,
                        Err(app) => app,
                    },
                );
            }
            Ok(Box::new(app))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("swiftinvert: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--selftest out.png [folder] [flags]`: the whole app rendered offscreen (the `selftest`
/// feature, on by default; release packaging can build without the test harness).
#[cfg(feature = "selftest")]
fn selftest_cli(args: Vec<String>) -> ExitCode {
    // --- U3: darkroom --- `--teststrip` / `--zones` add darkroom captures to `--selftest`
    let darkroom = negui::selftest::DarkroomCaptures {
        test_strip: args.iter().any(|a| a == "--teststrip"),
        zones: args.iter().any(|a| a == "--zones"),
    };
    let args: Vec<String> = args
        .into_iter()
        .filter(|a| a != "--teststrip" && a != "--zones")
        .collect();
    // --- end U3 ---
    {
        let Some(out) = args.get(1) else {
            return usage();
        };
        // --- U2: tools & dialogs --- `[folder]` then flags
        let mut extra = negui::selftest::SelftestOptions {
            darkroom,
            ..Default::default()
        };
        let mut folder: Option<PathBuf> = None;
        let mut rest = args.iter().skip(2);
        while let Some(a) = rest.next() {
            match a.as_str() {
                "--crop" => extra.crop = true,
                "--dialogs" => extra.dialogs = true,
                "--latency" => extra.latency = true,
                "--export" => match rest.next() {
                    Some(d) => extra.export_dir = Some(PathBuf::from(d)),
                    None => return usage(),
                },
                f if !f.starts_with("--") && folder.is_none() => folder = Some(PathBuf::from(f)),
                _ => return usage(),
            }
        }
        if std::env::var("NEG_GPU")
            .ok()
            .is_none_or(|v| !v.eq_ignore_ascii_case("off"))
        {
            let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
            setup.instance_descriptor.backends = neggpu::context::default_backends();
            setup.device_descriptor = device_descriptor();
            extra.wgpu_setup = Some(WgpuSetup::CreateNew(setup));
        }
        let folder =
            folder.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")));
        let Some(folder) = folder else { return usage() };
        match negui::selftest::run_selftest_with(
            std::path::Path::new(out),
            folder,
            std::time::Duration::from_secs(120),
            extra,
        ) {
            Ok([w, h]) => {
                println!("selftest: wrote {out} ({w}×{h})");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("selftest failed: {e}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(not(feature = "selftest"))]
fn selftest_cli(_args: Vec<String>) -> ExitCode {
    eprintln!("swiftinvert: built without the `selftest` feature");
    ExitCode::FAILURE
}
