import Foundation
import NegativeKit
import simd

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
    public var _pad: Float = 0
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
            preSaturation: Float(params.preSaturation)
        )
    }
}
