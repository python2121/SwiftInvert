#include <metal_stdlib>
using namespace metal;

// NegSwift render chain, ported from NegPy's WGSL shaders (C-41 path only:
// no B&W/E6 modes, no dye mix, no dodge/burn EV map, no crosstalk unmix).
// Compiled at runtime with MTLDevice.makeLibrary(source:).
//
// Uniform structs are mirrored byte-for-byte in ShaderTypes.swift; a layout
// test asserts the strides match.

struct NormUniforms {
    float4 floors;
    float4 ceils;
    float wpOffset;
    float bpOffset;
    float2 _pad;
};

struct CurveUniforms {
    float4 pivots;
    float4 slopes;
    float4 curvatures;
    float4 cmyOffsets;
    float4 shadowCMY;
    float4 midCMY;
    float4 highlightCMY;
    float4 dMinRGB;
    float toe;            // pre-scaled by toe_shoulder_strength
    float shoulder;
    float toeWidth;
    float shoulderWidth;
    float dMax;
    float aToeBase;
    float aShBase;
    float widthRef;
    float toeHeight;
    float shHeight;
    float zoneCenter;
    float flare;
    float surroundGamma;
    float vStar;
    float midtoneGamma;
    float gammaWidth;
    // Regional tone controls, pre-scaled by their K.*Max amplitudes.
    float shadowsLift;
    float shadowContrast;
    float highlightsShift;
    float highlightContrast;
    // CIELAB chroma ops (1.0 = off) + padding to a 16-byte multiple.
    float vibrance;
    float saturation;
    float2 _pad;
};

// Region-mask constants — must match K.toneRegionSharpness / K.*ToneAnchor
// in ExposureConstants.swift (GPU/CPU parity tests catch drift).
constant float TONE_SHARPNESS = 3.5f;
constant float SHADOW_ANCHOR = 1.40f;
constant float HIGHLIGHT_ANCHOR = 0.30f;

inline float fast_sigmoid(float x) {
    if (x >= 0.0f) { return 1.0f / (1.0f + exp(-x)); }
    float z = exp(x);
    return z / (1.0f + z);
}

// Numerically stable softplus log(1 + exp(x)).
inline float softplus(float x) {
    return max(x, 0.0f) + log(1.0f + exp(-abs(x)));
}

// Working-space OETF (ProPhoto ROMM: gamma 1.8 + linear toe below 1/512).
inline float oetf_encode(float t) {
    float x = clamp(t, 0.0f, 1.0f);
    return x < 0.001953125f ? x * 16.0f : pow(x, 0.55555556f);
}

// ── CIELAB in the working space (linear ProPhoto / ROMM, D50) — mirrors
//    LabColor.swift; constants must stay in sync. ─────────────────────────
constant float3x3 PROPHOTO_TO_XYZ = float3x3(
    float3(0.7976749f, 0.2880402f, 0.0f),       // columns (MSL is column-major)
    float3(0.1351917f, 0.7118741f, 0.0f),
    float3(0.0313534f, 0.0000857f, 0.8252100f));
constant float3x3 XYZ_TO_PROPHOTO = float3x3(
    float3(1.3459433f, -0.5445989f, 0.0f),
    float3(-0.2556075f, 1.5081673f, 0.0f),
    float3(-0.0511118f, 0.0205351f, 1.2118128f));
constant float3 D50_WHITE = float3(0.96422f, 1.0f, 0.82521f);
constant float LAB_EPS = 0.008856f;
constant float LAB_KAPPA = 7.787f;

inline float3 rgb_to_lab(float3 rgb) {
    float3 xyz = (PROPHOTO_TO_XYZ * max(rgb, float3(0.0f))) / D50_WHITE;
    float3 f = select(LAB_KAPPA * xyz + 16.0f / 116.0f, pow(xyz, float3(1.0f / 3.0f)), xyz > LAB_EPS);
    return float3(116.0f * f.y - 16.0f, 500.0f * (f.x - f.y), 200.0f * (f.y - f.z));
}

inline float3 lab_to_rgb(float3 lab) {
    float fy = (lab.x + 16.0f) / 116.0f;
    float3 f = float3(lab.y / 500.0f + fy, fy, fy - lab.z / 200.0f);
    float3 f3 = f * f * f;
    float3 xyz = select((f - 16.0f / 116.0f) / LAB_KAPPA, f3, f3 > LAB_EPS) * D50_WHITE;
    return max(XYZ_TO_PROPHOTO * xyz, float3(0.0f));
}

