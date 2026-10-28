#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negcli` — headless driver over `negsession` (the port of `Sources/negcli`): decode / thumb /
//! render / meter / bench / export. `render`/`meter`/`export` use `negsession::default_renderer`
//! (the GPU when one comes up, else the CPU chain; `NEG_GPU=off` forces the CPU); `bench` runs
//! the session pass through BOTH and prints them side by side with the adapter's name.
//!
//! ```text
//! negcli decode <raw> [--full] -o out.png
//! negcli thumb  <raw> -o out.jpg
//! negcli render <raw> [--sidecar P | --stock] [--set key=value]... [--srgb]
//!               [--tier proxy|medium|full] [--uncropped] -o out.png|out.jpg|out.tiff
//! negcli meter  <raw> [--sidecar P | --stock] [--set key=value]...
//! negcli bench  <raw> [--runs N]
//! negcli export <raw> [--jpeg|--tiff] [--srgb|--adobe] [--long-edge N] [--quality Q]
//!               [--sidecar P | --stock] -o <dir>
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{ExtendedColorType, ImageEncoder};
use negkit::derive::derive_render_params;
use negkit::kernel::{self, DEFAULT_ANALYSIS_BUFFER};
use negkit::reference::srgb_oetf;
use negkit::{ExposureSettings, RgbImage};
use negraw::RawBackend;
use negsession::densitometer::NegativeCharacter;
use negsession::renderer::default_renderer;
use negsession::{
    CpuRenderer, ExportColorSpace, ExportDestination, ExportFormat, ExportOptions, Exporter, Frame,
    GpuRenderer, OutputKind, RenderRequest, RenderTier, Renderer, Session, SidecarStore,
};

const USAGE: &str = "usage:
  negcli decode <raw> [--full] -o out.png
  negcli thumb  <raw> -o out.jpg
  negcli render <raw> [--sidecar P | --stock] [--set key=value]... [--srgb]
                [--tier proxy|medium|full] [--uncropped] -o out.png|out.jpg|out.tiff
  negcli meter  <raw> [--sidecar P | --stock] [--set key=value]...
  negcli bench  <raw> [--runs N]
  negcli export <raw> [--jpeg|--tiff] [--srgb|--adobe] [--long-edge N] [--quality Q]
                [--sidecar P | --stock] -o <dir>";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Parsed command line: the subcommand, its positional, and `--flag [value]` pairs.
struct Args {
    command: String,
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

/// Flags that take a value.
const VALUE_FLAGS: [&str; 8] = [
    "o",
    "out",
    "sidecar",
    "set",
    "tier",
    "runs",
    "long-edge",
    "quality",
];
/// Boolean flags.
const BOOL_FLAGS: [&str; 8] = [
    "full",
    "srgb",
    "uncropped",
    "stock",
    "jpeg",
    "tiff",
    "adobe",
    "help",
];

impl Args {
    /// Strict: an unknown flag, or a value flag with no value, is an error (a typo like
    /// `--tif` must not silently become a default run).
    fn parse(argv: &[String]) -> std::result::Result<Args, String> {
        let mut it = argv.iter();
        let command = it.next().ok_or_else(|| USAGE.to_owned())?.clone();
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let rest: Vec<&String> = it.collect();
        let mut i = 0;
        while i < rest.len() {
            let a = rest[i].as_str();
            let name = match a.strip_prefix("--") {
                Some(name) => name,
                None if a == "-o" => "o",
                None if a == "-h" => "help",
                None if a.starts_with('-') && a.len() > 1 => {
                    return Err(format!("unknown flag `{a}`\n{USAGE}"));
                }
                None => {
                    positional.push(a.to_owned());
                    i += 1;
                    continue;
                }
            };
            if VALUE_FLAGS.contains(&name) {
                let Some(value) = rest.get(i + 1) else {
                    return Err(format!("--{name} needs a value\n{USAGE}"));
                };
                flags.push((name.to_owned(), Some((*value).clone())));
                i += 2;
            } else if BOOL_FLAGS.contains(&name) {
                flags.push((name.to_owned(), None));
                i += 1;
            } else {
                return Err(format!("unknown flag `--{name}`\n{USAGE}"));
            }
        }
        Ok(Args {
            command,
            positional,
            flags,
        })
    }

