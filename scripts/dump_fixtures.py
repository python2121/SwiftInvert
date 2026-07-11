"""Dump parity fixtures from the NegPy reference implementation for the SwiftInvert port.

Run from the NegPy repo (needs its environment):

    cd /Users/andrewnowicki/Documents/code/NegPy
    uv run python /Users/andrewnowicki/Documents/code/SwiftInvert/scripts/dump_fixtures.py

Outputs to SwiftInvert/Tests/Fixtures/:
  - closed_form.json          scalar oracles for CurveLogic / percentile / OETF
  - ramp257.json              257-sample ramp through the default print curve
  - synthetic64/              64x64 stage-boundary fixtures for 3 exposure configs
      input.bin               (64,64,3) float32 LE  (numpy RNG image, dumped once)
      prefiltered.bin         log-grid after analysis crop (shared across configs)
      <config>/analysis.json  bounds, meters, derived curve params
      <config>/normalized.bin post-normalization image
      <config>/curve_linear.bin  post-print-curve linear reflectance
      <config>/output.bin     final working-OETF-encoded image
  - synthetic_grid/           hash-generated 1600x1066 image (regenerated in Swift,
                              exercises the 2x2 block-median path; only the small
                              grid + analysis results are dumped)
"""

from __future__ import annotations

import json
import sys
from dataclasses import replace
from pathlib import Path

import numpy as np

from negpy.domain.interfaces import PipelineContext
from negpy.features.exposure.logic import (
    _inv_softplus_np,
    _reference_linear_value,
    apply_characteristic_curve,
    cmy_to_density,
    compute_pivot,
    density_to_cmy,
    effective_cast_strength,
    effective_grade_range,
    filtration_offsets,
    grade_coupled_shape,
    grade_to_slope,
    normalized_neutral_axis,
    normalized_shadow_refs,
    per_channel_curve_params,
    slope_to_grade,
)
from negpy.features.exposure.models import EXPOSURE_CONSTANTS, ExposureConfig
from negpy.features.exposure.normalization import (
    analyze_log_exposure_bounds_from_log,
    measure_anchor_from_log,
    measure_neutral_axis_from_log,
    measure_shadow_refs_from_log,
    measure_textural_range_from_log,
    prefilter_log_grid,
    resolve_analysis_region,
)
from negpy.features.exposure.papers import effective_paper_profile
from negpy.features.exposure.processor import NormalizationProcessor, PhotometricProcessor
from negpy.features.process.models import ProcessConfig, ProcessMode
from negpy.kernel.image.logic import working_oetf_decode, working_oetf_encode

OUT = Path("/Users/andrewnowicki/Documents/code/SwiftInvert/Tests/Fixtures")


def dump_bin(path: Path, arr: np.ndarray) -> dict:
    arr = np.ascontiguousarray(arr.astype(np.float32))
    path.parent.mkdir(parents=True, exist_ok=True)
    arr.tofile(path)
    return {"path": str(path.relative_to(OUT)), "dtype": "float32", "shape": list(arr.shape)}


def jsonable(x):
    if x is None:
        return None
    if isinstance(x, (np.floating, np.integer)):
        return float(x)
    if isinstance(x, (tuple, list, np.ndarray)):
        return [jsonable(v) for v in x]
    return x


# ---------------------------------------------------------------- synthetic64

def synthetic_image(seed: int = 42) -> np.ndarray:
    """Byte-identical to tests/test_scene_linear_relocation.py::_synthetic_image."""
    rng = np.random.default_rng(seed)
    img = np.zeros((64, 64, 3), dtype=np.float32)
    for y in range(64):
        for x in range(64):
            img[y, x] = 0.1 + 0.8 * ((x + y) / 126.0)
    img[0:16, 0:16] = [0.9, 0.1, 0.1]
    img[0:16, 48:64] = [0.1, 0.9, 0.1]
    img[48:64, 0:16] = [0.1, 0.1, 0.9]
    img[48:64, 48:64] = [0.9, 0.9, 0.1]
    img += rng.normal(0, 0.005, img.shape).astype(np.float32)
    return np.clip(img, 0.0, 1.0).astype(np.float32)


# ------------------------------------------------------------- synthetic_grid

