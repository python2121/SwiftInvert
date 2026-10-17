//! System UI font discovery and the font fallback chain.

use negui::system_font::{parse_gsettings_font, parse_kde_font, validate};

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

#[test]
fn bundled_inter_is_a_valid_ui_font() {
    assert!(validate(INTER, 0).is_ok());
}

#[test]
fn garbage_is_refused_before_egui_can_panic_on_it() {
    assert!(validate(b"not a font at all", 0).is_err());
    assert!(validate(&[], 0).is_err());
    // a valid file with a face index past its end
    assert!(validate(INTER, 7).is_err());
}

#[test]
fn kde_font_entry_is_read_from_the_general_group_only() {
    let cfg = "[Colors:View]\nfont=Wrong,1\n\n[General]\nColorScheme=BreezeDark\nfont=Inter Variable,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1\n";
    assert_eq!(parse_kde_font(cfg).as_deref(), Some("Inter Variable"));
    assert_eq!(parse_kde_font("[General]\nColorScheme=Breeze\n"), None);
    assert_eq!(parse_kde_font(""), None);
}

#[test]
fn gsettings_font_name_drops_quotes_and_size() {
    assert_eq!(
        parse_gsettings_font("'Adwaita Sans 11'\n").as_deref(),
        Some("Adwaita Sans")
    );
    assert_eq!(
        parse_gsettings_font("'Cantarell 10.5'").as_deref(),
        Some("Cantarell")
    );
    assert_eq!(parse_gsettings_font("'Inter'").as_deref(), Some("Inter"));
    assert_eq!(parse_gsettings_font("''"), None);
}

/// Whatever this machine offers, discovery never hands egui something it can't parse — and the
/// theme installs either way (system face or the Inter fallback) without panicking.
#[test]
fn discovery_returns_a_parsable_font_or_none_and_the_theme_installs() {
    if let Some(font) = negui::system_font::discover() {
        assert!(validate(&font.bytes, font.index).is_ok(), "{font:?}");
        eprintln!("system UI font: {}", font.description);
    } else {
        eprintln!("no system UI font on this machine; the bundled Inter is used");
    }
    for choice in [
        negui::theme::FontChoice::System,
        negui::theme::FontChoice::Bundled,
    ] {
        let ctx = egui::Context::default();
        let face = negui::theme::install_with(&ctx, choice);
        assert!(!face.is_empty());
        // fonts are built at the next frame: run one to make sure they parse
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("SwiftInvert ↶ ↷ ▸ ⇧");
        });
        // a test has no renderer to hand the font atlas to (egui debug-asserts on dropping it)
        out.textures_delta.clear();
    }
}
