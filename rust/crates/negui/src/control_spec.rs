//! The static table behind the adjustments sidebar — `ControlsSidebar.swift`,
//! `ColorMixerSection.swift`, `ColorGradingSection.swift` as data (controls-spec §3). Every
//! slider and toggle the Mac sidebar shows is one row here: settings field, label, section,
//! range, default, value format, display transform, disabled rule and gradient track. The UI
//! renders the table; the tests sweep it against `ExposureSettings::STORED_KEYS` so a new
//! settings field cannot ship without a row (or an explicit "no UI" entry).

use negkit::ExposureSettings;

/// Sidebar groups, in on-screen order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    PreProcess,
    Print,
    Exposure,
    Color,
    ColorMixer,
    ColorGrading,
    Tone,
}

impl Section {
    /// Group box title.
    pub fn label(self) -> &'static str {
        match self {
            Section::PreProcess => "Pre-process",
            Section::Print => "Print",
            Section::Exposure => "Exposure",
            Section::Color => "Color",
            Section::ColorMixer => "Color Mixer",
            Section::ColorGrading => "Color Grading",
            Section::Tone => "Tone",
        }
    }
}

/// Color Mixer band (`ColorMixerSection.Band`), default red.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MixerBand {
    #[default]
    Red,
    Yellow,
    Green,
    Blue,
}

impl MixerBand {
    pub const ALL: [MixerBand; 4] = [
        MixerBand::Red,
        MixerBand::Yellow,
        MixerBand::Green,
        MixerBand::Blue,
    ];

    /// Segmented-picker label (Mac wording).
    pub fn label(self) -> &'static str {
        match self {
            MixerBand::Red => "Red",
            MixerBand::Yellow => "Yellow",
            MixerBand::Green => "Green",
            MixerBand::Blue => "Blue",
        }
    }

    /// Hue track colours [−, band, +] (ColorMixerSection.swift:40-67, sRGB floats).
    pub fn hue_colors(self) -> [[f32; 3]; 3] {
        match self {
            MixerBand::Red => [[0.85, 0.2, 0.5], [0.85, 0.2, 0.2], [0.9, 0.55, 0.15]],
            MixerBand::Yellow => [[0.9, 0.55, 0.15], [0.88, 0.8, 0.2], [0.6, 0.82, 0.25]],
            MixerBand::Green => [[0.6, 0.82, 0.25], [0.25, 0.72, 0.3], [0.15, 0.72, 0.6]],
            MixerBand::Blue => [[0.15, 0.68, 0.8], [0.25, 0.45, 0.88], [0.55, 0.3, 0.85]],
        }
    }
}

/// Color Grading band (`ColorGradingSection.Band`), default mids.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GradingBand {
    Shadows,
    #[default]
    Mids,
    Highs,
}

impl GradingBand {
    pub const ALL: [GradingBand; 3] = [GradingBand::Shadows, GradingBand::Mids, GradingBand::Highs];

    pub fn label(self) -> &'static str {
        match self {
            GradingBand::Shadows => "Shadows",
            GradingBand::Mids => "Mids",
            GradingBand::Highs => "Highs",
        }
    }
}

/// Which band picker (if any) gates a row's visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandTag {
    None,
    Mixer(MixerBand),
    Grading(GradingBand),
}

/// A numeric settings field (the slider's storage).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Density,
    Grade,
    ContrastMask,
    MaskSpacer,
    ExposureStops,
    OverallContrast,
    Shadows,
    ShadowContrast,
    DarkShadows,
    Highlights,
    HighlightContrast,
    PreSaturation,
    CastRemovalStrength,
    HueTrim,
    PrintSaturation,
    SeparationDamping,
    Vibrance,
    Saturation,
    SkinProtection,
    RedHue,
    RedSaturation,
    YellowHue,
    YellowSaturation,
    GreenHue,
    GreenSaturation,
    BlueHue,
    BlueSaturation,
    Temp,
    Tint,
    /// `colorShadows[i]`.
    ColorShadows(usize),
    ColorMids(usize),
    ColorHighs(usize),
    Toe,
    Shoulder,
    WhitePointOffset,
    BlackPointOffset,
}