def hash32(i: np.ndarray) -> np.ndarray:
    """splitmix32-style avalanche on uint32 — trivially portable to Swift UInt32."""
    x = i.astype(np.uint32)
    x = (x + np.uint32(0x9E3779B9)).astype(np.uint32)
    x ^= x >> np.uint32(16)
    x = (x * np.uint32(0x21F0AAAD)).astype(np.uint32)
    x ^= x >> np.uint32(15)
    x = (x * np.uint32(0x735A2D97)).astype(np.uint32)
    x ^= x >> np.uint32(15)
    return x


def synthetic_grid_image(w: int = 1600, h: int = 1066) -> np.ndarray:
    """Deterministic negative-like image built from integer hashing + float32 ops
    only, so Swift can regenerate it bit-exactly (see NegativeKitTests)."""
    yy, xx = np.meshgrid(np.arange(h, dtype=np.uint32), np.arange(w, dtype=np.uint32), indexing="ij")
    base = ((xx + yy).astype(np.float32) / np.float32(w + h - 2))
    base = np.float32(0.08) + np.float32(0.75) * base
    img = np.empty((h, w, 3), dtype=np.float32)
    scales = (np.float32(0.85), np.float32(0.45), np.float32(0.25))  # orange-mask-like
    idx = (yy.astype(np.uint32) * np.uint32(w) + xx.astype(np.uint32)) * np.uint32(3)
    for c in range(3):
        n = hash32(idx + np.uint32(c)).astype(np.float32) / np.float32(4294967296.0)
        n = (n - np.float32(0.5)) * np.float32(0.06)
        img[:, :, c] = (base + n) * scales[c]
    return np.clip(img, np.float32(1e-4), np.float32(1.0))


# ---------------------------------------------------------------- pipeline run

