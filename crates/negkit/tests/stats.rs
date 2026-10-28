//! Ports of `RadixSortTests.swift` and the Stats half of `AuditStatsAndLevelsTests.swift`.
//!
//! The levels-remap / inverse-remap round trip (`levelsInverseRoundTrip`) and the
//! `AuditGamutBoostTests` suite from that file belong to the curve / lab ports (agent C,
//! `tests/curve.rs` / `tests/lab.rs`) and are NOT here.
//!
//! `Stats.sortedAscending` is a radix sort, and every order statistic in the analysis funnels
//! through it — so its output must be indistinguishable from a comparison sort's on anything the
//! meters can produce. These pin that against `sort_by(total_cmp)` on the adversarial shapes
//! (sign boundaries, duplicates, denormals, the small-array threshold) rather than on the happy
//! path the fixtures already cover.

use std::collections::HashSet;

use negkit::stats;

/// Deterministic pseudo-random log-density values (no RNG crate: the suite must be
/// reproducible). Mirrors the Swift LCG bit for bit.
fn sample(n: usize, seed: u64, span: f32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            -((s >> 40) as f32) / 16_777_216.0 * span
        })
        .collect()
}

fn stdlib_sorted_f32(data: &[f32]) -> Vec<f32> {
    let mut v = data.to_vec();
    v.sort_by(f32::total_cmp);
    v
}