// ── Pass 1: log10 + per-channel stretch (normalization.wgsl) ──────────────
kernel void normalizeLog(
    texture2d<float, access::read> input [[texture(0)]],
    texture2d<float, access::write> output [[texture(1)]],
    constant NormUniforms &u [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= input.get_width() || gid.y >= input.get_height()) { return; }
    float3 color = input.read(gid).rgb;

    const float epsilon = 1e-6f;
    float3 logColor = log10(max(color, float3(epsilon)));

    float3 res;
    for (int ch = 0; ch < 3; ch++) {
        float f = u.floors[ch] + u.wpOffset;
        float c = u.ceils[ch] + u.bpOffset;
        float delta = c - f;
        float denom = delta;
        if (abs(delta) < epsilon) { denom = delta >= 0.0f ? epsilon : -epsilon; }
        res[ch] = (logColor[ch] - f) / denom;
    }
    output.write(float4(res, 1.0f), gid);
}

// ── Pass 2: asymmetric H&D print curve → LINEAR reflectance (exposure.wgsl,
//    minus its trailing oetf_encode — NegSwift keeps the linear/encoded split
//    at the same boundary as NegPy's CPU engine) ─────────────────────────────
kernel void printCurve(
    texture2d<float, access::read> input [[texture(0)]],
    texture2d<float, access::write> output [[texture(1)]],
    constant CurveUniforms &p [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= input.get_width() || gid.y >= input.get_height()) { return; }
    float3 color = input.read(gid).rgb;

    const float eps = 1e-6f;
    float a_hl = p.aShBase * p.widthRef / max(p.shoulderWidth, eps);
    float a_sh_base = p.aToeBase * p.widthRef / max(p.toeWidth, eps);
    float a_sh = p.toe >= 0.0f ? a_sh_base : a_sh_base * (1.0f - p.toe * 4.0f);
    float3 d_min_rgb = p.dMinRGB.xyz;
    float3 d_min_eff = max(d_min_rgb + float3(p.shoulder * p.shHeight), float3(0.0f));
    float d_max_base = p.toe >= 0.0f ? p.dMax - p.toe * p.toeHeight : p.dMax;
    float3 d_max_eff = max(float3(d_max_base), d_min_eff + float3(0.1f));
    float3 flare_white = pow(float3(10.0f), -d_min_rgb);
    bool hasBandCMY = any(p.shadowCMY.xyz != 0.0f) || any(p.midCMY.xyz != 0.0f)
        || any(p.highlightCMY.xyz != 0.0f);

    float3 dens;
    for (int ch = 0; ch < 3; ch++) {
        float val = color[ch] + p.cmyOffsets[ch];
        float v = p.slopes[ch] * (val - p.pivots[ch]) + p.curvatures[ch] * val * val;

        if (p.midtoneGamma != 0.0f) {
            v = v + p.midtoneGamma * p.gammaWidth * tanh((v - p.vStar) / p.gammaWidth);
        }

        // Regional tone: sigmoid-masked density shifts + anchor-pivoted contrast,
        // parallel form (both masks on the incoming v) — mirrors ReferenceCurve.
        if (p.shadowsLift != 0.0f || p.shadowContrast != 0.0f
            || p.highlightsShift != 0.0f || p.highlightContrast != 0.0f) {
            float wS = fast_sigmoid(TONE_SHARPNESS * (v - SHADOW_ANCHOR));
            float wH = fast_sigmoid(TONE_SHARPNESS * (HIGHLIGHT_ANCHOR - v));
            v = v + (-p.shadowsLift + p.shadowContrast * (v - SHADOW_ANCHOR)) * wS
                  + (-p.highlightsShift + p.highlightContrast * (v - HIGHLIGHT_ANCHOR)) * wH;
        }

        // 3-band color balance on the tone-region masks (mids = remainder) —
        // mirrors ReferenceCurve.applyPrintCurve. Uniform branch: free when off.
        if (hasBandCMY) {
            float wS = fast_sigmoid(TONE_SHARPNESS * (v - SHADOW_ANCHOR));
            float wH = fast_sigmoid(TONE_SHARPNESS * (HIGHLIGHT_ANCHOR - v));
            float wM = max(1.0f - wS - wH, 0.0f);
            v = v + p.shadowCMY[ch] * wS + p.midCMY[ch] * wM + p.highlightCMY[ch] * wH;
        }

        float v1 = d_min_eff[ch] + softplus(a_hl * (v - d_min_eff[ch])) / a_hl;
        dens[ch] = d_max_eff[ch] - softplus(a_sh * (d_max_eff[ch] - v1)) / a_sh;
    }

    float3 density = dens;
    if (p.surroundGamma != 1.0f) {
        density = d_min_rgb + p.surroundGamma * (density - d_min_rgb);
    }
    float3 transmittance = pow(float3(10.0f), -density);
    if (p.flare != 0.0f) {
        transmittance = (transmittance + p.flare * flare_white) / (1.0f + p.flare);
    }
    output.write(float4(clamp(transmittance, float3(0.0f), float3(1.0f)), 1.0f), gid);
}