    /// A numeric flag, naming the flag in the error.
    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T> {
        match self.value(name) {
            None => Ok(default),
            Some(v) => v
                .parse::<T>()
                .map_err(|_| format!("--{name}: `{v}` is not a valid number").into()),
        }
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    fn values(&self, name: &str) -> Vec<&str> {
        self.flags
            .iter()
            .filter(|(n, _)| n == name)
            .filter_map(|(_, v)| v.as_deref())
            .collect()
    }

    fn raw_path(&self) -> Result<PathBuf> {
        let p = self.positional.first().ok_or("missing <raw> path")?;
        let path = PathBuf::from(p);
        if !path.is_file() {
            return Err(format!("{}: not a file", path.display()).into());
        }
        Ok(path)
    }

    fn output(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.value("o").ok_or("missing -o <output>")?))
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match Args::parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("negcli: {e}");
            return ExitCode::from(2);
        }
    };
    let run = match args.command.as_str() {
        "decode" => cmd_decode(&args),
        "thumb" => cmd_thumb(&args),
        "render" => cmd_render(&args),
        "meter" => cmd_meter(&args),
        "bench" => cmd_bench(&args),
        "export" => cmd_export(&args),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n{USAGE}").into()),
    };
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("negcli: {e}");
            ExitCode::FAILURE
        }
    }
}

fn backend() -> Arc<dyn RawBackend> {
    Arc::from(negraw::default_backend())
}

/// Settings for a frame: `--sidecar P` (explicit file), `--stock` (ignore sidecars), else the
/// next-to-source sidecar when present; then `--set key=value` overrides (sidecar keys, JSON
/// values — `--set grade=95 --set cropRect='{"x":0.1,"y":0.1,"width":0.8,"height":0.8}'`).
fn settings_for(args: &Args, raw: &Path) -> Result<ExposureSettings> {
    let mut settings = if args.has("stock") {
        ExposureSettings::default()
    } else if let Some(p) = args.value("sidecar") {
        let bytes = std::fs::read(p)?;
        SidecarStore::parse(&bytes).ok_or_else(|| format!("{p}: not a sidecar"))?
    } else {
        SidecarStore::load(raw).unwrap_or_default()
    };
    let sets = args.values("set");
    if !sets.is_empty() {
        let mut map = match serde_json::to_value(&settings)? {
            serde_json::Value::Object(m) => m,
            _ => return Err("settings did not encode as an object".into()),
        };
        // The two optional rects are omitted when absent; seed them so a --set can type-check.
        for rect_key in ["cropRect", "analysisRect"] {
            map.entry(rect_key).or_insert(serde_json::Value::Null);
        }
        for kv in sets {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("--set expects key=value, got `{kv}`"))?;
            if !ExposureSettings::STORED_KEYS.contains(&k) {
                return Err(format!("unknown settings key `{k}`").into());
            }
            let value: serde_json::Value = serde_json::from_str(v).map_err(|_| {
                format!(
                    "--set {k}: `{v}` is not a JSON value (numbers, true/false, [..], {{..}}, null)"
                )
            })?;
            let Some(current) = map.get(k) else {
                return Err(format!("--set {k}: unknown key").into());
            };
            if !same_json_kind(current, &value) {
                return Err(
                    format!("--set {k}: expected {} but got `{v}`", json_kind(current)).into(),
                );
            }
            map.insert(k.to_owned(), value);
        }
        settings = serde_json::from_value(serde_json::Value::Object(map))?;
    }
    Ok(settings)
}

/// Whether `candidate` has the JSON shape `current` has (numbers stay numbers, booleans stay
/// booleans, arrays arrays; the optional rects accept an object or `null`) — the lenient
/// sidecar decoder would otherwise silently default a mistyped `--set`.
fn same_json_kind(current: &serde_json::Value, candidate: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    matches!(
        (current, candidate),
        (Number(_), Number(_))
            | (Bool(_), Bool(_))
            | (Array(_), Array(_))
            | (Object(_) | Null, Object(_) | Null)
    )
}

fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::Bool(_) => "true/false",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) | serde_json::Value::Null => "a rect object or null",
        serde_json::Value::String(_) => "a string",
    }
}

fn write_png_rgba8(path: &Path, rgba: &[u8], w: usize, h: usize) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let enc = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file));
    enc.write_image(
        rgba,
        u32::try_from(w)?,
        u32::try_from(h)?,
        ExtendedColorType::Rgba8,
    )?;
    Ok(())
}

