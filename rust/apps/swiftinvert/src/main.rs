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
use std::time::Duration;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup};
use eframe::wgpu;

const APP_NAME: &str = "SwiftInvert";

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
    if args.first().map(String::as_str) == Some("--selftest") {
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
        return match negui::selftest::run_selftest_with(
            std::path::Path::new(out),
            folder,
            Duration::from_secs(120),
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
        };
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
        viewport: egui::ViewportBuilder::default()
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
            Ok(Box::new(negui::NegApp::new(
                cc,
                negui::AppOptions {
                    initial_folder: folder,
                    ..Default::default()
                },
            )))
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
