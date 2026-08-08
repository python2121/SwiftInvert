// NOTE: this file is shared — Sources/VulkanRenderKit/ShaderTypes.swift is a
// symlink to it, so the uniform packing has exactly one source of truth on
// both platforms. Keep it building without Apple frameworks.
import Foundation
import NegativeKit
#if canImport(simd)
import simd
#endif

/// Swift mirrors of the MSL uniform structs in NegPipeline.metal. Field order
/// and alignment must match exactly — LayoutTests asserts the strides.
public struct NormUniforms {
    public var floors: SIMD4<Float>
    public var ceils: SIMD4<Float>
    public var wpOffset: Float
    public var bpOffset: Float
    public var _pad: SIMD2<Float> = .zero
}

public struct CurveUniforms {
    public var pivots: SIMD4<Float>
    public var slopes: SIMD4<Float>
    public var curvatures: SIMD4<Float>
    public var cmyOffsets: SIMD4<Float>
    public var shadowCMY: SIMD4<Float>
    public var midCMY: SIMD4<Float>
    public var highlightCMY: SIMD4<Float>
    public var dMinRGB: SIMD4<Float>
    public var toe: Float
    public var shoulder: Float
    public var toeWidth: Float
    public var shoulderWidth: Float
    public var dMax: Float
    public var aToeBase: Float
    public var aShBase: Float
    public var widthRef: Float
    public var toeHeight: Float
    public var shHeight: Float
    public var zoneCenter: Float
    public var trueBlack: Float
    public var darkShadowsLift: Float
    public var vStar: Float
    public var midtoneGamma: Float
    public var gammaWidth: Float
    public var shadowsLift: Float
    public var shadowContrast: Float
    public var highlightsShift: Float
    public var highlightContrast: Float
    public var vibrance: Float
    public var saturation: Float
    public var preSaturation: Float
    // Post-curve density-space chroma (rides the ex-pad slot: layout stable).
    public var printSaturation: Float
    // Color-mixer bands, R/Y/G/B lanes.
    public var bandHues: SIMD4<Float>
    public var bandSaturations: SIMD4<Float>
    // Separation Damping (0 = off) + Skin Protection rein strength
    // (0 = off); pads keep the 16-byte stride.
    public var separationDamping: Float
    public var skinProtection: Float
    /// Hue Trim in radians (rides the ex-pad slot: stride unchanged at 272).
    public var hueTrim: Float = 0
    public var _pad3: Float = 0
}


/// RenderParams (NegativeKit's per-slider derivation) → GPU uniform packing.
/// The same single-source-of-truth role as NegPy's _upload_unified_uniforms.
public enum UniformsBuilder {
    static func f4(_ v: SIMD3<Double>, _ w: Float = 0) -> SIMD4<Float> {
        SIMD4(Float(v.x), Float(v.y), Float(v.z), w)
    }

    public static func normUniforms(_ params: RenderParams) -> NormUniforms {
        // wp/bp offsets are already folded into finalBounds by deriveRenderParams.
        NormUniforms(
            floors: f4(params.finalBounds.floors),
            ceils: f4(params.finalBounds.ceils),
            wpOffset: 0,
            bpOffset: 0
        )
    }

    public static func curveUniforms(_ params: RenderParams) -> CurveUniforms {
        CurveUniforms(
            pivots: f4(params.pivots),
            slopes: f4(params.slopes),
            curvatures: f4(params.curvatures),
            cmyOffsets: f4(params.cmyOffsets),
            shadowCMY: f4(params.shadowCMY),
            midCMY: f4(params.midCMY),
            highlightCMY: f4(params.highlightCMY),
            dMinRGB: SIMD4(Float(max(params.dMin, 0)), Float(max(params.dMin, 0)), Float(max(params.dMin, 0)), 0),
            toe: Float(params.toeEff * K.toeShoulderStrength),
            shoulder: Float(params.shoulderEff * K.toeShoulderStrength),
            toeWidth: Float(params.toeWidth),
            shoulderWidth: Float(params.shoulderWidth),
            dMax: Float(K.dMax),
            aToeBase: Float(K.toeSharpnessBase),
            aShBase: Float(K.shoulderSharpnessBase),
            widthRef: Float(K.toeShoulderWidthRef),
            toeHeight: Float(K.toeHeight),
            shHeight: Float(K.shoulderHeight),
            zoneCenter: Float(K.anchorTargetDensity),
            trueBlack: params.trueBlack ? 1 : 0,
            darkShadowsLift: Float(params.darkShadows * K.shadowsMaxLift),
            vStar: Float(params.vStar),
            midtoneGamma: Float(K.paperMidtoneGamma),
            gammaWidth: Float(K.paperGammaWidth),
            shadowsLift: Float(params.shadows * K.shadowsMaxLift),
            shadowContrast: Float(max(params.shadowContrast * K.shadowContrastMax, K.shadowContrastNegFloor)),
            highlightsShift: Float(params.highlights * K.highlightsMaxShift),
            highlightContrast: Float(params.highlightContrast * K.highlightContrastMax),
            vibrance: Float(params.vibrance),
            saturation: Float(params.saturation),
            preSaturation: Float(params.preSaturation),
            printSaturation: Float(params.printSaturation),
            bandHues: SIMD4<Float>(params.bandHues),
            bandSaturations: SIMD4<Float>(params.bandSaturations),
            separationDamping: Float(params.separationDamping),
            skinProtection: Float(params.skinProtection),
            hueTrim: Float(params.hueTrim)
        )
    }

    /// Flat float layout consumed by `levels_remap` in NegPipeline.metal
    /// (LEVELS_BUFFER_FLOATS there — keep in sync; LayoutTests pins it):
    /// per channel c, [c*16 + 0..7] anchor inputs, [c*16 + 8..15] anchor
    /// outputs, [48 + c] anchor count. Anchors come pre-sanitized from
    /// deriveRenderParams (sorted, monotone, ≤ 8).
    public static let levelsBufferFloats = 51

    public static func levelsBuffer(_ params: RenderParams) -> [Float] {
        var buf = [Float](repeating: 0, count: levelsBufferFloats)
        for ch in 0..<3 {
            let points = ch < params.levelsPoints.count ? params.levelsPoints[ch] : []
            for (i, p) in points.prefix(8).enumerated() {
                buf[ch * 16 + i] = Float(p.x)
                buf[ch * 16 + 8 + i] = Float(p.y)
            }
            buf[48 + ch] = Float(min(points.count, 8))
        }
        return buf
    }
}
