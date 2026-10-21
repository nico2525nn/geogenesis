//! Deterministic pseudo-random numbers and hashing.
//!
//! The engine must reproduce a world exactly from its seed, so randomness is
//! never drawn from the OS and never depends on iteration order across
//! threads. [`Rng`] is a SplitMix64 stream; [`hash2`]/[`hash3`] are stateless
//! hashes for fields (noise lattices, per-cell jitter, feature placement).
//!
//! Domain separation: every subsystem derives its stream from the world seed
//! with its own tag (`derive_seed(seed, TAG)`), so adding a new consumer of
//! randomness cannot change existing worlds.

use core::fmt;

/// SplitMix64: tiny, fast, good statistical quality for simulation.
///
/// The generator is not cryptographic and must not be used for secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Creates a generator from a seed. The same seed always yields the same
    /// sequence.
    #[inline]
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Current internal state, useful for debugging and forking.
    #[inline]
    #[must_use]
    pub const fn state(&self) -> u64 {
        self.state
    }

    /// Next 64 random bits.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next 32 random bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform `f64` in `[0, 1)` with 53 bits of precision.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        // Top 53 bits scaled by 2^-53 gives exactly [0, 1).
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Uniform `f32` in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform `f64` in `[lo, hi)`.
    #[inline]
    pub fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        debug_assert!(lo <= hi);
        lo + (hi - lo) * self.next_f64()
    }

    /// Uniform `i64` in `[lo, hi)`.
    #[inline]
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        debug_assert!(lo < hi);
        let span = (hi - lo) as u64;
        lo + (self.next_u64() % span) as i64
    }

    /// Uniform `u32` in `[lo, hi)`.
    #[inline]
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        debug_assert!(lo < hi);
        lo + (self.next_u64() % u64::from(hi - lo)) as u32
    }

    /// Uniform `usize` in `[0, n)`; `n` must be positive.
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (self.next_u64() % (n as u64)) as usize
    }

    /// True with probability `p`. Values outside `[0, 1]` are clamped.
    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p.clamp(0.0, 1.0)
    }

    /// Uniformly random sign, `-1.0` or `1.0`.
    #[inline]
    pub fn sign(&mut self) -> f64 {
        if self.next_u64() & 1 == 0 {
            -1.0
        } else {
            1.0
        }
    }

    /// Standard normal variate (Box–Muller), mean 0, variance 1.
    pub fn normal(&mut self) -> f64 {
        // u1 in (0, 1] to keep ln() finite.
        let u1 = 1.0 - self.next_f64();
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Normal variate with the given mean and standard deviation.
    #[inline]
    pub fn normal_with(&mut self, mean: f64, std_dev: f64) -> f64 {
        mean + std_dev * self.normal()
    }

    /// A point in the unit disk, returned as `(x, y)`.
    pub fn unit_disk(&mut self) -> (f64, f64) {
        loop {
            let x = self.range_f64(-1.0, 1.0);
            let y = self.range_f64(-1.0, 1.0);
            let r2 = x * x + y * y;
            if r2 <= 1.0 && r2 > 1e-24 {
                return (x, y);
            }
        }
    }

    /// Derives an independent child stream; the parent state advances, so
    /// repeated forks give different children.
    #[inline]
    pub fn fork(&mut self, tag: u64) -> Self {
        Self::new(derive_seed(self.next_u64(), tag))
    }

    /// Fisher–Yates shuffle, deterministic for a given generator state.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }

    /// Picks a random element, or `None` when empty.
    pub fn choice<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.below(items.len())])
        }
    }
}

impl fmt::Display for Rng {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rng({:016x})", self.state)
    }
}

/// Avalanche mix for a single word (SplitMix64 finaliser).
#[inline]
#[must_use]
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    z = (z ^ (z >> 33)).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    z ^ (z >> 33)
}

/// Stateless 2D hash. Stable across runs and platforms.
#[inline]
#[must_use]
pub fn hash2(x: u64, y: u64, seed: u64) -> u64 {
    let h = seed
        .wrapping_add(x.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    mix64(h)
}

/// Stateless 3D hash. Stable across runs and platforms.
#[inline]
#[must_use]
pub fn hash3(x: u64, y: u64, z: u64, seed: u64) -> u64 {
    let h = seed
        .wrapping_add(x.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(z.wrapping_mul(0x1656_67B1_9E37_79F9));
    mix64(h)
}

/// Uniform `f64` in `[0, 1)` from a hash.
#[inline]
#[must_use]
pub fn unit_from_hash(h: u64) -> f64 {
    (h >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

/// Uniform `f64` in `[lo, hi)` from a hash.
#[inline]
#[must_use]
pub fn range_from_hash(h: u64, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * unit_from_hash(h)
}

/// Signed uniform `f64` in `[-1, 1)` from a hash.
#[inline]
#[must_use]
pub fn signed_from_hash(h: u64) -> f64 {
    unit_from_hash(h) * 2.0 - 1.0
}

/// Derives a stream seed for a named domain: `derive_seed(world_seed, TAG)`.
///
/// Subsystems define their own tag constant; tags do not need to be globally
/// unique in a meaningful ordering, only distinct per stream.
#[inline]
#[must_use]
pub fn derive_seed(seed: u64, tag: u64) -> u64 {
    hash2(seed, tag, 0x6765_6F67_656E_6573)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact sequence for the canonical seed, so a future refactor cannot
    /// silently change every generated world.
    #[test]
    fn splitmix64_sequence_is_stable() {
        let mut r = Rng::new(42);
        assert_eq!(r.next_u64(), 13679457532755275413_u64);
        assert_eq!(r.next_u64(), 2949826092126892291_u64);
        assert_eq!(r.next_u64(), 5139283748462763858_u64);
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn unit_interval() {
        let mut r = Rng::new(1);
        let mut sum = 0.0;
        let n = 100_000;
        for _ in 0..n {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
            sum += v;
        }
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.005, "mean {mean}");
    }

    #[test]
    fn uniform_ranges_are_respected() {
        let mut r = Rng::new(3);
        for _ in 0..10_000 {
            let v = r.range_f64(-2.0, 5.0);
            assert!((-2.0..5.0).contains(&v));
            let i = r.range_i64(-3, 4);
            assert!((-3..4).contains(&i));
            let u = r.range_u32(10, 20);
            assert!((10..20).contains(&u));
        }
    }

    #[test]
    fn normal_has_expected_moments() {
        let mut r = Rng::new(99);
        let n = 200_000;
        let (mut sum, mut sum2) = (0.0, 0.0);
        for _ in 0..n {
            let v = r.normal();
            sum += v;
            sum2 += v * v;
        }
        let mean = sum / n as f64;
        let var = sum2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 1.0).abs() < 0.05, "var {var}");
    }

    #[test]
    fn hash_is_stable_and_domain_separated() {
        assert_eq!(hash2(1, 2, 3), hash2(1, 2, 3));
        assert_ne!(hash2(1, 2, 3), hash2(2, 1, 3));
        assert_ne!(hash2(1, 2, 3), hash2(1, 2, 4));
        assert_ne!(derive_seed(5, 1), derive_seed(5, 2));
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Rng::new(11);
        let mut items: Vec<u32> = (0..64).collect();
        r.shuffle(&mut items);
        items.sort_unstable();
        assert_eq!(items, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn fork_children_differ() {
        let mut r = Rng::new(12);
        let mut a = r.fork(1);
        let mut b = r.fork(1);
        assert_ne!(a.next_u64(), b.next_u64());
    }
}
