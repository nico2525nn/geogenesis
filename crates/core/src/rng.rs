//! Deterministic pseudo-random number generation.
//!
//! Same seed yields the same stream on every platform; sub-seeds are SplitMix64-decorrelated.

/// Mixes a `u64` into a well-distributed `u64` (SplitMix64 finalizer).
#[inline]
pub const fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Derives an independent sub-seed from a parent seed and a domain label.
/// New domain constants must not perturb existing streams.
#[inline]
pub const fn derive_seed(parent: u64, domain: u64) -> u64 {
    mix64(parent.wrapping_add(domain.wrapping_mul(0x9e37_79b9_7f4a_7c15)))
}

/// xoshiro256++ generator; exact stream is pinned by this file.
#[derive(Clone)]
pub struct Rng {
    state: [u64; 4],
}

impl core::fmt::Debug for Rng {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // State is 256 bits of noise; not useful to print.
        f.write_str("Rng { .. }")
    }
}

impl Rng {
    /// Creates a generator for the given domain of `parent`.
    pub fn new(parent: u64, domain: u64) -> Self {
        Self::from_seed(derive_seed(parent, domain))
    }

    /// Creates a generator from an already-derived seed.
    pub fn from_seed(seed: u64) -> Self {
        // SplitMix64 expansion: a zero seed still yields a non-degenerate state.
        let mut z = seed;
        let mut state = [0u64; 4];
        let mut i = 0;
        while i < 4 {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            state[i] = mix64(z);
            i += 1;
        }
        // xoshiro requires nonzero state; mix64(0) != 0 makes this unreachable.
        if state == [0; 4] {
            state[0] = 0x9e37_79b9_7f4a_7c15;
        }
        Self { state }
    }

    /// Returns the next raw 64-bit value.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = s[0]
            .wrapping_add(s[3])
            .rotate_left(23)
            .wrapping_add(s[0]);
        let t = s[1] << 17;

        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);

        result
    }

    /// Returns a uniformly distributed `u32`.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        // Low bits of xoshiro++ are weakest; shift to the high bits.
        (self.next_u64() >> 32) as u32
    }

    /// Returns a uniform value in `[0, 1)`.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Returns a uniform value in `[low, high)`; returns `low` for an empty or inverted range.
    #[inline]
    pub fn range_f64(&mut self, low: f64, high: f64) -> f64 {
        if !(high > low) {
            return low;
        }
        low + (high - low) * self.next_f64()
    }

    /// Returns a uniform integer in `[0, n)` via debiased modulo.
    #[inline]
    pub fn below_u32(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let v = self.next_u32();
            if v >= threshold {
                return v % n;
            }
        }
    }

    /// Returns a uniform direction in `[0, 2π)` (radians).
    #[inline]
    pub fn next_angle(&mut self) -> f64 {
        self.next_f64() * core::f64::consts::TAU
    }

    /// Standard normal deviate via the Box–Muller transform.
    #[inline]
    pub fn next_normal(&mut self) -> f64 {
        // Guard `ln(0)` by clamping away from zero.
        let u1 = (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let u1 = if u1 <= f64::MIN_POSITIVE { f64::MIN_POSITIVE } else { u1 };
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (core::f64::consts::TAU * u2).cos()
    }

    /// Samples a unit vector uniformly on the unit circle.
    #[inline]
    pub fn next_unit_vector(&mut self) -> (f64, f64) {
        let a = self.next_angle();
        (a.cos(), a.sin())
    }

    /// Fisher–Yates shuffle in place.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below_u32((i + 1) as u32) as usize;
            items.swap(i, j);
        }
    }

    /// Picks one element uniformly; returns `None` for an empty slice.
    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        items.get(self.below_u32(items.len() as u32) as usize)
    }
}

