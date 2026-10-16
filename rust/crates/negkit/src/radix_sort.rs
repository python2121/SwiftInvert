//! LSD radix sort for the order statistics behind every meter — port of `RadixSort.swift`.
//!
//! Sorting dominates analysis: `prepare` runs one sort per channel plus one each for the anchor and
//! textural meters, and `finalize`'s neutral axis sorts every band's chroma and each kept channel.
//! Comparison sorts pay n·log n branchy comparisons for that; a radix sort pays a fixed 4 (`f32`)
//! or 8 (`f64`) branch-free counting passes. Measured in the Swift app on the 615×410 preview grid
//! (252k cells/channel): `vDSP_vsort` 21.6 ms, `Array.sort()` 15.8 ms, radix 1.9 ms — which took
//! whole-image analysis 162 → 30 ms.
//!
//! **This cannot perturb NegPy parity.** A sort is a permutation into a canonical order, and every
//! consumer ([`crate::stats::percentile_of_sorted`], [`crate::stats::median`],
//! [`crate::stats::quantile`]) only reads the sorted array by index — so any correct sort yields
//! identical bytes downstream. Verified field-by-field on raw bit patterns, not tolerances
//! (`tests/stats.rs`, the `RadixSortTests` port).

/// Below this the counting passes lose to a comparison sort's cache behaviour (the 256-entry
/// histograms cost more than the data). The small band sets in the neutral axis land here.
pub const THRESHOLD: usize = 512;

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// A float type the radix sort knows how to key: `f32` (4 passes) and `f64` (8 passes).
///
/// # Order-preserving float ↔ unsigned keys
///
/// IEEE-754 is built so that POSITIVE floats compare correctly when their bit patterns are read
/// as unsigned integers. Two things break for the rest: the sign bit makes negatives look
/// enormous, and negatives run backwards among themselves (−2.0's pattern exceeds −1.0's).
/// Flipping every bit of a negative fixes both (reverses their order, clears the top bit);
/// setting the top bit of a positive lifts it above them all. The map is strictly monotonic and
/// a bijection, so `unkey` restores the exact original value.
///
/// Edge cases, for the record: −0.0 keys just below +0.0 rather than tying. That is still a
/// valid ascending order (they compare equal, so a comparison sort's own relative order is
/// unspecified too), the values round-trip exactly, and the arithmetic downstream is identical
/// either way. NaN is not a concern on the meters' data — the grid is `log10(clip(x, 1e-6, 1.0))`,
/// finite by construction — and unlike a comparison sort, which violates its strict-weak-ordering
/// contract on NaN, this places them deterministically at the ends.
pub trait RadixSortable: Copy + sealed::Sealed {
    /// The unsigned key type (`u32` / `u64`).
    type Key: RadixKey;
    /// Order-preserving map to the unsigned key.
    fn key(self) -> Self::Key;
    /// Exact inverse of [`RadixSortable::key`].
    fn unkey(key: Self::Key) -> Self;
    /// The comparison used by the small-array fallback (`total_cmp`: a total order, so even
    /// NaN cannot break the sort's contract).
    fn compare(a: &Self, b: &Self) -> core::cmp::Ordering;
    /// Widen to `f64` (lossless for both types) — what the order statistics interpolate in.
    fn to_f64(self) -> f64;
}

/// An unsigned integer the counting passes can digit-extract from.
pub trait RadixKey: Copy + Default {
    /// Number of 8-bit passes (`size_of::<Self>()`).
    const PASSES: u32;
    /// The 8-bit digit at bit offset `shift`.
    fn digit(self, shift: u32) -> usize;
}

impl RadixKey for u32 {
    const PASSES: u32 = 4;
    #[inline(always)]
    fn digit(self, shift: u32) -> usize {
        // Masked to 8 bits, so the result is provably < 256.
        ((self >> shift) & 255) as usize
    }
}

impl RadixKey for u64 {
    const PASSES: u32 = 8;
    #[inline(always)]
    fn digit(self, shift: u32) -> usize {
        // Masked to 8 bits, so the result is provably < 256.
        ((self >> shift) & 255) as usize
    }
}

impl RadixSortable for f32 {
    type Key = u32;
    #[inline(always)]
    fn key(self) -> u32 {
        let b = self.to_bits();
        if b & 0x8000_0000 != 0 {
            !b
        } else {
            b | 0x8000_0000
        }
    }
    #[inline(always)]
    fn unkey(u: u32) -> f32 {
        f32::from_bits(if u & 0x8000_0000 != 0 {
            u & 0x7fff_ffff
        } else {
            !u
        })
    }
    #[inline(always)]
    fn compare(a: &f32, b: &f32) -> core::cmp::Ordering {
        a.total_cmp(b)
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl RadixSortable for f64 {
    type Key = u64;
    #[inline(always)]
    fn key(self) -> u64 {
        let b = self.to_bits();
        if b & 0x8000_0000_0000_0000 != 0 {
            !b
        } else {
            b | 0x8000_0000_0000_0000
        }
    }
    #[inline(always)]
    fn unkey(u: u64) -> f64 {
        f64::from_bits(if u & 0x8000_0000_0000_0000 != 0 {
            u & 0x7fff_ffff_ffff_ffff
        } else {
            !u
        })
    }
    #[inline(always)]
    fn compare(a: &f64, b: &f64) -> core::cmp::Ordering {
        a.total_cmp(b)
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self
    }
}

/// Ascending sort of a copy. Identical output to a comparison sort's for the NaN-free data the
/// meters produce (and a valid total order even with NaN present). Below [`THRESHOLD`] elements
/// it falls back to the standard library's sort — the Swift `Array.sorted()` fallback.
pub fn sorted<T: RadixSortable>(data: &[T]) -> Vec<T> {
    let n = data.len();
    if n < THRESHOLD {
        let mut out = data.to_vec();
        out.sort_by(T::compare);
        return out;
    }

    let mut src: Vec<T::Key> = data.iter().map(|v| v.key()).collect();
    let mut dst: Vec<T::Key> = vec![T::Key::default(); n];
    for pass_index in 0..T::Key::PASSES {
        pass(&src, &mut dst, pass_index * 8);
        core::mem::swap(&mut src, &mut dst);
    }
    src.into_iter().map(T::unkey).collect()
}

/// One stable counting pass over an 8-bit digit: histogram → exclusive prefix sum → scatter.
/// Stability is what makes least-significant-first work: elements tied on this digit keep the
/// order the previous digit left them in, so after the last pass the array is fully sorted.
#[inline(always)]
fn pass<K: RadixKey>(from: &[K], to: &mut [K], shift: u32) {
    let mut slot = [0usize; 256];
    for v in from {
        // `digit` is masked to 8 bits, so the index is in range by construction.
        if let Some(s) = slot.get_mut(v.digit(shift)) {
            *s += 1;
        }
    }
    let mut running = 0usize;
    for s in slot.iter_mut() {
        let count = *s;
        *s = running;
        running += count;
    }
    for v in from {
        if let Some(s) = slot.get_mut(v.digit(shift)) {
            // Every slot cursor stays below `from.len() == to.len()` because the prefix sums
            // partition exactly that many elements; `get_mut` keeps the no-panic guarantee.
            if let Some(d) = to.get_mut(*s) {
                *d = *v;
            }
            *s += 1;
        }
    }
}