/// Scene-linear → a viewable 8-bit PNG (sRGB OETF, no colour management — a quick look).
fn write_png_linear(path: &Path, img: &RgbImage) -> Result<()> {
    let mut rgb = Vec::with_capacity(img.data.len());
    // Clamped to [0, 255.5) before the truncating cast.
    rgb.extend(
        img.data
            .iter()
            .map(|&v| (srgb_oetf(v.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8),
    );
    let file = std::fs::File::create(path)?;
    let enc = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file));
    enc.write_image(
        &rgb,
        u32::try_from(img.width)?,
        u32::try_from(img.height)?,
        ExtendedColorType::Rgb8,
    )?;
    Ok(())
}

fn cmd_decode(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let out = args.output()?;
    let be = backend();
    let t = Instant::now();
    let decoded = if args.has("full") {
        be.decode_full(&raw)?
    } else {
        be.decode_preview(&raw)?
    };
    let dt = t.elapsed();
    write_png_linear(&out, &decoded.image)?;
    println!(
        "decoded {}×{} ({}, {}, flip {}) in {:.0} ms → {}",
        decoded.image.width,
        decoded.image.height,
        be.name(),
        if args.has("full") { "full" } else { "preview" },
        decoded.flip,
        dt.as_secs_f64() * 1e3,
        out.display()
    );
    Ok(())
}

fn cmd_thumb(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let out = args.output()?;
    let bytes = backend().thumbnail(&raw)?.ok_or("no embedded thumbnail")?;
    std::fs::write(&out, &bytes)?;
    println!("thumbnail {} bytes → {}", bytes.len(), out.display());
    Ok(())
}

fn parse_tier(s: Option<&str>) -> Result<RenderTier> {
    Ok(match s.unwrap_or("proxy") {
        "proxy" => RenderTier::Proxy,
        "medium" => RenderTier::MediumIfFree,
        "full" => RenderTier::Full,
        other => return Err(format!("--tier: expected proxy|medium|full, got `{other}`").into()),
    })
}

fn cmd_render(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let out = args.output()?;
    let settings = settings_for(args, &raw)?;
    let renderer = default_renderer();
    let t = Instant::now();
    let mut session = Session::open(&raw, backend(), Arc::clone(&renderer))?;
    session.set_settings(settings);
    let t_open = t.elapsed();
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let srgb = args.has("srgb") || ext == "png";
    let tier = parse_tier(args.value("tier"))?;
    let t = Instant::now();
    match ext.as_str() {
        "png" => {
            let output = session.render(RenderRequest {
                tier,
                uncropped: args.has("uncropped"),
                srgb,
                ..Default::default()
            })?;
            let dt = t.elapsed();
            match &output.frame {
                Frame::Rgba8(bytes) => write_png_rgba8(&out, bytes, output.width, output.height)?,
                Frame::Float(_) => return Err("unexpected float frame".into()),
            }
            println!(
                "render {:?} {}×{} (open {:.0} ms, render {:.1} ms, {}) → {}",
                output.tier,
                output.width,
                output.height,
                t_open.as_secs_f64() * 1e3,
                dt.as_secs_f64() * 1e3,
                renderer.name(),
                out.display()
            );
        }
        "jpg" | "jpeg" | "tif" | "tiff" => {
            let format = if ext.starts_with("ti") {
                ExportFormat::Tiff16
            } else {
                ExportFormat::Jpeg
            };
            let colorspace = if srgb {
                ExportColorSpace::Srgb
            } else {
                ExportColorSpace::AdobeRgb
            };
            let output = session.render(RenderRequest {
                tier,
                uncropped: args.has("uncropped"),
                srgb,
                output: OutputKind::Float,
                compute_histogram: false,
                ..Default::default()
            })?;
            let dt = t.elapsed();
            let image = output.frame.image().ok_or("expected a float frame")?;
            let options = ExportOptions {
                format,
                color_space: colorspace,
                ..Default::default()
            };
            Exporter::write_encoded(image, &out, &options)?;
            println!(
                "render {:?} {}×{} {} ({:.1} ms) → {}",
                output.tier,
                output.width,
                output.height,
                colorspace.label(),
                dt.as_secs_f64() * 1e3,
                out.display()
            );
        }
        other => return Err(format!("-o: expected .png/.jpg/.tiff, got `.{other}`").into()),
    }
    Ok(())
}