def run_pipeline(img: np.ndarray, exposure: ExposureConfig, out_dir: Path, dump_images: bool) -> dict:
    """Run NormalizationProcessor + PhotometricProcessor + working_oetf_encode,
    capturing every stage boundary. Mirrors DarkroomEngine's exposure path without
    geometry/retouch/lab/toning/finish (LabConfig.sharpen would contaminate goldens)."""
    process = ProcessConfig(white_point_offset=0.0, black_point_offset=0.0)
    ctx = PipelineContext(original_size=(img.shape[1], img.shape[0]), scale_factor=1.0,
                          process_mode=ProcessMode.C41)

    norm = NormalizationProcessor(process).process(img, ctx)
    photo = PhotometricProcessor(exposure).process(norm, ctx)
    encoded = working_oetf_encode(photo)

    # Re-derive the curve params exactly as PhotometricProcessor.process does,
    # so the Swift UniformsBuilder can be checked against them.
    paper = effective_paper_profile(exposure.paper_profile, ProcessMode.C41)
    d_min = paper.d_min if exposure.paper_dmin else 0.0
    final_bounds = ctx.metrics["final_bounds"]
    neutral_axis_refs = ctx.metrics.get("neutral_axis_refs")
    confidence = neutral_axis_refs[3] if neutral_axis_refs is not None else None
    # NegPy >=0.36: confidence scaling is always applied (auto toggle removed).
    strength = effective_cast_strength(exposure.cast_removal_strength, confidence)
    slopes, pivots, curvatures = per_channel_curve_params(
        exposure.grade,
        exposure.density,
        exposure.auto_normalize_contrast,
        strength,
        ctx.metrics.get("norm_density_range"),
        normalized_shadow_refs(final_bounds, ctx.metrics.get("shadow_log_refs")),
        ctx.metrics.get("textural_range"),
        d_min=d_min,
        anchor=ctx.metrics.get("metered_anchor") if exposure.auto_exposure else None,
        paper=paper,
        neutral_axis_norm=normalized_neutral_axis(final_bounds, neutral_axis_refs),
    )
    cmy_offsets = filtration_offsets((exposure.wb_cyan, exposure.wb_magenta, exposure.wb_yellow), final_bounds)
    toe_eff, shoulder_eff = grade_coupled_shape(slopes[1], exposure.toe, exposure.shoulder)

    base_bounds = ctx.metrics["log_bounds_base"]
    analysis = {
        "exposure_config": {
            "density": exposure.density, "grade": exposure.grade,
            "wb_cyan": exposure.wb_cyan, "wb_magenta": exposure.wb_magenta, "wb_yellow": exposure.wb_yellow,
            "auto_exposure": exposure.auto_exposure, "auto_normalize_contrast": exposure.auto_normalize_contrast,
            "cast_removal_strength": exposure.cast_removal_strength,
            "toe": exposure.toe, "toe_width": exposure.toe_width,
            "shoulder": exposure.shoulder, "shoulder_width": exposure.shoulder_width,
            "paper_dmin": exposure.paper_dmin,
        },
        "bounds": {
            "base_floors": jsonable(base_bounds.floors), "base_ceils": jsonable(base_bounds.ceils),
            "final_floors": jsonable(final_bounds.floors), "final_ceils": jsonable(final_bounds.ceils),
        },
        "meters": {
            "norm_density_range": jsonable(ctx.metrics.get("norm_density_range")),
            "metered_anchor": jsonable(ctx.metrics.get("metered_anchor")),
            "textural_range": jsonable(ctx.metrics.get("textural_range")),
            "shadow_log_refs": jsonable(ctx.metrics.get("shadow_log_refs")),
            "neutral_axis_refs": jsonable(neutral_axis_refs),
            "scan_clip_fractions": jsonable(ctx.metrics.get("scan_clip_fractions")),
        },
        "curve_params": {
            "slopes": jsonable(slopes), "pivots": jsonable(pivots), "curvatures": jsonable(curvatures),
            "cmy_offsets": jsonable(cmy_offsets),
            "toe_eff": jsonable(toe_eff), "shoulder_eff": jsonable(shoulder_eff),
            "cast_strength": jsonable(strength),
            "d_min": jsonable(d_min),
            "v_star": jsonable(_reference_linear_value(d_min, paper)),
        },
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    if dump_images:
        analysis["arrays"] = {
            "normalized": dump_bin(out_dir / "normalized.bin", norm),
            "curve_linear": dump_bin(out_dir / "curve_linear.bin", photo),
            "output": dump_bin(out_dir / "output.bin", np.asarray(encoded)),
        }
    (out_dir / "analysis.json").write_text(json.dumps(analysis, indent=1))
    return analysis


def dump_synthetic64() -> None:
    img = synthetic_image()
    root = OUT / "synthetic64"
    root.mkdir(parents=True, exist_ok=True)
    manifest = {"input": dump_bin(root / "input.bin", img)}

    # Shared prefiltered grid (analysis_buffer default 0.05 -> 3px inset, 58x58; block
    # size b=1 so the grid IS the cropped log image).
    process = ProcessConfig()
    roi, buffer = resolve_analysis_region(img.shape, None, process.analysis_buffer, None)
    manifest["prefiltered"] = dump_bin(root / "prefiltered.bin", prefilter_log_grid(img, roi, buffer))
    manifest["analysis_buffer"] = process.analysis_buffer
    (root / "manifest.json").write_text(json.dumps(manifest, indent=1))

    configs = {
        "default": ExposureConfig(),
        "expo_dark": ExposureConfig(density=-1.0, grade=2.0),  # grade 2.0 -> legacy migration -> 110
        "expo_cmy": ExposureConfig(wb_cyan=0.3, wb_magenta=-0.2, wb_yellow=0.5),
    }
    for name, cfg in configs.items():
        run_pipeline(img, cfg, root / name, dump_images=True)
    print(f"synthetic64: 3 configs dumped (migrated expo_dark grade={configs['expo_dark'].grade})")


def dump_synthetic_grid() -> None:
    img = synthetic_grid_image()
    root = OUT / "synthetic_grid"
    root.mkdir(parents=True, exist_ok=True)
    process = ProcessConfig()
    roi, buffer = resolve_analysis_region(img.shape, None, process.analysis_buffer, None)
    grid = prefilter_log_grid(img, roi, buffer)
    manifest = {
        "generator": {"w": 1600, "h": 1066, "note": "regenerated in Swift via hash32/splitmix32 formula"},
        "prefiltered": dump_bin(root / "prefiltered.bin", grid),
        "analysis_buffer": process.analysis_buffer,
        # First few input pixels as a generator cross-check before the heavy compare.
        "input_probe": {f"{y},{x}": jsonable(img[y, x]) for (y, x) in [(0, 0), (0, 1), (1, 0), (500, 800), (1065, 1599)]},
    }
    (root / "manifest.json").write_text(json.dumps(manifest, indent=1))
    run_pipeline(img, ExposureConfig(), root / "default", dump_images=False)
    print("synthetic_grid: analysis dumped")


# ---------------------------------------------------------------- closed form

def dump_closed_form() -> None:
    from negpy.features.exposure.logic import _softplus

    data = {
        "constants": {k: jsonable(v) for k, v in EXPOSURE_CONSTANTS.items() if isinstance(v, (int, float, tuple))},
        "cmy_to_density": [
            {"val": v, "log_range": r, "out": jsonable(cmy_to_density(v, r))}
            for v, r in [(0.5, 1.0), (-0.5, 1.0), (0.3, 1.3), (1.0, 2.0), (0.0, 1.3)]
        ],
        "density_to_cmy": [
            {"density": d, "log_range": r, "out": jsonable(density_to_cmy(d, r))}
            for d, r in [(0.1, 1.0), (-0.06, 1.3), (0.2, 2.0)]
        ],
        "grade_to_slope": [
            {"grade": g, "range": r, "out": jsonable(grade_to_slope(g, r))}
            for g in (50.0, 90.0, 115.0, 150.0, 180.0) for r in (0.8, 1.3, 1.4, 2.5)
        ],
        "slope_to_grade": [
            {"slope": s, "range": r, "out": jsonable(slope_to_grade(s, r))}
            for s, r in [(2.0, 1.3), (3.2653, 1.3), (5.0, 1.4), (10.0, 1.3)]
        ],
        "compute_pivot": [
            {"slope": s, "density": d, "d_min": 0.06, "anchor": a,
             "out": jsonable(compute_pivot(s, d, 0.06, anchor=a))}
            for s, d, a in [(3.0, 1.0, None), (3.0, 1.0, 0.46), (3.0, 0.5, 0.52), (5.0, 1.5, 0.4), (2.9, 1.0, 0.34)]
        ],
        "reference_linear_value": [
            {"d_min": d, "out": jsonable(_reference_linear_value(d))} for d in (0.0, 0.06)
        ],
        "softplus": [{"x": x, "out": jsonable(_softplus(x))} for x in (-20.0, -5.0, -1.0, 0.0, 1.0, 5.0, 20.0)],
        "inv_softplus": [{"y": y, "out": jsonable(float(_inv_softplus_np(np.array([y], dtype=np.float64))[0]))}
                         for y in (0.01, 0.1, 0.5, 1.0, 5.0)],
        "effective_grade_range": [
            {"auto": a, "floor_ceil": fc, "textural": t, "out": jsonable(effective_grade_range(a, fc, t))}
            for a, fc, t in [(False, 1.3, 0.7), (True, 1.3, 0.7), (True, 1.3, None), (True, 2.0, 0.4), (True, 0.9, 0.8)]
        ],
        "grade_coupled_shape": [
            {"slope_g": s, "toe": t, "shoulder": sh, "out": jsonable(grade_coupled_shape(s, t, sh))}
            for s, t, sh in [(2.0, 0.0, 0.0), (3.2653, 0.0, 0.0), (10.0, 0.0, 0.0), (5.0, 0.5, -0.3)]
        ],
        "effective_cast_strength": [
            {"strength": s, "confidence": c, "out": jsonable(effective_cast_strength(s, c))}
            for s, c in [(0.5, 0.8), (0.5, None), (0.9, 0.2), (2.0, 0.7)]
        ],
        "working_oetf": [
            {"x": x, "enc": jsonable(float(working_oetf_encode(np.float32(x)))),
             "dec_of_enc": jsonable(float(working_oetf_decode(working_oetf_encode(np.float32(x)))))}
            for x in (0.0, 0.001, 1.0 / 512.0, 0.01, 0.18, 0.5, 1.0)
        ],
        # np.percentile linear-interpolation semantics the Swift port must match.
        "percentile": [
            {"data": d, "q": q, "out": jsonable(float(np.percentile(np.array(d, dtype=np.float32), q)))}
            for d, q in [
                ([1.0, 2.0, 3.0, 4.0], 50.0),
                ([1.0, 2.0, 3.0, 4.0, 5.0], 50.0),
                ([0.1, 0.9, 0.4, 0.7, 0.2, 0.35], 10.0),
                ([0.1, 0.9, 0.4, 0.7, 0.2, 0.35], 90.0),
                ([0.1, 0.9, 0.4, 0.7, 0.2, 0.35], 98.0),
                ([3.0, 1.0, 2.0], 0.01),
                ([3.0, 1.0, 2.0], 99.99),
                ([5.0], 50.0),
            ]
        ],
        "filtration_offsets_sample": {
            "wb_cmy": [0.3, -0.2, 0.5],
            "floors": [-2.1, -1.7, -1.3], "ceils": [-0.4, -0.35, -0.3],
            "out": jsonable(filtration_offsets(
                (0.3, -0.2, 0.5),
                type("B", (), {"floors": (-2.1, -1.7, -1.3), "ceils": (-0.4, -0.35, -0.3)})(),
            )),
        },
    }
    (OUT / "closed_form.json").write_text(json.dumps(data, indent=1))
    print("closed_form.json dumped")


def dump_ramp() -> None:
    """Mirror of tests/test_characteristic_curve.py::_curve at defaults + variants."""
    x = np.linspace(0.0, 1.0, 257).astype(np.float32)
    ramp = np.stack([x, x, x], axis=-1)[None, :, :]
    d_min = EXPOSURE_CONSTANTS["d_min"]
    cases = {}
    for name, (toe, shoulder, grade, density, lum_range) in {
        "default": (0.0, 0.0, 115.0, 1.0, 1.3),
        "toe1": (1.0, 0.0, 115.0, 1.0, 1.3),
        "shoulder1": (0.0, 1.0, 115.0, 1.0, 1.3),
        "hard_dark": (0.0, 0.0, 60.0, -0.5, 1.3),
        "soft_bright": (0.2, 0.2, 170.0, 1.8, 1.0),
    }.items():
        slope = grade_to_slope(grade, lum_range)
        pivot = compute_pivot(slope, density=density, d_min=d_min)
        out = apply_characteristic_curve(
            ramp, (pivot, slope), (pivot, slope), (pivot, slope),
            toe=toe, shoulder=shoulder, d_min=d_min,
        )
        enc = np.asarray(working_oetf_encode(np.asarray(out)))[0, :, 0]
        cases[name] = {
            "toe": toe, "shoulder": shoulder, "grade": grade, "density": density,
            "lum_range": lum_range, "slope": jsonable(slope), "pivot": jsonable(pivot),
            "output_encoded": jsonable(enc),
            "output_linear": jsonable(np.asarray(out)[0, :, 0]),
        }
    (OUT / "ramp257.json").write_text(json.dumps({"x": jsonable(x), "d_min": d_min, "cases": cases}, indent=1))
    print("ramp257.json dumped")


def dump_lab_color() -> None:
    """Saturation/vibrance parity: NegPy's CIELAB chroma ops (lab/logic.py)
    applied to a deterministic color-patch grid (vibrance first, then
    saturation — LabProcessor order)."""
    from negpy.features.lab.logic import apply_saturation, apply_vibrance

    n = 16
    img = np.zeros((n, n, 3), dtype=np.float32)
    for i in range(n):
        for j in range(n):
            img[i, j] = [i / (n - 1), j / (n - 1), ((i + j) / 2.0) / (n - 1)]
    img[0, :] = np.linspace(0.0, 1.0, n, dtype=np.float32)[:, None]  # neutral ramp row
    img[1, :, 0] = np.linspace(0.0, 1.0, n)  # saturated red ramp row
    img[1, :, 1] = 0.05
    img[1, :, 2] = 0.05

    root = OUT / "lab_color"
    root.mkdir(parents=True, exist_ok=True)
    manifest = {"input": dump_bin(root / "input.bin", img), "cases": {}}
    for name, (vib, sat) in {
        "vib_140": (1.4, 1.0),
        "sat_130": (1.0, 1.3),
        "combo": (1.5, 0.8),
        "extreme": (2.0, 1.6),
        "desat": (1.0, 0.3),
    }.items():
        res = apply_vibrance(img, vib)
        res = apply_saturation(res, sat)
        manifest["cases"][name] = {
            "vibrance": vib, "saturation": sat,
            "output": dump_bin(root / f"{name}.bin", np.asarray(res)),
        }
    (root / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print("lab_color dumped")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    dump_closed_form()
    dump_ramp()
    dump_synthetic64()
    dump_synthetic_grid()
    dump_lab_color()
    print(f"All fixtures written to {OUT}", file=sys.stderr)