impl Field {
    /// The sidecar key (`ExposureSettings::STORED_KEYS` spelling).
    pub fn key(self) -> &'static str {
        match self {
            Field::Density => "density",
            Field::Grade => "grade",
            Field::ContrastMask => "contrastMask",
            Field::MaskSpacer => "maskSpacer",
            Field::ExposureStops => "exposureStops",
            Field::OverallContrast => "overallContrast",
            Field::Shadows => "shadows",
            Field::ShadowContrast => "shadowContrast",
            Field::DarkShadows => "darkShadows",
            Field::Highlights => "highlights",
            Field::HighlightContrast => "highlightContrast",
            Field::PreSaturation => "preSaturation",
            Field::CastRemovalStrength => "castRemovalStrength",
            Field::HueTrim => "hueTrim",
            Field::PrintSaturation => "printSaturation",
            Field::SeparationDamping => "separationDamping",
            Field::Vibrance => "vibrance",
            Field::Saturation => "saturation",
            Field::SkinProtection => "skinProtection",
            Field::RedHue => "redHue",
            Field::RedSaturation => "redSaturation",
            Field::YellowHue => "yellowHue",
            Field::YellowSaturation => "yellowSaturation",
            Field::GreenHue => "greenHue",
            Field::GreenSaturation => "greenSaturation",
            Field::BlueHue => "blueHue",
            Field::BlueSaturation => "blueSaturation",
            Field::Temp => "temp",
            Field::Tint => "tint",
            Field::ColorShadows(_) => "colorShadows",
            Field::ColorMids(_) => "colorMids",
            Field::ColorHighs(_) => "colorHighs",
            Field::Toe => "toe",
            Field::Shoulder => "shoulder",
            Field::WhitePointOffset => "whitePointOffset",
            Field::BlackPointOffset => "blackPointOffset",
        }
    }

    /// Read the stored value.
    pub fn get(self, s: &ExposureSettings) -> f64 {
        let comp = |v: &[f64; 3], i: usize| v.get(i).copied().unwrap_or(0.0);
        match self {
            Field::Density => s.density,
            Field::Grade => s.grade,
            Field::ContrastMask => s.contrast_mask,
            Field::MaskSpacer => s.mask_spacer,
            Field::ExposureStops => s.exposure_stops,
            Field::OverallContrast => s.overall_contrast,
            Field::Shadows => s.shadows,
            Field::ShadowContrast => s.shadow_contrast,
            Field::DarkShadows => s.dark_shadows,
            Field::Highlights => s.highlights,
            Field::HighlightContrast => s.highlight_contrast,
            Field::PreSaturation => s.pre_saturation,
            Field::CastRemovalStrength => s.cast_removal_strength,
            Field::HueTrim => s.hue_trim,
            Field::PrintSaturation => s.print_saturation,
            Field::SeparationDamping => s.separation_damping,
            Field::Vibrance => s.vibrance,
            Field::Saturation => s.saturation,
            Field::SkinProtection => s.skin_protection,
            Field::RedHue => s.red_hue,
            Field::RedSaturation => s.red_saturation,
            Field::YellowHue => s.yellow_hue,
            Field::YellowSaturation => s.yellow_saturation,
            Field::GreenHue => s.green_hue,
            Field::GreenSaturation => s.green_saturation,
            Field::BlueHue => s.blue_hue,
            Field::BlueSaturation => s.blue_saturation,
            Field::Temp => s.temp,
            Field::Tint => s.tint,
            Field::ColorShadows(i) => comp(&s.color_shadows, i),
            Field::ColorMids(i) => comp(&s.color_mids, i),
            Field::ColorHighs(i) => comp(&s.color_highs, i),
            Field::Toe => s.toe,
            Field::Shoulder => s.shoulder,
            Field::WhitePointOffset => s.white_point_offset,
            Field::BlackPointOffset => s.black_point_offset,
        }
    }

    /// Write the stored value.
    pub fn set(self, s: &mut ExposureSettings, v: f64) {
        let comp = |a: &mut [f64; 3], i: usize| {
            if let Some(slot) = a.get_mut(i) {
                *slot = v;
            }
        };
        match self {
            Field::Density => s.density = v,
            Field::Grade => s.grade = v,
            Field::ContrastMask => s.contrast_mask = v,
            Field::MaskSpacer => s.mask_spacer = v,
            Field::ExposureStops => s.exposure_stops = v,
            Field::OverallContrast => s.overall_contrast = v,
            Field::Shadows => s.shadows = v,
            Field::ShadowContrast => s.shadow_contrast = v,
            Field::DarkShadows => s.dark_shadows = v,
            Field::Highlights => s.highlights = v,
            Field::HighlightContrast => s.highlight_contrast = v,
            Field::PreSaturation => s.pre_saturation = v,
            Field::CastRemovalStrength => s.cast_removal_strength = v,
            Field::HueTrim => s.hue_trim = v,
            Field::PrintSaturation => s.print_saturation = v,
            Field::SeparationDamping => s.separation_damping = v,
            Field::Vibrance => s.vibrance = v,
            Field::Saturation => s.saturation = v,
            Field::SkinProtection => s.skin_protection = v,
            Field::RedHue => s.red_hue = v,
            Field::RedSaturation => s.red_saturation = v,
            Field::YellowHue => s.yellow_hue = v,
            Field::YellowSaturation => s.yellow_saturation = v,
            Field::GreenHue => s.green_hue = v,
            Field::GreenSaturation => s.green_saturation = v,
            Field::BlueHue => s.blue_hue = v,
            Field::BlueSaturation => s.blue_saturation = v,
            Field::Temp => s.temp = v,
            Field::Tint => s.tint = v,
            Field::ColorShadows(i) => comp(&mut s.color_shadows, i),
            Field::ColorMids(i) => comp(&mut s.color_mids, i),
            Field::ColorHighs(i) => comp(&mut s.color_highs, i),
            Field::Toe => s.toe = v,
            Field::Shoulder => s.shoulder = v,
            Field::WhitePointOffset => s.white_point_offset = v,
            Field::BlackPointOffset => s.black_point_offset = v,
        }
    }
}