fn stdlib_sorted_f64(data: &[f64]) -> Vec<f64> {
    let mut v = data.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

// MARK: - RadixSortTests

#[test]
fn matches_stdlib_on_realistic_grid_sizes() {
    // Straddles the 512 threshold in both directions, and covers the real preview grid
    // (615×410 = 252k cells per channel).
    for n in [1usize, 2, 7, 511, 512, 513, 4096, 252_150] {
        let data = sample(n, n as u64 + 99, 3.0);
        assert_eq!(
            stats::sorted_ascending(&data),
            stdlib_sorted_f32(&data),
            "n = {n}"
        );
    }
}

#[test]
fn matches_stdlib_across_the_sign_boundary() {
    // The key transform's whole job: negatives run backwards in raw bit order and the sign bit
    // outranks magnitude. Mix both signs densely.
    let mut data = sample(2000, 7, 4.0);
    for i in (0..data.len()).step_by(2) {
        data[i] = -data[i];
    }
    assert_eq!(stats::sorted_ascending(&data), stdlib_sorted_f32(&data));

    let doubles: Vec<f64> = data.iter().map(|&v| f64::from(v)).collect();
    assert_eq!(
        stats::sorted_ascending(&doubles),
        stdlib_sorted_f64(&doubles)
    );
}

#[test]
fn matches_stdlib_on_extremes_and_denormals() {
    let corners: [f32; 14] = [
        0.0,
        -0.0,
        f32::from_bits(1),  // leastNonzeroMagnitude
        -f32::from_bits(1), // −leastNonzeroMagnitude
        f32::MIN_POSITIVE,  // leastNormalMagnitude
        -f32::MIN_POSITIVE,
        f32::MAX,
        -f32::MAX,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1.0,
        -1.0,
        1e-6,
        -6.0,
    ];
    // Padded past the threshold so the radix path (not the fallback) runs.
    let mut data = corners.to_vec();
    data.extend(sample(600, 31, 3.0));
    let ours = stats::sorted_ascending(&data);
    let theirs = stdlib_sorted_f32(&data);
    // Compare by value: ±0.0 tie, and either order is a valid sort.
    assert_eq!(ours, theirs);

    let doubles: Vec<f64> = data.iter().map(|&v| f64::from(v)).collect();
    assert_eq!(
        stats::sorted_ascending(&doubles),
        stdlib_sorted_f64(&doubles)
    );
}

#[test]
fn heavy_duplicates_preserve_counts() {
    // Block-median output is highly quantized — long runs of equal values are the norm, and a
    // miscounted histogram bucket would drop or duplicate elements rather than misorder them.
    let data: Vec<f32> = (0..4000).map(|i| (i % 7) as f32 * -0.5).collect();
    let sorted = stats::sorted_ascending(&data);
    assert_eq!(sorted, stdlib_sorted_f32(&data));
    assert_eq!(sorted.len(), data.len());
    let distinct: HashSet<u32> = data.iter().map(|v| v.to_bits()).collect();
    for bits in distinct {
        let v = f32::from_bits(bits);
        assert_eq!(
            sorted.iter().filter(|&&x| x == v).count(),
            data.iter().filter(|&&x| x == v).count()
        );
    }
}

#[test]
fn round_trips_values_exactly() {
    // unkey ∘ key must be the identity: a lossy transform would silently shift percentile
    // VALUES while keeping the order plausible.
    let data = sample(1500, 5150, 6.0);
    let sorted = stats::sorted_ascending(&data);
    let a: HashSet<u32> = sorted.iter().map(|v| v.to_bits()).collect();
    let b: HashSet<u32> = data.iter().map(|v| v.to_bits()).collect();
    assert_eq!(a, b);

    let doubles: Vec<f64> = data.iter().map(|&v| f64::from(v)).collect();
    let sorted_d = stats::sorted_ascending(&doubles);
    let a: HashSet<u64> = sorted_d.iter().map(|v| v.to_bits()).collect();
    let b: HashSet<u64> = doubles.iter().map(|v| v.to_bits()).collect();
    assert_eq!(a, b);
}

#[test]
fn percentiles_agree_with_a_comparison_sort() {
    // The property that actually matters downstream: identical order statistics at every clip
    // the meters use.
    let data = sample(50_000, 4242, 3.0);
    let ours = stats::sorted_ascending(&data);
    let theirs = stdlib_sorted_f32(&data);
    for q in [0.0, 0.01, 1.0, 2.0, 10.0, 50.0, 90.0, 98.0, 99.0, 100.0] {
        assert_eq!(
            stats::percentile_of_sorted(&ours, q),
            stats::percentile_of_sorted(&theirs, q),
            "q = {q}"
        );
    }
    assert_eq!(stats::median(&data), stats::median(&theirs));
}

// MARK: - AuditStatsAndLevelsTests (Stats endpoints — numpy parity corners)

#[test]
fn percentile_endpoints_float_and_double() {
    let f: [f32; 4] = [-2.5, -1.0, 0.25, 3.5];
    let d: [f64; 4] = [-2.5, -1.0, 0.25, 3.5];
    // q = 0: pos = 0, frac = 0 → lower element exactly.
    assert_eq!(stats::percentile_of_sorted(&f, 0.0), Some(-2.5));
    assert_eq!(stats::percentile_of_sorted(&d, 0.0), Some(-2.5));
    // q = 100: pos = n−1, lo = n−1, hi = n−1, frac = 0 → last element.
    assert_eq!(stats::percentile_of_sorted(&f, 100.0), Some(3.5));
    assert_eq!(stats::percentile_of_sorted(&d, 100.0), Some(3.5));
}

#[test]
fn single_element_arrays() {
    assert_eq!(stats::percentile_of_sorted(&[0.75f32], 0.0), Some(0.75));
    assert_eq!(stats::percentile_of_sorted(&[0.75f32], 37.2), Some(0.75));
    assert_eq!(stats::percentile_of_sorted(&[0.75f32], 100.0), Some(0.75));
    assert_eq!(stats::percentile_of_sorted(&[-1.25f64], 50.0), Some(-1.25));
    assert_eq!(stats::median(&[0.5f32]), Some(0.5));
    assert_eq!(stats::median(&[0.5f64]), Some(0.5));
}

#[test]
fn even_count_median_both_overloads() {
    // n = 4: mean of the two middle values, (2 + 3) / 2 = 2.5.
    assert_eq!(stats::median(&[1f32, 2.0, 3.0, 4.0]), Some(2.5));
    assert_eq!(stats::median(&[1f64, 2.0, 3.0, 4.0]), Some(2.5));
    // n = 2 (the block-median fallback shape).
    assert_eq!(stats::median(&[0.25f32, 0.75]), Some(0.5));
    let mut scratch: [f32; 4] = [4.0, 1.0, 3.0, 2.0];
    assert_eq!(stats::median_in_place(&mut scratch), Some(2.5));
}

/// numpy's _lerp two-sided branch: frac ≥ 0.5 computes b − (b−a)(1−t), NOT a + t(b−a). For
/// sorted [0, 1] both forms are exact; for [0.1, 0.4] (f64 literals) the two forms differ in
/// the last ulp, which pins the branch bit-for-bit.
///
/// NOTE (pinned differently than specced): for a = 0.1, b = 0.3 the two forms happen to agree
/// bit-for-bit in f64 (verified empirically), so that pair cannot discriminate the branch.
/// a = 0.1, b = 0.4 does: b − (b−a)·0.25 = 0.32500000000000007 ≠ a + 0.75·(b−a) = 0.325…01 off
/// by one ulp. Both the specced pair and the discriminating pair are asserted against the
/// two-sided form.
#[test]
fn two_sided_lerp_branch_exactness() {
    // Sorted [0, 1], q = 75 → pos = 0.75, frac = 0.75 ≥ 0.5.
    assert_eq!(
        stats::percentile_of_sorted(&[0f32, 1.0], 75.0),
        Some(1.0 - (1.0 - 0.0) * 0.25)
    );
    assert_eq!(
        stats::percentile_of_sorted(&[0f64, 1.0], 75.0),
        Some(1.0 - (1.0 - 0.0) * 0.25)
    );

    // Specced pair (0.1, 0.3): still must equal the two-sided form exactly (bit comparison) in
    // both overloads.
    {
        let (a_f, b_f) = (0.1f32, 0.3f32);
        let (a, b) = (f64::from(a_f), f64::from(b_f));
        let expected = b - (b - a) * 0.25;
        assert_eq!(
            stats::percentile_of_sorted(&[a_f, b_f], 75.0),
            Some(expected)
        );
    }
    {
        let (a, b) = (0.1f64, 0.3f64);
        assert_eq!(
            stats::percentile_of_sorted(&[a, b], 75.0),
            Some(b - (b - a) * 0.25)
        );
    }

    // Discriminating pair (f64 overload): the one-sided form differs by one ulp, so this is a
    // true branch pin.
    {
        let (a, b) = (0.1f64, 0.4f64);
        let two_sided = b - (b - a) * 0.25;
        let one_sided = a + 0.75 * (b - a);
        assert_ne!(two_sided, one_sided); // the pair actually discriminates
        assert_eq!(stats::percentile_of_sorted(&[a, b], 75.0), Some(two_sided));
    }
}

// MARK: - Rust-side contract: the no-panic edges return None rather than trapping.

#[test]
fn empty_input_is_none_not_a_trap() {
    let empty: [f32; 0] = [];
    assert_eq!(stats::percentile(&empty, 50.0), None);
    assert_eq!(stats::percentile_of_sorted(&empty, 50.0), None);
    assert_eq!(stats::median(&empty), None);
    assert_eq!(stats::quantile(&empty, 0.5), None);
    let mut scratch: [f32; 0] = [];
    assert_eq!(stats::median_in_place(&mut scratch), None);
    // quantile is percentile with q in [0, 1].
    assert_eq!(
        stats::quantile(&[1f32, 2.0, 3.0, 4.0], 0.5),
        stats::percentile(&[1f32, 2.0, 3.0, 4.0], 50.0)
    );
}