fn fmt3(v: [f64; 3]) -> String {
    format!("[{:.4}, {:.4}, {:.4}]", v[0], v[1], v[2])
}

fn cmd_meter(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let settings = settings_for(args, &raw)?;
    let mut session = Session::open(&raw, backend(), default_renderer())?;
    session.set_settings(settings.clone());
    let analysis = session.analysis()?;
    let info = session.session_info()?;
    let params = derive_render_params(&settings, &analysis);
    let b = &analysis.base_bounds;
    println!("{}", raw.display());
    println!(
        "  proxy frame        {}×{} (medium tier: {})",
        info.width,
        info.height,
        if session.has_medium_tier() {
            "yes"
        } else {
            "no"
        }
    );
    println!("  base floors        {}", fmt3(b.floors));
    println!("  base ceils         {}", fmt3(b.ceils));
    println!("  channel ranges     {}", fmt3(b.ranges()));
    println!(
        "  density range      {:.3} D · {} (default grade range {:.3})",
        info.density_range,
        info.character.map_or("—", NegativeCharacter::label),
        info.default_grade_range
    );
    println!("  anchor             {:.4}", analysis.anchor);
    println!("  textural range     {:.4}", analysis.textural_range);
    println!(
        "  shadow / highlight {:.4} / {:.4}",
        analysis.shadow_point.unwrap_or(f64::NAN),
        analysis.highlight_point.unwrap_or(f64::NAN)
    );
    println!("  shadow refs        {}", fmt3(analysis.shadow_refs));
    match (analysis.neutral_mid, analysis.neutral_shadow) {
        (Some(m), Some(s)) => {
            println!("  neutral mid        {}", fmt3(m));
            println!("  neutral shadow     {}", fmt3(s));
            if let Some(h) = analysis.neutral_highlight {
                println!("  neutral highlight  {}", fmt3(h));
            }
            println!(
                "  cast confidence    {:.3}",
                analysis.neutral_confidence.unwrap_or(0.0)
            );
        }
        _ => println!("  neutral axis       none (shadow-ref fallback)"),
    }
    println!("  derived slopes     {}", fmt3(params.slopes));
    println!("  derived pivots     {}", fmt3(params.pivots));
    println!("  derived curvature  {}", fmt3(params.curvatures));
    println!("  cmy offsets        {}", fmt3(params.cmy_offsets));
    println!("  auto highlight     {:.4}", params.auto_highlight);
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed())
}

