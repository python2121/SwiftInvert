// Linux has no Apple `simd` module. The stdlib SIMD types (SIMD2/3/4<Double>)
// are cross-platform; only these free functions need standing in. Semantics
// match Apple's for finite inputs — the kernel never feeds NaN.
#if !canImport(simd)
@inlinable
func simd_dot(_ a: SIMD3<Double>, _ b: SIMD3<Double>) -> Double {
    (a * b).sum()
}

@inlinable
func simd_max(_ a: SIMD3<Double>, _ b: SIMD3<Double>) -> SIMD3<Double> {
    pointwiseMax(a, b)
}

@inlinable
func simd_clamp(_ x: SIMD3<Double>, _ lo: SIMD3<Double>, _ hi: SIMD3<Double>) -> SIMD3<Double> {
    x.clamped(lowerBound: lo, upperBound: hi)
}

@inlinable
func simd_length(_ v: SIMD2<Double>) -> Double {
    (v * v).sum().squareRoot()
}

@inlinable
func simd_length(_ v: SIMD3<Double>) -> Double {
    (v * v).sum().squareRoot()
}
#endif