/// Deterministic domain-separated hash of a coordinate pair to a `u64`.
#[inline]
pub const fn hash2(x: i64, y: i64, seed: u64) -> u64 {
    let a = mix64((x as u64) ^ seed);
    mix64(a ^ (y as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// Deterministic hash of a coordinate triple to a `u64`.
#[inline]
pub const fn hash3(x: i64, y: i64, z: i64, seed: u64) -> u64 {
    mix64(hash2(x, y, seed) ^ mix64((z as u64).wrapping_add(0x517c_c1b7_2722_0a95)))
}

/// Hash to a uniform value in `[0, 1)`.
#[inline]
pub fn hash2_f64(x: i64, y: i64, seed: u64) -> f64 {
    (hash2(x, y, seed) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Domain labels; never reuse a value.
pub mod domain {
    pub const PLATES: u64 = 0x01;
    pub const HOTSPOTS: u64 = 0x02;
    pub const FEATURES: u64 = 0x03;
    pub const STRATA: u64 = 0x04;
    pub const NOISE_PERLIN: u64 = 0x10;
    pub const NOISE_SIMPLEX: u64 = 0x11;
    pub const NOISE_VALUE: u64 = 0x12;
    pub const NOISE_WAVELET: u64 = 0x13;
    pub const CLIMATE: u64 = 0x20;
    pub const RAINFALL: u64 = 0x21;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix64_avalanches_adjacent_inputs() {
        // Neighbouring worlds must not look like shifted copies.
        let a = mix64(1);
        let b = mix64(2);
        assert_ne!(a, b);
        assert!(a ^ b > (1 << 32), "outputs differ by more than a bit flip");
    }

    #[test]
    fn stream_is_reproducible() {
        let mut a = Rng::from_seed(12345);
        let mut b = Rng::from_seed(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(2);
        let differing = (0..64).filter(|_| a.next_u64() != b.next_u64()).count();
        assert_eq!(differing, 64, "streams from adjacent seeds overlap");
    }

    #[test]
    fn next_f64_in_unit_interval() {
        let mut rng = Rng::from_seed(7);
        for _ in 0..10_000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "{v} out of range");
        }
    }

    #[test]
    fn next_f64_is_roughly_uniform() {
        // Deciles must land within a few percent of uniform.
        let mut rng = Rng::from_seed(99);
        let mut buckets = [0u32; 10];
        const N: u32 = 200_000;
        for _ in 0..N {
            let b = (rng.next_f64() * 10.0) as usize;
            buckets[b.min(9)] += 1;
        }
        for (i, &count) in buckets.iter().enumerate() {
            let expected = N as f64 / 10.0;
            let err = (count as f64 - expected).abs() / expected;
            assert!(err < 0.03, "decile {i} off by {err:.3}");
        }
    }

    #[test]
    fn range_is_inclusive_low_exclusive_high() {
        let mut rng = Rng::from_seed(5);
        for _ in 0..1000 {
            let v = rng.range_f64(2.0, 3.0);
            assert!((2.0..3.0).contains(&v));
        }
    }

    #[test]
    fn degenerate_range_does_not_panic() {
        let mut rng = Rng::from_seed(5);
        assert_eq!(rng.range_f64(1.0, 1.0), 1.0);
        assert_eq!(rng.range_f64(2.0, 1.0), 2.0);
    }

    #[test]
    fn below_is_in_range_and_unbiased() {
        let mut rng = Rng::from_seed(3);
        let mut counts = [0u32; 7];
        const N: u32 = 70_000;
        for _ in 0..N {
            let v = rng.below_u32(7);
            assert!(v < 7);
            counts[v as usize] += 1;
        }
        for &c in &counts {
            let err = (c as f64 - N as f64 / 7.0).abs() / (N as f64 / 7.0);
            assert!(err < 0.05, "bucket skew {err:.3}");
        }
    }

    #[test]
    fn below_zero_is_zero() {
        let mut rng = Rng::from_seed(3);
        assert_eq!(rng.below_u32(0), 0);
    }

    #[test]
    fn normal_has_zero_mean_unit_variance() {
        let mut rng = Rng::from_seed(11);
        let n = 200_000;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..n {
            let v = rng.next_normal();
            sum += v;
            sum_sq += v * v;
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.03, "variance {var}");
    }

    #[test]
    fn unit_vectors_are_on_unit_circle() {
        let mut rng = Rng::from_seed(13);
        for _ in 0..1000 {
            let (x, y) = rng.next_unit_vector();
            let r = (x * x + y * y).sqrt();
            assert!((r - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn shuffle_is_a_permutation_and_deterministic() {
        let mut a: Vec<u32> = (0..64).collect();
        let mut b = a.clone();
        let mut ra = Rng::from_seed(17);
        let mut rb = Rng::from_seed(17);
        ra.shuffle(&mut a);
        rb.shuffle(&mut b);
        assert_eq!(a, b);
        a.sort_unstable();
        assert_eq!(a, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn hash_is_stable_and_spread() {
        assert_eq!(hash2(3, 7, 42), hash2(3, 7, 42));
        assert_ne!(hash2(3, 7, 42), hash2(3, 8, 42));
        let mut rng = Rng::from_seed(1);
        for _ in 0..1000 {
            let (x, y) = (
                rng.below_u32(1000) as i64,
                rng.below_u32(1000) as i64,
            );
            let v = hash2_f64(x, y, 42);
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn hash_uniformity_over_grid() {
        // Neighbouring lattice cells must not share a bucket.
        let mut buckets = [0u32; 8];
        for x in 0..200 {
            for y in 0..200 {
                let v = hash2_f64(x, y, domain::NOISE_VALUE);
                buckets[(v * 8.0) as usize % 8] += 1;
            }
        }
        for &c in &buckets {
            let err = (c as f64 - 40_000.0 / 8.0).abs() / (40_000.0 / 8.0);
            assert!(err < 0.05, "hash bucket skew {err:.3}");
        }
    }

    #[test]
    fn derived_seeds_are_independent() {
        let mut a = Rng::new(1, domain::PLATES);
        let mut b = Rng::new(1, domain::HOTSPOTS);
        let differing = (0..32).filter(|_| a.next_u64() != b.next_u64()).count();
        assert_eq!(differing, 32);
    }
}