/// A boolean settings field (a toggle row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Toggle {
    AutoExposure,
    AutoNormalizeContrast,
    TrueBlack,
}

impl Toggle {
    pub fn key(self) -> &'static str {
        match self {
            Toggle::AutoExposure => "autoExposure",
            Toggle::AutoNormalizeContrast => "autoNormalizeContrast",
            Toggle::TrueBlack => "trueBlack",
        }
    }

    pub fn get(self, s: &ExposureSettings) -> bool {
        match self {
            Toggle::AutoExposure => s.auto_exposure,
            Toggle::AutoNormalizeContrast => s.auto_normalize_contrast,
            Toggle::TrueBlack => s.true_black,
        }
    }

    pub fn set(self, s: &mut ExposureSettings, v: bool) {
        match self {
            Toggle::AutoExposure => s.auto_exposure = v,
            Toggle::AutoNormalizeContrast => s.auto_normalize_contrast = v,
            Toggle::TrueBlack => s.true_black = v,
        }
    }
}

/// Value text format (the Mac `String(format:)` strings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    /// `%.Nf`
    Fixed(usize),
    /// `%+.Nf`
    Signed(usize),
    /// `%.1f°`
    Degrees,
    /// `%.1f%%`
    Percent,
}

impl Fmt {
    /// Format `v` (in display units).
    pub fn format(self, v: f64) -> String {
        match self {
            Fmt::Fixed(n) => format!("{v:.n$}"),
            Fmt::Signed(n) => format!("{v:+.n$}"),
            Fmt::Degrees => format!("{v:.1}°"),
            Fmt::Percent => format!("{v:.1}%"),
        }
    }
}

/// Stored ↔ displayed value transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    Plain,
    /// Brightness = `2 − density`, right = brighter (ControlsSidebar.swift:359-363).
    Brightness,
}

impl Display {
    pub fn to_shown(self, stored: f64) -> f64 {
        match self {
            Display::Plain => stored,
            Display::Brightness => 2.0 - stored,
        }
    }

    pub fn to_stored(self, shown: f64) -> f64 {
        match self {
            Display::Plain => shown,
            Display::Brightness => 2.0 - shown,
        }
    }
}

