use rand::Rng;
use rand_xoshiro::Xoshiro256StarStar;
use rand::SeedableRng;
use serde::{Deserialize, Serialize};

/// ノイズタイプ
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum NoiseType {
    Perlin,
    Simplex,
    Value,
    Wavelet,
    GaussianSpike,
}

/// ノイズ生成パラメータ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseConfig {
    pub noise_type: NoiseType,
    pub octaves: u32,
    pub lacunarity: f64,
    pub persistence: f64,
    pub anisotropy: f64,
    pub seed: u64,
}

impl Default for NoiseConfig {
    fn default() -> Self {
        Self {
            noise_type: NoiseType::Perlin,
            octaves: 6,
            lacunarity: 2.0,
            persistence: 0.5,
            anisotropy: 0.0,
            seed: 42,
        }
    }
}

/// ノイズ生成エンジン
pub struct NoiseGenerator {
    rng: Xoshiro256StarStar,
    perm: [usize; 512],
}

impl NoiseGenerator {
    pub fn new(seed: u64) -> Self {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let mut perm = [0usize; 512];
        let mut p = [0usize; 256];
        for i in 0..256 {
            p[i] = i;
        }
        for i in (0..256).rev() {
            let j = rng.gen_range(0..=i);
            p.swap(i, j);
        }
        for i in 0..512 {
            perm[i] = p[i & 255];
        }
        Self { rng, perm }
    }

    fn fade(t: f64) -> f64 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    fn lerp(a: f64, b: f64, t: f64) -> f64 {
        a + t * (b - a)
    }

    fn grad(hash: usize, x: f64, y: f64) -> f64 {
        let h = hash & 3;
        let u = if h & 1 == 0 { x } else { -x };
        let v = if h & 2 == 0 { y } else { -y };
        u + v
    }

    /// 2次元 Perlin ノイズ [-1, 1]
    pub fn perlin_2d(&self, x: f64, y: f64) -> f64 {
        let xi = x.floor() as i64;
        let yi = y.floor() as i64;
        let xf = x - xi as f64;
        let yf = y - yi as f64;
        let u = Self::fade(xf);
        let v = Self::fade(yf);

        let aa = self.perm[(self.perm[(xi & 255) as usize] + (yi & 255) as usize) & 511];
        let ab = self.perm[(self.perm[(xi & 255) as usize] + ((yi + 1) & 255) as usize) & 511];
        let ba = self.perm[(self.perm[((xi + 1) & 255) as usize] + (yi & 255) as usize) & 511];
        let bb = self.perm[(self.perm[((xi + 1) & 255) as usize] + ((yi + 1) & 255) as usize) & 511];

        let x1 = Self::lerp(Self::grad(aa, xf, yf), Self::grad(ba, xf - 1.0, yf), u);
        let x2 = Self::lerp(Self::grad(ab, xf, yf - 1.0), Self::grad(bb, xf - 1.0, yf - 1.0), u);
        Self::lerp(x1, x2, v)
    }

    /// 2次元 Simplex ノイズ [-1, 1]
    pub fn simplex_2d(&self, x: f64, y: f64) -> f64 {
        let s = (x + y) * 0.3660254037844386;
        let i = (x + s).floor();
        let j = (y + s).floor();
        let t = (i + j) * 0.21132486540518713;
        let x0 = x - (i - t);
        let y0 = y - (j - t);

        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - i1 as f64 + 0.21132486540518713;
        let y1 = y0 - j1 as f64 + 0.21132486540518713;
        let x2 = x0 - 1.0 + 2.0 * 0.21132486540518713;
        let y2 = y0 - 1.0 + 2.0 * 0.21132486540518713;

        let ii = (i as i64) & 255;
        let jj = (j as i64) & 255;

        let idx0 = (ii as usize + self.perm[jj as usize]) & 511;
        let idx1 = (ii as usize + i1 as usize + self.perm[(jj + j1) as usize]) & 511;
        let idx2 = (ii as usize + 1 + self.perm[(jj + 1) as usize]) & 511;

        let n0 = self.simplex_corner(x0, y0, self.perm[idx0]);
        let n1 = self.simplex_corner(x1, y1, self.perm[idx1]);
        let n2 = self.simplex_corner(x2, y2, self.perm[idx2]);

        n0 + n1 + n2
    }

