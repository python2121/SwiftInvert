//! Ports of `LabColorTests.swift` (LabColorTests, GamutAwareChromaTests, SkinProtectionTests),
//! `AuditStatsAndLevelsTests.swift`'s AuditGamutBoostTests and `HueTrimTests.swift`, at the Swift
//! tolerances. The `lab_color` fixtures were dumped from NegPy's `features/lab/logic.py`
//! (vibrance frozen at the `0369b10` dump — upstream deleted theirs).

use negkit::fixtures;
use negkit::lab::{self, ColorPop};
use negkit::{ExposureSettings, RgbImage, V3};
use std::f64::consts::PI;

fn chroma(lab: V3) -> f64 {
    (lab[1] * lab[1] + lab[2] * lab[2]).sqrt()
}

fn hue_deg(lab: V3) -> f64 {
    lab[2].atan2(lab[1]).to_degrees()
}

fn chroma_hue(rgb: V3) -> (f64, f64) {
    let l = lab::rgb_to_lab(rgb);
    (chroma(l), hue_deg(l))
}

fn length(a: V3, b: V3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn wrap_deg(d: f64) -> f64 {
    (d + 540.0) % 360.0 - 180.0
}

/// Flat a*/b* scale then per-channel clamp — the retired behaviour the gamut-aware tests
/// compare against.
fn flat_scale(rgb: V3, k: f64) -> V3 {
    let mut l = lab::rgb_to_lab(rgb);
    l[1] *= k;
    l[2] *= k;
    let out = lab::lab_to_rgb(l);
    [
        out[0].clamp(0.0, 1.0),
        out[1].clamp(0.0, 1.0),
        out[2].clamp(0.0, 1.0),
    ]
}

// ── LabColorTests ───────────────────────────────────────────────────────────────────────────

#[test]
fn fixture_parity() {
    let manifest = fixtures::read_json("lab_color/manifest.json").unwrap();
    let shape = manifest["input"]["shape"].as_array().unwrap();
    let (h, w) = (
        shape[0].as_u64().unwrap() as usize,
        shape[1].as_u64().unwrap() as usize,
    );
    let input = fixtures::read_rgb("lab_color/input.bin", w, h).unwrap();
    let cases = manifest["cases"].as_object().unwrap();
    assert_eq!(cases.len(), 5, "manifest lists five cases");
    for (name, case) in cases {
        let vibrance = case["vibrance"].as_f64().unwrap();
        let saturation = case["saturation"].as_f64().unwrap();
        let expected = fixtures::read_f32(&format!("lab_color/{name}.bin")).unwrap();
        let got = lab::apply(&input, vibrance, saturation, 0.0);
        assert_eq!(got.data.len(), expected.len());
        let (max, _) = fixtures::max_mean_abs_diff(&got.data, &expected);
        assert!(max < 1e-4, "{name}: max diff {max}");
    }
}

#[test]
fn neutral_stay_neutral() {
    // Grays have zero chroma — no op may tint them.
    let img = RgbImage::new_with(8, 1, |d| {
        for (i, px) in d.chunks_exact_mut(3).enumerate() {
            px.fill(i as f32 / 7.0);
        }
    })
    .unwrap();
    let out = lab::apply(&img, 2.0, 1.8, 0.0);
    for (i, (a, b)) in out.data.iter().zip(&img.data).enumerate() {
        assert!((a - b).abs() < 1e-4, "neutral shifted at {i}");
    }
}

#[test]
fn identity_at_one() {
    let img = RgbImage::from_vec(2, 1, vec![0.2, 0.5, 0.7, 0.9, 0.1, 0.3]).unwrap();
    let out = lab::apply(&img, 1.0, 1.0, 0.0);
    assert_eq!(out.data, img.data);
}

#[test]
fn vibrance_protects_saturated_colors() {
    // A muted colour gains more chroma than an already-saturated one.
    let chroma_gain = |rgb: V3| {
        let before = lab::rgb_to_lab(rgb);
        let after = lab::rgb_to_lab(lab::apply_vibrance_saturation(rgb, 1.6, 1.0, 0.0));
        let c0 = chroma(before);
        if c0 > 0.0 { chroma(after) / c0 } else { 1.0 }
    };
    let muted_gain = chroma_gain([0.45, 0.40, 0.38]); // near-neutral skin-ish tone
    let saturated_gain = chroma_gain([0.8, 0.1, 0.1]); // strong red
    assert!(
        muted_gain > saturated_gain + 0.05,
        "muted {muted_gain} vs saturated {saturated_gain}"
    );
    assert!(muted_gain > 1.1);
}

// ── Colour mixer (chroma-gated R/Y/G/B bands) ───────────────────────────────────────────────

type Band = (f64, f64);

fn mixer(rgb: V3, red: Band, yellow: Band, green: Band, blue: Band) -> V3 {
    lab::apply_color_mixer(
        rgb,
        [red.0, yellow.0, green.0, blue.0],
        [red.1, yellow.1, green.1, blue.1],
    )
}

const OFF: Band = (0.0, 1.0);

#[test]
fn mixer_defaults_are_identity() {
    let px = [0.62, 0.21, 0.17];
    assert_eq!(mixer(px, OFF, OFF, OFF, OFF), px);
}

#[test]
fn mixer_leaves_neutrals_untouched() {
    // The whole point: grays (and faint casts) never move, any strength.
    for gray in [0.02, 0.18, 0.5, 0.95] {
        let px = [gray; 3];
        let out = mixer(px, (1.0, 0.0), (-1.0, 0.0), (1.0, 2.0), (-1.0, 2.0));
        assert!(length(out, px) < 1e-6);
    }
    // Near-neutral warm cast: chroma below the gate floor → protected.
    let cast = [0.52, 0.50, 0.49];
    assert!(chroma_hue(cast).0 < lab::BAND_CHROMA_GATE_LOW);
    let out = mixer(cast, (1.0, 0.0), OFF, OFF, OFF);
    assert!(length(out, cast) < 1e-6);
}

#[test]
fn mixer_shifts_saturated_reds() {
    let red = [0.55, 0.10, 0.08];
    let (chroma_in, hue_in) = chroma_hue(red);
    assert!(chroma_in > lab::BAND_CHROMA_GATE_HIGH);

    // + hue rotates toward orange (hue angle increases).
    let warmed = chroma_hue(mixer(red, (1.0, 1.0), OFF, OFF, OFF));
    assert!(warmed.1 > hue_in + 3.0);

    // Saturation < 1 pulls chroma down without killing it.
    let tamed = chroma_hue(mixer(red, (0.0, 0.5), OFF, OFF, OFF));
    assert!(tamed.0 < chroma_in * 0.85);
    assert!(tamed.0 > chroma_in * 0.3);
}

#[test]
fn mixer_bands_target_their_own_hues() {
    let red = [0.55, 0.10, 0.08];
    let green = [0.10, 0.55, 0.12];
    // Hue 264° in the Adobe-D65 Lab (in the blue band 235±65). The old primary-ish
    // (0.08, 0.12, 0.60) lands at 293° — the feather's edge — consistent with the bands being
    // tuned on real content, whose blues (sky ~240-265° here) stay in-band across the space
    // change.
    let blue = [0.06, 0.22, 0.55];

    // The blue band ignores saturated red/green (far outside its window)…
    for px in [red, green] {
        assert!(length(mixer(px, OFF, OFF, OFF, (1.0, 0.2)), px) < 1e-6);
    }
    // …but moves saturated blue.
    let shifted = chroma_hue(mixer(blue, OFF, OFF, OFF, (0.0, 0.5)));
    assert!(shifted.0 < chroma_hue(blue).0 * 0.9);

    // The green band moves green but not red.
    assert!(length(mixer(red, OFF, OFF, (1.0, 0.2), OFF), red) < 1e-6);
    // Wrapped delta: a full + shift can carry the hue across ±180°.
    let green_shift = chroma_hue(mixer(green, OFF, OFF, (1.0, 1.0), OFF));
    let delta = wrap_deg(green_shift.1 - chroma_hue(green).1);
    assert!(delta > 3.0);
}

/// Overlapping feathers compose jointly from the ORIGINAL hue, so the result is independent
/// of band order: driving the overlap through one combined call equals the product/sum of the
/// per-band contributions, and the red+yellow pair gives the same answer however it's
/// expressed. (Property the Swift comment promises; no separate Swift test.)
#[test]
fn mixer_overlapping_bands_compose_order_independently() {
    // Red-orange (Lab hue ≈56°, chroma ≈40 in the Adobe-D65 Lab) sits in both the red
    // (25±45) and yellow (90±45) feathers, well above the gate.
    let orange = [0.60, 0.30, 0.15];
    let (_, h) = chroma_hue(orange);
    assert!(
        h > 25.0 && h < 90.0,
        "orange hue {h} must sit in the overlap"
    );
    let both = mixer(orange, (0.5, 1.3), (-0.4, 0.7), OFF, OFF);
    // Weights from the original hue, composed jointly:
    let l = lab::rgb_to_lab(orange);
    let gate = lab::smoothstep(
        lab::BAND_CHROMA_GATE_LOW,
        lab::BAND_CHROMA_GATE_HIGH,
        chroma(l),
    );
    let w = |i: usize| {
        let dh = wrap_deg(h - lab::BAND_CENTERS_DEG[i]);
        let t = (dh.abs() / lab::BAND_HALF_WIDTHS_DEG[i]).min(1.0);
        gate * 0.5 * (1.0 + (PI * t).cos())
    };
    assert!(w(0) > 0.05 && w(1) > 0.05, "both feathers engaged");
    let delta = 0.5 * 30.0 * w(0) + (-0.4) * 30.0 * w(1);
    let scale = (1.0 + 0.3 * w(0)) * (1.0 + (-0.3) * w(1));
    let mut expect = l;
    let c = chroma(l) * scale;
    expect[1] = c * (h + delta).to_radians().cos();
    expect[2] = c * (h + delta).to_radians().sin();
    let e = lab::lab_to_rgb(expect);
    let expect_rgb = [
        e[0].clamp(0.0, 1.0),
        e[1].clamp(0.0, 1.0),
        e[2].clamp(0.0, 1.0),
    ];
    assert!(length(both, expect_rgb) < 1e-9);
}

// ── GamutAwareChromaTests ───────────────────────────────────────────────────────────────────

fn hue_delta(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    d -= 360.0 * (d / 360.0).round();
    d.abs()
}

/// A vivid pixel that clips under a flat 1.6× push: the old flat+clamp path shifts its hue;
/// the gamut-aware path must hold it far tighter.
#[test]
fn hue_preserved_where_flat_scale_clipped() {
    let vivid = [0.85, 0.1, 0.12]; // strong red, near the gamut wall
    let h0 = hue_deg(lab::rgb_to_lab(vivid));

    let flat = flat_scale(vivid, 1.6);
    let flat_err = hue_delta(hue_deg(lab::rgb_to_lab(flat)), h0);

    let aware = lab::apply_vibrance_saturation(vivid, 1.0, 1.6, 0.0);
    let aware_err = hue_delta(hue_deg(lab::rgb_to_lab(aware)), h0);

    assert!(
        flat_err > 1.0,
        "premise: the flat path visibly shifts hue ({flat_err}°)"
    );
    assert!(
        aware_err < flat_err * 0.5,
        "gamut-aware must at least halve it ({flat_err}° → {aware_err}°)"
    );
    assert!(
        aware_err < 2.0,
        "and hold it small in absolute terms ({aware_err}°)"
    );
}

/// Comfortably in-gamut pixels get the exact flat scale — ANY hue: the skin term was removed
/// from the boost (bfcd90a); Skin Protection is a separate operator now.
#[test]
fn in_gamut_pixels_match_flat_scale() {
    let muted = [0.25, 0.52, 0.60];
    let flat = flat_scale(muted, 1.2);
    let aware = lab::apply_vibrance_saturation(muted, 1.0, 1.2, 0.0);
    assert!(length(aware, flat) < 1e-9);
}

/// Desaturation is untouched by the machinery.
#[test]
fn desaturation_stays_flat() {
    let vivid = [0.85, 0.1, 0.12];
    let flat = flat_scale(vivid, 0.5);
    let aware = lab::apply_vibrance_saturation(vivid, 1.0, 0.5, 0.0);
    assert!(length(aware, flat) < 1e-9);
}

// ── AuditGamutBoostTests ────────────────────────────────────────────────────────────────────

#[test]
fn out_of_gamut_input_yields_bounded_finite_boost() {
    // L 50, a 200, b −150 sits far outside the Adobe RGB cube.
    let l: V3 = [50.0, 200.0, -150.0];
    assert!(!lab::in_gamut_lab(l[0], l[1], l[2]));

    for saturation in [1.2, 1.5, 3.0] {
        let boost = lab::gamut_aware_boost(l, saturation);
        assert!(boost.is_finite());
        assert!(boost >= 1.0);
        assert!(boost <= saturation);
    }
    // An already-out pixel has no headroom: the bisection collapses to sMax = 1 + tolerance,
    // so the boost stays within a hair of 1.0 (the knee can't exceed sMax).
    let boost = lab::gamut_aware_boost(l, 2.0);
    assert!(boost <= 1.0 + lab::GAMUT_TOLERANCE + 1e-12);
}

/// Sanity contrast: an in-gamut muted pixel takes the full requested push.
#[test]
fn in_gamut_pixel_takes_full_push() {
    let l = lab::rgb_to_lab([0.4, 0.38, 0.36]); // near-neutral
    assert!(lab::in_gamut_lab(l[0], l[1] * 1.3, l[2] * 1.3));
    assert_eq!(lab::gamut_aware_boost(l, 1.3), 1.3);
}

// ── SkinProtectionTests ─────────────────────────────────────────────────────────────────────

/// Build a Lab triplet from (L, C, hue°).
fn lch(l: f64, c: f64, hue_deg: f64) -> V3 {
    let h = hue_deg.to_radians();
    [l, c * h.cos(), c * h.sin()]
}

fn weight(l: f64, c: f64, hue: f64) -> f64 {
    let v = lch(l, c, hue);
    lab::skin_weight(v[0], v[1], v[2])
}

#[test]
fn mask_matches_the_measured_locus() {
    // Dead-centre skin: L*65, C*27, hue 52° → near-full weight.
    assert!(weight(65.0, 27.0, 52.0) > 0.9);
    // Pure saturated red: right hue-ish but C*≈104 → drops out entirely.
    assert_eq!(weight(55.0, 104.0, 40.0), 0.0);
    // Sunset-chroma warm (C*57): mostly out via the chroma window.
    assert!(weight(60.0, 57.0, 50.0) < 0.15);
    // Wrong hue at skin chroma (teal): out via the hue Gaussian.
    assert!(weight(65.0, 27.0, -125.0) < 1e-6);
    // Too dark / too bright: the lightness rolloff is SMOOTH (smoothstep 0…15 and 95…100),
    // so assert attenuation, not zero.
    assert!(weight(2.0, 27.0, 52.0) < 0.05);
    assert!(weight(5.0, 27.0, 52.0) < 0.3);
    assert!(weight(99.0, 27.0, 52.0) < 0.2);
    // Near-neutral: chroma gate.
    assert_eq!(weight(65.0, 1.0, 52.0), 0.0);
}

#[test]
fn rein_is_reduce_only_and_preserves_hue_and_l() {
    for (l, c, hue) in [(65.0, 45.0, 52.0), (55.0, 38.0, 60.0), (70.0, 50.0, 45.0)] {
        let out = lab::skin_chroma_rein(lch(l, c, hue), 1.0);
        let c_out = chroma(out);
        assert!(c_out <= c + 1e-9, "chroma may only fall ({c} → {c_out})");
        assert_eq!(out[0], l, "L* untouched");
        assert!((hue_deg(out) - hue).abs() < 1e-9, "hue untouched");
    }
}

#[test]
fn rein_bites_skin_but_not_warm_objects() {
    // Skin arriving over-chromatic (C*45 @ 52°): pulled down hard.
    let skin_c = 45.0;
    let reined = lab::skin_chroma_rein(lch(65.0, skin_c, 52.0), 1.0);
    let c_reined = chroma(reined);
    assert!(c_reined < skin_c * 0.85, "skin {skin_c} → {c_reined}");

    // A sunset-chroma pixel (C*57, weight ~0.04): essentially untouched.
    let sun_c = 57.0;
    let sun = lab::skin_chroma_rein(lch(60.0, sun_c, 50.0), 1.0);
    let c_sun = chroma(sun);
    assert!(
        c_sun > sun_c * 0.93,
        "sunset must keep its colour ({sun_c} → {c_sun})"
    );
}

#[test]
fn identity_below_knee_and_at_zero_strength() {
    // Typical skin (C*27) sits just over the knee at default 0.5 (ceiling 44, start 26.4);
    // use C*24.
    let l = lch(65.0, 24.0, 52.0);
    assert_eq!(lab::skin_chroma_rein(l, 0.5), l);
    let hot = [65.0, 45.0, 20.0];
    assert_eq!(lab::skin_chroma_rein(hot, 0.0), hot);
}

/// Keyless settings adopt the on-by-default (upstream's choice). The JSON round trip lives in
/// agent C's sidecar tests.
#[test]
fn skin_protection_default_is_half() {
    assert_eq!(ExposureSettings::default().skin_protection, 0.5);
}

// ── HueTrimTests ────────────────────────────────────────────────────────────────────────────

/// Chromatic samples spanning the chroma range upstream measured over (CIELAB chroma 4 to
/// 60+), plus deliberate near-neutrals.
const SAMPLES: [V3; 7] = [
    [0.62, 0.31, 0.24], // skin-ish
    [0.18, 0.42, 0.22], // foliage
    [0.22, 0.35, 0.71], // sky
    [0.81, 0.66, 0.20], // warm highlight
    [0.50, 0.50, 0.50], // neutral
    [0.51, 0.50, 0.49], // near-neutral
    [0.05, 0.05, 0.05], // deep shadow
];

#[test]
fn zero_is_exactly_identity() {
    for rgb in SAMPLES {
        assert_eq!(lab::apply_hue_trim(rgb, 0.0), rgb, "{rgb:?}");
    }
}

/// The default must be identity or every NegPy fixture would move. (The derive half —
/// `deriveRenderParams(ExposureSettings(), analysis).hueTrim == 0` and the degrees→radians
/// clamp — belongs with `derive_render_params` in agent C's tests.)
#[test]
fn default_setting_is_off() {
    assert_eq!(ExposureSettings::default().hue_trim, 0.0);
    assert!(!ColorPop::default().is_active());
}

/// Neutrals are the fixed point — the property that lets Hue Trim sit in front of the cast
/// removal without fighting it.
///
/// Bounded in ABSOLUTE terms, not as "chroma is exactly preserved": the working matrix's
/// rowsums differ from the D65 reference white in the 7th digit, so `rgb_to_lab` already
/// reports a few times 1e-6 of chroma for a perfect grey — and the rgb→Lab→rgb round trip
/// perturbs that by about as much again. Swept 994 greys × 121 angles in Swift: worst residual
/// chroma 1.05e-5, worst channel spread 1.3e-7.
#[test]
fn neutrals_never_move() {
    for grey in [0.02, 0.1, 0.25, 0.5, 0.75, 0.98] {
        let rgb = [grey; 3];
        assert!(
            chroma(lab::rgb_to_lab(rgb)) < 1e-5,
            "precondition: grey {grey} starts near-neutral"
        );
        for deg in [-30.0, -7.0, 3.0, 12.0, 30.0] {
            let out = lab::apply_hue_trim(rgb, deg * PI / 180.0);
            let c = chroma(lab::rgb_to_lab(out));
            assert!(c < 5e-5, "grey {grey} at {deg}°: chroma became {c}");
            assert!((out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6);
        }
    }
}

#[test]
fn lightness_and_chroma_are_untouched() {
    for rgb in SAMPLES {
        let before = lab::rgb_to_lab(rgb);
        // Skip samples the RGB cube clips on rotation — the clamp is real, but it is not what
        // this invariant is about.
        if !(chroma(before) > 1.0 && chroma(before) < 40.0) {
            continue;
        }
        for deg in [-20.0, -5.0, 9.0, 25.0] {
            let after = lab::rgb_to_lab(lab::apply_hue_trim(rgb, deg * PI / 180.0));
            assert!(
                (after[0] - before[0]).abs() < 0.02,
                "L* moved at {deg}° for {rgb:?}"
            );
            assert!(
                (chroma(after) - chroma(before)).abs() < 0.05,
                "chroma moved at {deg}° for {rgb:?}"
            );
        }
    }
}

/// The angle you dial is the angle you get.
#[test]
fn the_measured_rotation_is_the_angle_asked() {
    for rgb in SAMPLES {
        let before = lab::rgb_to_lab(rgb);
        if !(chroma(before) > 2.0 && chroma(before) < 40.0) {
            continue;
        }
        for deg in [-18.0, -6.0, 4.0, 21.0] {
            let after = lab::rgb_to_lab(lab::apply_hue_trim(rgb, deg * PI / 180.0));
            let delta = wrap_deg(hue_deg(after) - hue_deg(before));
            assert!(
                (delta - deg).abs() < 0.1,
                "asked {deg}°, measured {delta}° for {rgb:?}"
            );
        }
    }
}

/// Opposite angles cancel — the operation is invertible where the cube doesn't clip.
#[test]
fn opposite_angles_cancel() {
    for rgb in SAMPLES {
        let c = chroma(lab::rgb_to_lab(rgb));
        if !(c > 1.0 && c < 40.0) {
            continue;
        }
        for deg in [-25.0, -8.0, 11.0, 28.0] {
            let there = lab::apply_hue_trim(rgb, deg * PI / 180.0);
            let back = lab::apply_hue_trim(there, -deg * PI / 180.0);
            let err = (0..3).map(|i| (back[i] - rgb[i]).abs()).fold(0.0, f64::max);
            assert!(
                err < 2e-3,
                "round trip at ±{deg}° for {rgb:?} drifted {err}"
            );
        }
    }
}

/// A rotation is NOT a cast: its effect must GROW with chroma, where a fixed a*/b* offset
/// would be constant. This is the distinction upstream's measurement rests on, so pin the
/// direction.
#[test]
fn effect_grows_with_chroma_unlike_a_cast() {
    // Same hue, increasing chroma, well inside the cube.
    let low = [0.52, 0.50, 0.48];
    let high = [0.72, 0.45, 0.25];
    let deg = 15.0;
    let shift = |rgb: V3| {
        let a = lab::rgb_to_lab(rgb);
        let b = lab::rgb_to_lab(lab::apply_hue_trim(rgb, deg * PI / 180.0));
        ((b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    };
    assert!(chroma(lab::rgb_to_lab(high)) > chroma(lab::rgb_to_lab(low)));
    assert!(
        shift(high) > shift(low) * 2.0,
        "a rotation must move saturated colour much further than muted"
    );
}

// ── The combined stage ──────────────────────────────────────────────────────────────────────

/// `ColorPop::apply_pixel` is the ReferenceCurve order: trim → mixer → vibrance/saturation/skin.
#[test]
fn color_pop_composes_in_kernel_order() {
    let p = ColorPop {
        hue_trim: 8.0_f64.to_radians(),
        band_hues: [0.3, 0.0, -0.2, 0.0],
        band_saturations: [1.2, 1.0, 1.0, 0.8],
        vibrance: 1.3,
        saturation: 1.15,
        skin_protection: 0.5,
    };
    assert!(p.is_active());
    for rgb in SAMPLES {
        let step = lab::apply_hue_trim(rgb, p.hue_trim);
        let step = lab::apply_color_mixer(step, p.band_hues, p.band_saturations);
        let step =
            lab::apply_vibrance_saturation(step, p.vibrance, p.saturation, p.skin_protection);
        assert_eq!(p.apply_pixel(rgb), step);
    }
    // Whole-buffer path equals the per-pixel path, narrowed to f32 once.
    let img = RgbImage::new_with(SAMPLES.len(), 1, |d| {
        for (px, s) in d.chunks_exact_mut(3).zip(SAMPLES) {
            px.copy_from_slice(&[s[0] as f32, s[1] as f32, s[2] as f32]);
        }
    })
    .unwrap();
    let mut out = img.clone();
    p.apply_in_place(&mut out);
    for (px, s) in out.data.chunks_exact(3).zip(SAMPLES) {
        let want = p.apply_pixel([
            f64::from(s[0] as f32),
            f64::from(s[1] as f32),
            f64::from(s[2] as f32),
        ]);
        for i in 0..3 {
            assert_eq!(px[i], want[i] as f32);
        }
    }
    // Identity stage: untouched bytes and no work.
    let mut same = img.clone();
    ColorPop::default().apply_in_place(&mut same);
    assert_eq!(same.data, img.data);
}