/// When a row is greyed out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Disabled {
    Never,
    /// Disabled while `field == value` (within 1e-9).
    WhenEquals(Field, f64),
}

/// How the groove is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    /// Plain groove (`LabeledSlider`).
    Plain,
    /// `GradientSlider` with these stops, leading → trailing (sRGB floats).
    Gradient3([[f32; 3]; 3]),
    Gradient2([[f32; 3]; 2]),
}

/// One slider row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSpec {
    /// Stable id (`print.brightness`, `mixer.red.hue`…) for tests, the control channel and ui ids.
    pub id: &'static str,
    pub field: Field,
    pub label: &'static str,
    pub section: Section,
    /// Range in DISPLAY units.
    pub min: f64,
    pub max: f64,
    /// Default (= reset-⨯ target) in DISPLAY units.
    pub default: f64,
    pub fmt: Fmt,
    pub display: Display,
    pub disabled: Disabled,
    pub track: Track,
    pub band: BandTag,
}

impl ControlSpec {
    /// The displayed value.
    pub fn shown(&self, s: &ExposureSettings) -> f64 {
        self.display.to_shown(self.field.get(s))
    }

    /// Store a displayed value (clamped to the range; non-finite → default).
    pub fn apply(&self, s: &mut ExposureSettings, shown: f64) {
        let v = if shown.is_finite() {
            shown.clamp(self.min, self.max)
        } else {
            self.default
        };
        self.field.set(s, self.display.to_stored(v));
    }

    /// `abs(value − default) > 1e-9` (the reset-⨯ rule).
    pub fn is_changed(&self, s: &ExposureSettings) -> bool {
        (self.shown(s) - self.default).abs() > 1e-9
    }

    /// The disabled rule evaluated on `s`.
    pub fn is_disabled(&self, s: &ExposureSettings) -> bool {
        match self.disabled {
            Disabled::Never => false,
            Disabled::WhenEquals(f, v) => (f.get(s) - v).abs() <= 1e-9,
        }
    }

    /// Value text.
    pub fn format(&self, shown: f64) -> String {
        self.fmt.format(shown)
    }