    fn simplex_corner(&self, x: f64, y: f64, hash: usize) -> f64 {
        let t = 0.5 - x * x - y * y;
        if t < 0.0 {
            0.0
        } else {
            let t2 = t * t;
            t2 * t2 * Self::grad(hash, x, y)
        }
    }

    /// バリューノイズ [-1, 1]
    pub fn value_2d(&self, x: f64, y: f64) -> f64 {
        let xi = x.floor() as i64;
        let yi = y.floor() as i64;
        let xf = x - xi as f64;
        let yf = y - yi as f64;
        let u = Self::fade(xf);
        let v = Self::fade(yf);

        let n00 = self.hash_f64(xi, yi);
        let n10 = self.hash_f64(xi + 1, yi);
        let n01 = self.hash_f64(xi, yi + 1);
        let n11 = self.hash_f64(xi + 1, yi + 1);

        Self::lerp(Self::lerp(n00, n10, u), Self::lerp(n01, n11, u), v) * 2.0 - 1.0
    }

    fn hash_f64(&self, x: i64, y: i64) -> f64 {
        let h = self.perm[(x & 255) as usize] ^ (y & 255) as usize;
        let h = (h.wrapping_mul(0x9E3779B9)) & 0xFF;
        h as f64 / 255.0
    }

    /// フラクタルノイズ（複数オクターブ合成）
    pub fn fractal_2d(&self, x: f64, y: f64, config: &NoiseConfig) -> f64 {
        let mut value = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut max_amplitude = 0.0;

        for _ in 0..config.octaves {
            let nx = x * frequency;
            let ny = y * frequency;

            let n = if config.anisotropy > 0.0 {
                let ax = nx;
                let ay = ny * (1.0 + config.anisotropy);
                self.sample_2d(ax, ay, config.noise_type)
            } else {
                self.sample_2d(nx, ny, config.noise_type)
            };

            value += n * amplitude;
            max_amplitude += amplitude;
            amplitude *= config.persistence;
            frequency *= config.lacunarity;
        }

        value / max_amplitude
    }

    fn sample_2d(&self, x: f64, y: f64, noise_type: NoiseType) -> f64 {
        match noise_type {
            NoiseType::Perlin => self.perlin_2d(x, y),
            NoiseType::Simplex => self.simplex_2d(x, y),
            NoiseType::Value => self.value_2d(x, y),
            NoiseType::Wavelet => self.wavelet_2d(x, y),
            NoiseType::GaussianSpike => self.gaussian_spike_2d(x, y),
        }
    }

    fn wavelet_2d(&self, x: f64, y: f64) -> f64 {
        let xi = x.floor() as i64;
        let yi = y.floor() as i64;
        let xf = x - xi as f64;
        let yf = y - yi as f64;

        let mut sum = 0.0;
        for dx in -2..=2 {
            for dy in -2..=2 {
                let w = self.wavelet_weight(xf - dx as f64) * self.wavelet_weight(yf - dy as f64);
                let h = self.hash_f64(xi + dx, yi + dy);
                sum += w * h;
            }
        }
        sum * 2.0 - 1.0
    }

    fn wavelet_weight(&self, t: f64) -> f64 {
        let t = t.abs();
        if t < 1.0 {
            1.0 - (3.0 * t * t) + (2.0 * t * t * t)
        } else if t < 2.0 {
            let t2 = t - 1.0;
            (1.0 - t2) * (1.0 - t2) * (1.0 - t2)
        } else {
            0.0
        }
    }

    fn gaussian_spike_2d(&self, x: f64, y: f64) -> f64 {
        let sigma = 0.3;
        let r2 = x * x + y * y;
        (-r2 / (2.0 * sigma * sigma)).exp() * 2.0 - 1.0
    }
}