fn cmd_bench(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let runs: usize = args.number("runs", 5)?.max(1);
    let be = backend();
    let mut rows: Vec<Row> = Vec::new();

    // Stage timings on the proxy, outside the session (the tower hides them behind its keys);
    // renderer-independent, so they print once.
    let (native, t_decode) = time(|| be.decode_preview(&raw));
    let native = native?.image;
    row(
        &mut rows,
        "decode preview",
        t_decode,
        format!("{}×{} {}", native.width, native.height, be.name()),
    );
    let (proxy, t_down) = time(|| native.downsampled(1536).into_owned());
    row(
        &mut rows,
        "downsample → proxy",
        t_down,
        format!("{}×{}", proxy.width, proxy.height),
    );
    let (prepared, t_prep) = time(|| kernel::prepare(&proxy, None, None, DEFAULT_ANALYSIS_BUFFER));
    let prepared = prepared?;
    row(
        &mut rows,
        "prepare (analysis)",
        t_prep,
        format!("grid {}×{}", prepared.grid.width, prepared.grid.height),
    );
    let (analysis, t_fin) = time(|| kernel::finalize(&prepared));
    row(&mut rows, "finalize (neutral axis)", t_fin, String::new());
    let settings = ExposureSettings::default();
    let (params, t_derive) = time(|| derive_render_params(&settings, &analysis));
    rows.push((
        "derive_render_params".to_owned(),
        format!("{:.1} µs", t_derive.as_secs_f64() * 1e6),
        format!("slopes {}", fmt3(params.slopes)),
    ));
    drop(native);
    drop(proxy);

    // The session, once per renderer: CPU always, GPU when one comes up (NEG_GPU=off skips it).
    let cpu: Arc<dyn Renderer> = Arc::new(CpuRenderer);
    let gpu: Option<Arc<dyn Renderer>> = match default_renderer() {
        r if r.as_any().is_some_and(|a| a.is::<GpuRenderer>()) => Some(r),
        _ => None,
    };
    let cpu_rows = bench_session(&raw, &be, &cpu, runs)?;
    let gpu_rows = gpu
        .as_ref()
        .map(|g| bench_session(&raw, &be, g, runs))
        .transpose()?;

    println!("negcli bench — {} ({} runs)", raw.display(), runs);
    println!("  cpu: {}", cpu.name());
    match &gpu {
        Some(g) => println!("  gpu: {}", g.name()),
        None => println!(
            "  gpu: — ({})",
            neggpu::unavailable_reason().unwrap_or_else(|| "NEG_GPU=off".to_owned())
        ),
    }
    println!();
    let w0 = rows
        .iter()
        .map(|r| r.0.len())
        .chain(cpu_rows.iter().map(|r| r.0.len()))
        .max()
        .unwrap_or(10);
    let w1 = rows
        .iter()
        .map(|r| r.1.len())
        .chain(cpu_rows.iter().map(|r| r.1.len()))
        .max()
        .unwrap_or(8)
        .max(4);
    let w2 = gpu_rows
        .iter()
        .flatten()
        .map(|r| r.1.len())
        .max()
        .unwrap_or(1)
        .max(4);
    let rule = |a: usize, b: usize, c: usize| {
        println!(
            "{}  {}  {}  {}",
            "-".repeat(a),
            "-".repeat(b),
            "-".repeat(c),
            "-".repeat(40)
        )
    };
    println!(
        "{:<w0$}  {:>w1$}  {:>w2$}  note",
        "analysis stage", "time", ""
    );
    rule(w0, w1, w2);
    for (s, t, n) in &rows {
        println!("{s:<w0$}  {t:>w1$}  {:>w2$}  {n}", "");
    }
    println!();
    println!(
        "{:<w0$}  {:>w1$}  {:>w2$}  note",
        "session stage", "cpu", "gpu"
    );
    rule(w0, w1, w2);
    for (i, (s, t, n)) in cpu_rows.iter().enumerate() {
        let (g, gn) = gpu_rows
            .as_ref()
            .and_then(|g| g.get(i))
            .map_or(("—", None), |r| (r.1.as_str(), Some(r.2.as_str())));
        // The GPU pass's note wins when it differs (its own fps / memory); the CPU's otherwise.
        let note = match gn {
            Some(gn) if gn != n && !gn.is_empty() => format!("{n}  ‖ gpu: {gn}"),
            _ => n.clone(),
        };
        println!("{s:<w0$}  {t:>w1$}  {g:>w2$}  {note}");
    }
    Ok(())
}