    /// 0…1 track position of a displayed value.
    pub fn position(&self, shown: f64) -> f64 {
        if self.max <= self.min {
            return 0.0;
        }
        ((shown - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    /// Displayed value at a 0…1 track position.
    pub fn value_at(&self, t: f64) -> f64 {
        self.min + (self.max - self.min) * t.clamp(0.0, 1.0)
    }
}

/// One toggle row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleSpec {
    pub id: &'static str,
    pub toggle: Toggle,
    pub label: &'static str,
    pub section: Section,
    pub default: bool,
}

const W04: [f32; 3] = [0.4, 0.4, 0.4];

const fn plain(
    id: &'static str,
    field: Field,
    label: &'static str,
    section: Section,
    min: f64,
    max: f64,
    default: f64,
    fmt: Fmt,
) -> ControlSpec {
    ControlSpec {
        id,
        field,
        label,
        section,
        min,
        max,
        default,
        fmt,
        display: Display::Plain,
        disabled: Disabled::Never,
        track: Track::Plain,
        band: BandTag::None,
    }
}

const fn mixer(band: MixerBand, hue: Field, sat: Field, prefix: &'static str) -> [ControlSpec; 2] {
    let hc = band.hue_colors_const();
    [
        ControlSpec {
            id: prefix_id(prefix, 0),
            field: hue,
            label: "Hue",
            section: Section::ColorMixer,
            min: -1.5,
            max: 1.5,
            default: 0.0,
            fmt: Fmt::Fixed(2),
            display: Display::Plain,
            disabled: Disabled::Never,
            track: Track::Gradient3(hc),
            band: BandTag::Mixer(band),
        },
        ControlSpec {
            id: prefix_id(prefix, 1),
            field: sat,
            label: "Saturation",
            section: Section::ColorMixer,
            min: 0.0,
            max: 2.0,
            default: 1.0,
            fmt: Fmt::Fixed(2),
            display: Display::Plain,
            disabled: Disabled::Never,
            track: Track::Gradient2([W04, hc[1]]),
            band: BandTag::Mixer(band),
        },
    ]
}

const fn prefix_id(prefix: &'static str, which: usize) -> &'static str {
    // const string concat is not available; the ids are spelled out per band below.
    match (prefix.as_bytes()[0], which) {
        (b'r', 0) => "mixer.red.hue",
        (b'r', _) => "mixer.red.saturation",
        (b'y', 0) => "mixer.yellow.hue",
        (b'y', _) => "mixer.yellow.saturation",
        (b'g', 0) => "mixer.green.hue",
        (b'g', _) => "mixer.green.saturation",
        (_, 0) => "mixer.blue.hue",
        (_, _) => "mixer.blue.saturation",
    }
}

impl MixerBand {
    const fn hue_colors_const(self) -> [[f32; 3]; 3] {
        match self {
            MixerBand::Red => [[0.85, 0.2, 0.5], [0.85, 0.2, 0.2], [0.9, 0.55, 0.15]],
            MixerBand::Yellow => [[0.9, 0.55, 0.15], [0.88, 0.8, 0.2], [0.6, 0.82, 0.25]],
            MixerBand::Green => [[0.6, 0.82, 0.25], [0.25, 0.72, 0.3], [0.15, 0.72, 0.6]],
            MixerBand::Blue => [[0.15, 0.68, 0.8], [0.25, 0.45, 0.88], [0.55, 0.3, 0.85]],
        }
    }
}

const GRADE_RC: [[f32; 3]; 3] = [[0.85, 0.2, 0.2], W04, [0.1, 0.75, 0.8]];
const GRADE_GM: [[f32; 3]; 3] = [[0.2, 0.75, 0.25], W04, [0.8, 0.2, 0.7]];
const GRADE_BY: [[f32; 3]; 3] = [[0.25, 0.35, 0.85], W04, [0.8, 0.75, 0.2]];

const fn grading_row(
    band: GradingBand,
    id: &'static str,
    field: Field,
    label: &'static str,
    track: [[f32; 3]; 3],
) -> ControlSpec {
    ControlSpec {
        id,
        field,
        label,
        section: Section::ColorGrading,
        min: -1.0,
        max: 1.0,
        default: 0.0,
        fmt: Fmt::Fixed(2),
        display: Display::Plain,
        disabled: Disabled::Never,
        track: Track::Gradient3(track),
        band: BandTag::Grading(band),
    }
}

const fn grading(band: GradingBand, fields: [Field; 3], ids: [&'static str; 3]) -> [ControlSpec; 3] {
    [
        grading_row(band, ids[0], fields[0], "R ↔ C", GRADE_RC),
        grading_row(band, ids[1], fields[1], "G ↔ M", GRADE_GM),
        grading_row(band, ids[2], fields[2], "B ↔ Y", GRADE_BY),
    ]
}

const MIXER_RED: [ControlSpec; 2] = mixer(MixerBand::Red, Field::RedHue, Field::RedSaturation, "r");
const MIXER_YELLOW: [ControlSpec; 2] =
    mixer(MixerBand::Yellow, Field::YellowHue, Field::YellowSaturation, "y");
const MIXER_GREEN: [ControlSpec; 2] =
    mixer(MixerBand::Green, Field::GreenHue, Field::GreenSaturation, "g");
const MIXER_BLUE: [ControlSpec; 2] = mixer(MixerBand::Blue, Field::BlueHue, Field::BlueSaturation, "b");
const GRADING_SHADOWS: [ControlSpec; 3] = grading(
    GradingBand::Shadows,
    [Field::ColorShadows(0), Field::ColorShadows(1), Field::ColorShadows(2)],
    ["grading.shadows.rc", "grading.shadows.gm", "grading.shadows.by"],
);
const GRADING_MIDS: [ControlSpec; 3] = grading(
    GradingBand::Mids,
    [Field::ColorMids(0), Field::ColorMids(1), Field::ColorMids(2)],
    ["grading.mids.rc", "grading.mids.gm", "grading.mids.by"],
);
const GRADING_HIGHS: [ControlSpec; 3] = grading(
    GradingBand::Highs,
    [Field::ColorHighs(0), Field::ColorHighs(1), Field::ColorHighs(2)],
    ["grading.highs.rc", "grading.highs.gm", "grading.highs.by"],
);

/// Every slider row, in on-screen order (band-gated rows listed per band).
pub static CONTROLS: &[ControlSpec] = &[
    // Print
    ControlSpec {
        id: "print.brightness",
        field: Field::Density,
        label: "Brightness",
        section: Section::Print,
        min: -3.0,
        max: 4.0,
        default: 1.0,
        fmt: Fmt::Fixed(2),
        display: Display::Brightness,
        disabled: Disabled::Never,
        track: Track::Plain,
        band: BandTag::None,
    },
    plain("print.grade", Field::Grade, "Grade (ISO R)", Section::Print, 50.0, 180.0, 115.0, Fmt::Fixed(0)),
    plain("print.contrastMask", Field::ContrastMask, "Contrast Mask", Section::Print, -0.5, 0.5, 0.0, Fmt::Signed(2)),
    ControlSpec {
        id: "print.maskSpacer",
        field: Field::MaskSpacer,
        label: "Mask Spacer",
        section: Section::Print,
        min: 2.0,
        max: 6.0,
        default: 4.0,
        fmt: Fmt::Percent,
        display: Display::Plain,
        disabled: Disabled::WhenEquals(Field::ContrastMask, 0.0),
        track: Track::Plain,
        band: BandTag::None,
    },
    // Exposure
    plain("exposure.stops", Field::ExposureStops, "Exposure (stops)", Section::Exposure, -2.0, 2.0, 0.0, Fmt::Signed(2)),
    plain("exposure.contrast", Field::OverallContrast, "Contrast", Section::Exposure, -1.0, 2.0, 0.0, Fmt::Fixed(2)),
    plain("exposure.shadows", Field::Shadows, "Shadows", Section::Exposure, -2.0, 2.0, 0.0, Fmt::Fixed(2)),
    plain("exposure.shadowContrast", Field::ShadowContrast, "Shadow contrast", Section::Exposure, -3.0, 6.0, 0.0, Fmt::Fixed(2)),
    plain("exposure.darkShadows", Field::DarkShadows, "Dark shadows", Section::Exposure, -2.0, 2.0, 0.0, Fmt::Fixed(2)),
    plain("exposure.highlights", Field::Highlights, "Highlights", Section::Exposure, -1.0, 1.0, 0.0, Fmt::Fixed(2)),
    plain("exposure.highlightContrast", Field::HighlightContrast, "Highlight contrast", Section::Exposure, -1.0, 1.0, 0.0, Fmt::Fixed(2)),
    // Color
    plain("color.preSaturation", Field::PreSaturation, "Pre-saturation", Section::Color, 0.5, 2.0, 1.15, Fmt::Fixed(2)),
    plain("color.castStrength", Field::CastRemovalStrength, "Cast strength", Section::Color, 0.0, 2.0, 0.5, Fmt::Fixed(2)),
    plain("color.hueTrim", Field::HueTrim, "Hue Trim", Section::Color, -30.0, 30.0, 0.0, Fmt::Degrees),
    plain("color.printSaturation", Field::PrintSaturation, "Print Saturation", Section::Color, 0.0, 2.0, 1.0, Fmt::Fixed(2)),
    ControlSpec {
        id: "color.separationDamping",
        field: Field::SeparationDamping,
        label: "Separation Damping",
        section: Section::Color,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        fmt: Fmt::Fixed(2),
        display: Display::Plain,
        disabled: Disabled::WhenEquals(Field::PrintSaturation, 1.0),
        track: Track::Plain,
        band: BandTag::None,
    },
    plain("color.vibrance", Field::Vibrance, "Vibrance", Section::Color, 0.0, 2.0, 1.0, Fmt::Fixed(2)),
    plain("color.saturation", Field::Saturation, "Saturation", Section::Color, 0.0, 2.0, 1.0, Fmt::Fixed(2)),
    plain("color.skinProtection", Field::SkinProtection, "Skin Protection", Section::Color, 0.0, 1.0, 0.5, Fmt::Fixed(2)),
    // Color Mixer (per band)
    MIXER_RED[0],
    MIXER_RED[1],
    MIXER_YELLOW[0],
    MIXER_YELLOW[1],
    MIXER_GREEN[0],
    MIXER_GREEN[1],
    MIXER_BLUE[0],
    MIXER_BLUE[1],
    // Color Grading
    ControlSpec {
        id: "grading.temp",
        field: Field::Temp,
        label: "Temp",
        section: Section::ColorGrading,
        min: -1.0,
        max: 1.0,
        default: 0.0,
        fmt: Fmt::Fixed(2),
        display: Display::Plain,
        disabled: Disabled::Never,
        track: Track::Gradient3([[0.25, 0.35, 0.85], W04, [0.75, 0.7, 0.15]]),
        band: BandTag::None,
    },
    ControlSpec {
        id: "grading.tint",
        field: Field::Tint,
        label: "Tint",
        section: Section::ColorGrading,
        min: -1.0,
        max: 1.0,
        default: 0.0,
        fmt: Fmt::Fixed(2),
        display: Display::Plain,
        disabled: Disabled::Never,
        track: Track::Gradient3([[0.2, 0.75, 0.25], W04, [0.7, 0.2, 0.75]]),
        band: BandTag::None,
    },
    GRADING_SHADOWS[0],
    GRADING_SHADOWS[1],
    GRADING_SHADOWS[2],
    GRADING_MIDS[0],
    GRADING_MIDS[1],
    GRADING_MIDS[2],
    GRADING_HIGHS[0],
    GRADING_HIGHS[1],
    GRADING_HIGHS[2],
    // Tone
    plain("tone.toe", Field::Toe, "Toe", Section::Tone, -1.0, 1.0, 0.0, Fmt::Fixed(2)),
    plain("tone.shoulder", Field::Shoulder, "Shoulder", Section::Tone, -1.0, 1.0, 0.0, Fmt::Fixed(2)),
    plain("tone.whitePoint", Field::WhitePointOffset, "White point", Section::Tone, -0.3, 0.3, 0.0, Fmt::Fixed(3)),
    plain("tone.blackPoint", Field::BlackPointOffset, "Black point", Section::Tone, -0.3, 0.3, 0.0, Fmt::Fixed(3)),
];

/// Every toggle row.
pub static TOGGLES: &[ToggleSpec] = &[
    ToggleSpec {
        id: "print.autoExposure",
        toggle: Toggle::AutoExposure,
        label: "Auto exposure",
        section: Section::Print,
        default: true,
    },
    ToggleSpec {
        id: "print.autoContrast",
        toggle: Toggle::AutoNormalizeContrast,
        label: "Auto contrast",
        section: Section::Print,
        default: true,
    },
    ToggleSpec {
        id: "tone.trueBlack",
        toggle: Toggle::TrueBlack,
        label: "True black",
        section: Section::Tone,
        default: true,
    },
];

/// Settings keys the sidebar deliberately does NOT expose (controls-spec §3: WB trims and the
/// knee widths/paper white have no UI anywhere; levels live in the interactive histogram;
/// geometry lives in the canvas tools / Crop & Rotation section).
pub const KEYS_WITHOUT_SLIDER: &[&str] = &[
    "wbCyan",
    "wbMagenta",
    "wbYellow",
    "toeWidth",
    "shoulderWidth",
    "paperDmin",
    "levelsRed",
    "levelsGreen",
    "levelsBlue",
    "rotation",
    "flipHorizontal",
    "fineRotation",
    "analysisRect",
    "analysisRectFineRotation",
    "cropRect",
];

/// Look a row up by id.
pub fn control(id: &str) -> Option<&'static ControlSpec> {
    CONTROLS.iter().find(|c| c.id == id)
}

/// The rows of `section` visible under the given band selections, in order.
pub fn rows_for(section: Section, mixer: MixerBand, grading: GradingBand) -> Vec<&'static ControlSpec> {
    CONTROLS
        .iter()
        .filter(|c| c.section == section)
        .filter(|c| match c.band {
            BandTag::None => true,
            BandTag::Mixer(b) => b == mixer,
            BandTag::Grading(b) => b == grading,
        })
        .collect()
}
