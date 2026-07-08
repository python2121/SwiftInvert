import Foundation

/// Scalar CPU implementation of the render chain — the numerical reference the
/// Metal shaders are tested against, and the port of NegPy's
/// _normalize_log_image_jit + _apply_print_curve_kernel + working_oetf_encode.
/// Not used for interactive rendering (that's MetalRenderKit).
public enum ReferenceCurve {
    /// normalize_log_image: log10 (lower-clip only) → per-channel stretch, unclamped.
    public static func normalize(_ image: RGBImage, bounds: LogNegativeBounds) -> RGBImage {
        var out = image
        let eps: Float = 1e-6
        var floors = SIMD3<Float>(), invD = SIMD3<Float>()
        for ch in 0..<3 {
            floors[ch] = Float(bounds.floors[ch])
            var denom = Float(bounds.ceils[ch]) - floors[ch]
            if abs(denom) < eps { denom = denom >= 0 ? eps : -eps }
            invD[ch] = 1.0 / denom
        }
        out.pixels.withUnsafeMutableBufferPointer { buf in
            var i = 0
            while i < buf.count {
                for ch in 0..<3 {
                    var v = buf[i + ch]
                    if v.isNaN { v = eps }
                    if v.isInfinite { v = v > 0 ? 1.0 : eps }
                    let lg = log10(max(v, eps))  // no upper clamp (matches the WGSL)
                    buf[i + ch] = (lg - floors[ch]) * invD[ch]
                }
                i += 3
            }
        }
        return out
    }

    /// _apply_print_curve_kernel (C-41 path: no B&W collapse, no dye mix, no EV
    /// map). Input normalized log; output linear reflectance in [0, 1].
    public static func applyPrintCurve(
        _ img: RGBImage,
        params: RenderParams,
        shadowCMY: SIMD3<Double> = .zero,
        highlightCMY: SIMD3<Double> = .zero,
        flare: Double = 0,
        surroundGamma: Double = 1.0
    ) -> RGBImage {
        let eps = 1e-6
        let toe = params.toeEff * K.toeShoulderStrength
        let shoulder = params.shoulderEff * K.toeShoulderStrength

        let aHl = K.shoulderSharpnessBase * K.toeShoulderWidthRef / max(params.shoulderWidth, eps)
        var aSh = K.toeSharpnessBase * K.toeShoulderWidthRef / max(params.toeWidth, eps)
        var dMaxBase = K.dMax
        if toe >= 0 {
            dMaxBase = K.dMax - toe * K.toeHeight
        } else {
            aSh *= (1.0 - toe * 4.0)
        }

        // Neutral paper: d_min_rgb is uniform.
        let dMinRGB = SIMD3<Double>(repeating: max(params.dMin, 0))
        var dMinEff = SIMD3<Double>(), dMaxEff = SIMD3<Double>(), flareWhite = SIMD3<Double>()
        for ch in 0..<3 {
            var dmn = dMinRGB[ch] + shoulder * K.shoulderHeight
            if dmn < 0 { dmn = 0 }
            var dmx = dMaxBase
            if dmx < dmn + 0.1 { dmx = dmn + 0.1 }
            dMinEff[ch] = dmn
            dMaxEff[ch] = dmx
            flareWhite[ch] = pow(10.0, -dMinRGB[ch])
        }

        let zoneCenter = K.anchorTargetDensity
        let midtoneGamma = K.paperMidtoneGamma
        let gammaWidth = K.paperGammaWidth

        var out = img
        out.pixels.withUnsafeMutableBufferPointer { buf in
            var i = 0
            while i < buf.count {
                for ch in 0..<3 {
                    let val = Double(buf[i + ch]) + params.cmyOffsets[ch]
                    var v = params.slopes[ch] * (val - params.pivots[ch]) + params.curvatures[ch] * val * val
                    if midtoneGamma != 0 {
                        v += midtoneGamma * gammaWidth * tanh((v - params.vStar) / gammaWidth)
                    }
                    let wSh = CurveLogic.sigmoid(3.0 * (v - zoneCenter))
                    v += shadowCMY[ch] * wSh + highlightCMY[ch] * (1.0 - wSh)

                    let v1 = dMinEff[ch] + CurveLogic.softplus(aHl * (v - dMinEff[ch])) / aHl
                    var density = dMaxEff[ch] - CurveLogic.softplus(aSh * (dMaxEff[ch] - v1)) / aSh

                    if surroundGamma != 1.0 {
                        density = dMinRGB[ch] + surroundGamma * (density - dMinRGB[ch])
                    }
                    var t = pow(10.0, -density)
                    if flare != 0 {
                        t = (t + flare * flareWhite[ch]) / (1.0 + flare)
                    }
                    buf[i + ch] = Float(min(max(t, 0.0), 1.0))
                }
                i += 3
            }
        }
        return out
    }

    /// Working-space OETF encode over a whole buffer (final engine step).
    public static func encodeOutput(_ img: RGBImage) -> RGBImage {
        var out = img
        out.pixels.withUnsafeMutableBufferPointer { buf in
            for i in 0..<buf.count { buf[i] = WorkingOETF.encode(buf[i]) }
        }
        return out
    }

    /// Full CPU render: normalize → print curve → encode.
    public static func render(linearImage: RGBImage, settings: ExposureSettings, analysis: ExposureAnalysis) -> RGBImage {
        let params = ExposureKernel.deriveRenderParams(settings, analysis)
        let normalized = normalize(linearImage, bounds: params.finalBounds)
        let positive = applyPrintCurve(normalized, params: params)
        return encodeOutput(positive)
    }
}
