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
    /// map; NegPy's 2-band regional CMY generalized to the 3-band tone-mask
    /// color balance). Input normalized log; output linear reflectance in [0, 1].
    public static func applyPrintCurve(
        _ img: RGBImage,
        params: RenderParams
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
        var dMinEff = SIMD3<Double>(), dMaxEff = SIMD3<Double>()
        for ch in 0..<3 {
            var dmn = dMinRGB[ch] + shoulder * K.shoulderHeight
            if dmn < 0 { dmn = 0 }
            var dmx = dMaxBase
            if dmx < dmn + 0.1 { dmx = dmn + 0.1 }
            dMinEff[ch] = dmn
            dMaxEff[ch] = dmx
        }
        // True Black (BPC) references the PHYSICAL d_max (not d_max_eff) so toe
        // lifts survive; a negative toe raises the clip point — the softplus
        // bound reaches d_max only asymptotically (mirrors NegPy 0.36).
        var bpcDb = K.dMax
        if toe < 0 { bpcDb = K.dMax + toe * K.toeHeight }
        let bpcBlack = pow(10.0, -bpcDb)

        let midtoneGamma = K.paperMidtoneGamma
        let gammaWidth = K.paperGammaWidth

        // Regional tone controls, all-zero → identity (fixture parity preserved).
        let hasTone = params.shadows != 0 || params.shadowContrast != 0
            || params.darkShadows != 0
            || params.highlights != 0 || params.highlightContrast != 0
        let hasBandCMY = params.shadowCMY != .zero || params.midCMY != .zero
            || params.highlightCMY != .zero
        let shLift = params.shadows * K.shadowsMaxLift
        let dsLift = params.darkShadows * K.shadowsMaxLift
        let shContrast = max(params.shadowContrast * K.shadowContrastMax, K.shadowContrastNegFloor)
        let hiShift = params.highlights * K.highlightsMaxShift
        let hiContrast = params.highlightContrast * K.highlightContrastMax

        let preSat = params.preSaturation
        var out = img
        out.pixels.withUnsafeMutableBufferPointer { buf in
            var i = 0
            while i < buf.count {
                // Pre-saturation: scale density deviations from the per-pixel
                // neutral (channel mean) before any print decision.
                if preSat != 1.0 {
                    let m = (Double(buf[i]) + Double(buf[i + 1]) + Double(buf[i + 2])) / 3.0
                    for ch in 0..<3 {
                        buf[i + ch] = Float(m + preSat * (Double(buf[i + ch]) - m))
                    }
                }
                for ch in 0..<3 {
                    let val = Double(buf[i + ch]) + params.cmyOffsets[ch]
                    var v = params.slopes[ch] * (val - params.pivots[ch]) + params.curvatures[ch] * val * val
                    if midtoneGamma != 0 {
                        v += midtoneGamma * gammaWidth * tanh((v - params.vStar) / gammaWidth)
                    }

                    // Regional tone: sigmoid-masked density shifts + anchor-pivoted
                    // contrast, parallel form (both masks on the incoming v).
                    if hasTone {
                        let wS = CurveLogic.sigmoid(K.toneRegionSharpness * (v - K.shadowToneAnchor))
                        let wH = CurveLogic.sigmoid(K.toneRegionSharpness * (K.highlightToneAnchor - v))
                        let wDS = CurveLogic.sigmoid(K.toneRegionSharpness * (v - K.darkShadowToneAnchor))
                        v += (-shLift + shContrast * (v - K.shadowToneAnchor)) * wS
                        v += -dsLift * wDS
                        v += (-hiShift + hiContrast * (v - K.highlightToneAnchor)) * wH
                    }

                    // 3-band color balance on the same tone-region masks
                    // (mids = whatever the shadow/highlight bands don't claim).
                    if hasBandCMY {
                        let wS = CurveLogic.sigmoid(K.toneRegionSharpness * (v - K.shadowToneAnchor))
                        let wH = CurveLogic.sigmoid(K.toneRegionSharpness * (K.highlightToneAnchor - v))
                        let wM = max(1.0 - wS - wH, 0.0)
                        v += params.shadowCMY[ch] * wS + params.midCMY[ch] * wM
                            + params.highlightCMY[ch] * wH
                    }

                    let v1 = dMinEff[ch] + CurveLogic.softplus(aHl * (v - dMinEff[ch])) / aHl
                    let density = dMaxEff[ch] - CurveLogic.softplus(aSh * (dMaxEff[ch] - v1)) / aSh
                    var t = pow(10.0, -density)
                    if params.trueBlack {
                        t = (t - bpcBlack) / (1.0 - bpcBlack)
                    }
                    buf[i + ch] = Float(min(max(t, 0.0), 1.0))
                }
                // Color pop on the linear print (reds band, then NegPy lab
                // stage: vibrance, then saturation), identity at defaults.
                if params.redHue != 0 || params.redSaturation != 1.0
                    || params.vibrance != 1.0 || params.saturation != 1.0 {
                    var rgb = SIMD3(Double(buf[i]), Double(buf[i + 1]), Double(buf[i + 2]))
                    rgb = LabColor.applyRedBand(
                        rgb, hue: params.redHue, saturation: params.redSaturation)
                    let res = LabColor.applyVibranceSaturation(
                        rgb, vibrance: params.vibrance, saturation: params.saturation)
                    buf[i] = Float(res.x)
                    buf[i + 1] = Float(res.y)
                    buf[i + 2] = Float(res.z)
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