// ── Color pop: vibrance then saturation in CIELAB on the linear print
//    (NegPy lab stage; mirrors LabColor.applyVibranceSaturation). A separate
//    pass dispatched only when active — keeping the Lab code out of printCurve
//    preserves that kernel's occupancy. ────────────────────────────────────
kernel void colorPop(
    texture2d<float, access::read> input [[texture(0)]],
    texture2d<float, access::write> output [[texture(1)]],
    constant CurveUniforms &p [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= input.get_width() || gid.y >= input.get_height()) { return; }
    float3 result = input.read(gid).rgb;

    if (p.vibrance != 1.0f) {
        float3 lab = rgb_to_lab(result);
        float chroma = length(lab.yz);
        float muted = clamp(1.0f - chroma / 60.0f, 0.0f, 1.0f);
        float boost = 1.0f + (p.vibrance - 1.0f) * muted;
        lab.yz *= boost;
        result = clamp(lab_to_rgb(lab), float3(0.0f), float3(1.0f));
    }
    if (p.saturation != 1.0f) {
        float3 lab = rgb_to_lab(result);
        lab.yz *= p.saturation;
        result = clamp(lab_to_rgb(lab), float3(0.0f), float3(1.0f));
    }

    output.write(float4(result, 1.0f), gid);
}

// ── Pass 3: working-space OETF encode (output_encode.wgsl) ────────────────
kernel void outputEncode(
    texture2d<float, access::read> input [[texture(0)]],
    texture2d<float, access::write> output [[texture(1)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= input.get_width() || gid.y >= input.get_height()) { return; }
    float3 c = input.read(gid).rgb;
    output.write(float4(oetf_encode(c.x), oetf_encode(c.y), oetf_encode(c.z), 1.0f), gid);
}

// ── Histogram: 4×256 bins (R, G, B, Rec.709 luma), reads the LINEAR curve
//    output and encodes internally (metrics.wgsl) ─────────────────────────
kernel void histogram256(
    texture2d<float, access::read> input [[texture(0)]],
    device atomic_uint *bins [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]]
) {
    if (gid.x >= input.get_width() || gid.y >= input.get_height()) { return; }
    float3 raw = input.read(gid).rgb;
    float3 color = float3(oetf_encode(raw.x), oetf_encode(raw.y), oetf_encode(raw.z));
    float luma = dot(color, float3(0.2126f, 0.7152f, 0.0722f));

    uint binR = uint(clamp(color.r * 255.0f, 0.0f, 255.0f));
    uint binG = uint(clamp(color.g * 255.0f, 0.0f, 255.0f));
    uint binB = uint(clamp(color.b * 255.0f, 0.0f, 255.0f));
    uint binL = uint(clamp(luma * 255.0f, 0.0f, 255.0f));

    atomic_fetch_add_explicit(&bins[binR], 1u, memory_order_relaxed);
    atomic_fetch_add_explicit(&bins[256u + binG], 1u, memory_order_relaxed);
    atomic_fetch_add_explicit(&bins[512u + binB], 1u, memory_order_relaxed);
    atomic_fetch_add_explicit(&bins[768u + binL], 1u, memory_order_relaxed);
}
