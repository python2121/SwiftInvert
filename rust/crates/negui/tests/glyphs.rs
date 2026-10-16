//! Every non-ASCII symbol in a UI string literal must be drawable by the bundled fonts (review
//! 8 #16: `◧ ▤ ◉ ▦ ⌗ ↷ ↶` drew as boxes). Scans `src/` so a new glyph is checked automatically.

/// Glyphs egui reports as "missing" because they live in the SAME face as its replacement
/// character (Noto Emoji) — `has_glyph`'s known false negative. Verified by the selftest PNGs.
const IN_REPLACEMENT_FACE: &[char] = &['↔', '✔'];

fn ui_symbols() -> std::collections::BTreeSet<char> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![];
    for dir in [src.clone(), src.join("dialogs")] {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "rs") {
                files.push(e.path());
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for f in files {
        for line in std::fs::read_to_string(&f).unwrap().lines() {
            let t = line.trim_start();
            if t.starts_with("//") || t.contains("log::") {
                continue;
            }
            // characters inside string literals only
            let mut inside = false;
            for c in line.chars() {
                if c == '"' {
                    inside = !inside;
                } else if inside && !c.is_ascii() {
                    out.insert(c);
                }
            }
        }
    }
    out
}

#[test]
fn every_ui_symbol_has_a_glyph() {
    let ctx = egui::Context::default();
    negui::theme::install(&ctx);
    let mut o = ctx.run_ui(Default::default(), |_| {});
    o.textures_delta.clear();
    let symbols = ui_symbols();
    assert!(symbols.contains(&'·'), "the scan found the UI strings");
    let missing: String = symbols
        .iter()
        .filter(|c| !IN_REPLACEMENT_FACE.contains(c))
        .filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(13.0), **c)))
        .collect();
    assert!(missing.is_empty(), "no glyph for: {missing}");
}

/// Tooltips spell shortcuts like the menus (`format_shortcut`): the platform's modifier names,
/// never hard-coded Mac symbols on Linux/Windows.
#[test]
fn tooltip_shortcuts_use_the_platform_modifier_names() {
    use negui::app::{Command, with_shortcut};
    let ctx = egui::Context::default();
    // `format_shortcut` consults the loaded fonts on macOS (can the font draw ⌘?), which only
    // exist inside a frame — a bare Context panicked there on the Mac.
    let mut out = (String::new(), String::new(), String::new());
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        out = (
            with_shortcut(ctx, "Undo", Command::Undo),
            with_shortcut(ctx, "Redo", Command::Redo),
            with_shortcut(ctx, "Test strip", Command::TestStrip),
        );
    });
    let (undo, redo, strip) = out;
    if ctx.os() == egui::os::OperatingSystem::Mac {
        // symbols when the font has them, egui's textual fallback otherwise
        assert!(
            undo == "Undo (⌘Z)" || undo == "Undo (Cmd+Z)",
            "unexpected Mac spelling: {undo}"
        );
    } else {
        assert_eq!(undo, "Undo (Ctrl+Z)");
        assert_eq!(redo, "Redo (Ctrl+Shift+Z)");
        assert_eq!(strip, "Test strip (Shift+T)");
    }
    // no UI string literal hard-codes a Mac modifier symbol
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for dir in [src.clone(), src.join("dialogs")] {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//") || !t.contains('"') {
                    continue;
                }
                let lit: String = line.split('"').skip(1).step_by(2).collect();
                assert!(
                    !lit.contains(['⌘', '⇧', '⌥']),
                    "{}:{}: hard-coded modifier symbol",
                    e.path().display(),
                    n + 1
                );
            }
        }
    }
}