/// One renderer's session pass: open, per-tier display renders (first = cold, then the
/// slider-tick average), the export render, the JPEG write, resident memory. The rows come
/// back in a fixed order so the CPU and GPU passes line up column by column.
fn bench_session(
    raw: &Path,
    be: &Arc<dyn RawBackend>,
    renderer: &Arc<dyn Renderer>,
    runs: usize,
) -> Result<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    let (session, t_open) = time(|| Session::open(raw, Arc::clone(be), Arc::clone(renderer)));
    let mut session = session?;
    row(
        &mut rows,
        "Session::open (decode + proxy)",
        t_open,
        format!("{} resident", human(session.bytes_resident())),
    );
    let tiers: Vec<(&str, RenderTier)> = vec![
        ("proxy", RenderTier::Proxy),
        ("medium", RenderTier::MediumIfFree),
        ("full", RenderTier::Full),
    ];
    for (name, tier) in tiers {
        if tier == RenderTier::MediumIfFree && !session.has_medium_tier() {
            rows.push((
                "render_display medium".to_owned(),
                "—".to_owned(),
                "no medium tier on this body".to_owned(),
            ));
            continue;
        }
        let req = RenderRequest {
            tier,
            retain_full: true,
            srgb: true,
            ..Default::default()
        };
        let (first, t_first) = time(|| session.render(req));
        let first = first?;
        let n = if tier == RenderTier::Full {
            runs.min(3)
        } else {
            runs
        };
        let mut total = Duration::ZERO;
        for i in 0..n {
            session.update_settings(|s| s.density = 1.0 + 0.01 * (i as f64 + 1.0));
            let (r, dt) = time(|| session.render(req));
            r?;
            total += dt;
        }
        let avg = total / u32::try_from(n)?;
        row(
            &mut rows,
            &format!("render_display {name}"),
            avg,
            format!(
                "{}×{} · first {:.1} ms (incl. {}) · {:.0} fps",
                first.width,
                first.height,
                ms(t_first),
                if tier == RenderTier::Full {
                    "full decode + orient"
                } else {
                    "orient + source"
                },
                1.0 / avg.as_secs_f64().max(1e-9)
            ),
        );
        session.update_settings(|s| s.density = 1.0);
    }
    let (encoded, t_export) = time(|| session.export_render(true));
    let encoded = encoded?;
    row(
        &mut rows,
        "export render (float, sRGB)",
        t_export,
        format!("{}×{} (retained full tier)", encoded.width, encoded.height),
    );
    let dir = std::env::temp_dir();
    let out = TempFile(dir.join(format!("negcli-bench-{}.jpg", std::process::id())));
    let (w, t_jpeg) = time(|| {
        Exporter::write_encoded(
            &encoded,
            &out.0,
            &ExportOptions {
                resize: true,
                max_long_edge: 3000,
                ..Default::default()
            },
        )
    });
    w?;
    row(
        &mut rows,
        "export write JPEG (3000 px)",
        t_jpeg,
        out.0.display().to_string(),
    );
    rows.push((
        "memory".to_owned(),
        human(session.bytes_resident()),
        format!("{:?}", session.stats()),
    ));
    Ok(rows)
}

/// A scratch file removed on every exit path (early `?` returns included).
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

type Row = (String, String, String);

fn row(rows: &mut Vec<Row>, stage: &str, d: Duration, note: String) {
    rows.push((stage.to_owned(), format!("{:.1} ms", ms(d)), note));
}

fn human(bytes: usize) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

/// `--quality`: 0.5…1.0 like the export sheet's slider (or the same range as a percentage,
/// 50…100); anything else is rejected rather than silently rescaled.
fn parse_quality(q: f64) -> Result<f64> {
    let (lo, hi) = negsession::export::JPEG_QUALITY_RANGE;
    if (lo..=hi).contains(&q) {
        Ok(q)
    } else if (lo * 100.0..=hi * 100.0).contains(&q) {
        Ok(q / 100.0)
    } else {
        Err(format!(
            "--quality: {q} is outside {lo}…{hi} (or {}…{} %)",
            lo * 100.0,
            hi * 100.0
        )
        .into())
    }
}

fn cmd_export(args: &Args) -> Result<()> {
    let raw = args.raw_path()?;
    let dir = args.output()?;
    std::fs::create_dir_all(&dir)?;
    let settings = settings_for(args, &raw)?;
    let quality = parse_quality(args.number("quality", negsession::export::DEFAULT_JPEG_QUALITY)?)?;
    let format = if args.has("tiff") {
        ExportFormat::Tiff16
    } else {
        ExportFormat::Jpeg
    };
    let colorspace = if args.has("adobe") {
        ExportColorSpace::AdobeRgb
    } else {
        ExportColorSpace::Srgb
    };
    let long_edge: Option<u32> = match args.value("long-edge") {
        Some(_) => Some(args.number("long-edge", 0u32)?),
        None => None,
    };
    if let Some(n) = long_edge {
        let (lo, hi) = negsession::export::LONG_EDGE_RANGE;
        if !(lo..=hi).contains(&n) {
            return Err(format!("--long-edge: {n} is outside {lo}…{hi}").into());
        }
    }
    let options = ExportOptions {
        format,
        jpeg_quality: quality,
        resize: long_edge.is_some(),
        max_long_edge: long_edge.unwrap_or(negsession::export::DEFAULT_MAX_LONG_EDGE),
        color_space: colorspace,
        ..Default::default()
    }
    .with_destination(ExportDestination::Folder(dir));
    let mut session = Session::open(&raw, backend(), default_renderer())?;
    session.set_settings(settings);
    let t = Instant::now();
    let written = Exporter::write(&mut session, &options)?;
    println!(
        "exported {} {} ({:.0} ms) → {}",
        format.label(),
        colorspace.label(),
        ms(t.elapsed()),
        written.display()
    );
    Ok(())
}
