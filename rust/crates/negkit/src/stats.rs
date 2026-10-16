//! numpy-compatible order statistics — port of `Stats.swift`.
//!
//! Linear-interpolation percentile, even/odd median. NegPy's bounds analysis is built on
//! `np.percentile` / `np.median`, so these must match exactly. Every percentile / median /
//! quantile funnels through [`sorted_ascending`] — the radix sort — which is the ONLY sort pixel
//! statistics may use (see `RadixSort.swift`'s measurements in [`crate::radix_sort`]). Keep new
//! percentile/median code going through here rather than calling `.sort()` directly.
//!
//! The Swift functions trap on empty input (`precondition`); the Rust ports return `None` there
//! (and for a `q` that would index past the data), so the no-panic crate rule holds.
//!
//! `f32` and `f64` overloads are one generic each: the same-pixel colour-floor refs run in float64
//! upstream (the one analysis path that does), so their order statistics must too.

use crate::radix_sort::{self, RadixSortable};

/// Ascending sort — same ordering as a comparison sort for the NaN-free float data the meters
/// see. Radix, not comparison: sorting is the bulk of analysis and this is ~11× faster than
/// `vDSP_vsort` was in the Swift app (see [`crate::radix_sort`]).
pub fn sorted_ascending<T: RadixSortable>(data: &[T]) -> Vec<T> {
    radix_sort::sorted(data)
}

/// `np.percentile(data, q)` with the default "linear" interpolation:
/// `pos = q/100 · (n−1)`; `result = lower + frac · (upper − lower)`. Sorts a copy; `q` in `[0, 100]`.
/// `None` on empty input.
pub fn percentile<T: RadixSortable>(data: &[T], q: f64) -> Option<f64> {
    percentile_of_sorted(&sorted_ascending(data), q)
}

/// Percentile over already-sorted data (for multiple `q`s on one sort). `None` on empty input
/// or a `q` outside `[0, 100]` that would index past the data.
pub fn percentile_of_sorted<T: RadixSortable>(sorted: &[T], q: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 1 {
        return sorted.first().map(|v| v.to_f64());
    }
    let n_minus_1 = n.checked_sub(1)?;
    let pos = q / 100.0 * n_minus_1 as f64;
    if pos.is_nan() || pos < 0.0 {
        // NaN or negative: Swift's `Int(pos)` would trap here.
        return None;
    }
    // `pos` is finite and non-negative; the cast truncates like Swift's `Int(pos.rounded(.down))`.
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(n_minus_1);
    let frac = pos - lo as f64;
    let a = sorted.get(lo)?.to_f64();
    let b = sorted.get(hi)?.to_f64();
    // numpy's _lerp mirrors the interpolation above t = 0.5 (last-ulp parity).
    Some(if frac >= 0.5 {
        b - (b - a) * (1.0 - frac)
    } else {
        a + frac * (b - a)
    })
}

/// `np.quantile` — percentile with `q` in `[0, 1]`. `None` on empty input.
pub fn quantile<T: RadixSortable>(data: &[T], q: f64) -> Option<f64> {
    percentile(data, q * 100.0)
}

/// `np.median` — middle value, or the mean of the two middle values. `None` on empty input.
pub fn median<T: RadixSortable>(data: &[T]) -> Option<f64> {
    let sorted = sorted_ascending(data);
    median_of_sorted(&sorted)
}

/// Median of already-sorted data (the tail of [`median`], shared with [`median_in_place`]).
pub fn median_of_sorted<T: RadixSortable>(sorted: &[T]) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    if n % 2 == 1 {
        return sorted.get(n / 2).map(|v| v.to_f64());
    }
    let a = sorted.get(n / 2 - 1)?.to_f64();
    let b = sorted.get(n / 2)?.to_f64();
    Some((a + b) / 2.0)
}

/// In-place median of a small scratch slice (block-median inner loop; avoids per-cell
/// allocation). Sorts `scratch` with a comparison sort — the slice is `b×b` elements, far below
/// the radix threshold — and returns the `f32` median exactly as Swift does (its even-count
/// mean is computed in `Float`). `None` on an empty slice.
#[inline]
pub fn median_in_place(scratch: &mut [f32]) -> Option<f32> {
    let n = scratch.len();
    if n == 0 {
        return None;
    }
    scratch.sort_by(f32::total_cmp);
    if n % 2 == 1 {
        return scratch.get(n / 2).copied();
    }
    let a = *scratch.get(n / 2 - 1)?;
    let b = *scratch.get(n / 2)?;
    Some((a + b) / 2.0)
}
