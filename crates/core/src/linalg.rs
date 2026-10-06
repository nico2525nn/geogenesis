//! Small fixed-size vector and matrix helpers.
//!
//! Hand-written rather than pulled from a crate: the engine has no runtime
//! dependencies, and these are the only linear-algebra shapes it needs.

/// 2D vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn from_angle(angle: f64, length: f64) -> Self {
        Self {
            x: angle.cos() * length,
            y: angle.sin() * length,
        }
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// Z component of the 3D cross product; the signed area of the parallelogram.
    pub fn cross(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// Returns the unit vector, or `ZERO` for a zero-length input.
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len > f64::EPSILON {
            Self {
                x: self.x / len,
                y: self.y / len,
            }
        } else {
            Self::ZERO
        }
    }

    pub fn scaled(self, factor: f64) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
        }
    }

    pub fn rotated(self, angle: f64) -> Self {
        let (s, c) = (angle.sin(), angle.cos());
        Self {
            x: self.x * c - self.y * s,
            y: self.x * s + self.y * c,
        }
    }

    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }
}

impl core::ops::Add for Vec2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl core::ops::Sub for Vec2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl core::ops::Mul<f64> for Vec2 {
    type Output = Self;
    fn mul(self, k: f64) -> Self {
        Self::new(self.x * k, self.y * k)
    }
}

impl core::ops::Neg for Vec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

/// 3D vector. Used for plate kinematics in full 3D and for 3D surface normals.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    pub fn normalized(self) -> Self {
        let len = self.length();
        if len > f64::EPSILON {
            self.scaled(1.0 / len)
        } else {
            Self::ZERO
        }
    }

    pub fn scaled(self, k: f64) -> Self {
        Self::new(self.x * k, self.y * k, self.z * k)
    }

    /// Removes the component parallel to `axis`, which must be unit length.
    pub fn reject(self, axis: Self) -> Self {
        self - axis.scaled(self.dot(axis))
    }
}

impl core::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl core::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl core::ops::Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, k: f64) -> Self {
        self.scaled(k)
    }
}

impl core::ops::Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// 3×3 matrix in row-major order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    pub m: [f64; 9],
}

impl Mat3 {
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    pub const fn from_rows(m: [f64; 9]) -> Self {
        Self { m }
    }

    pub fn multiply_vec3(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0] * v.x + self.m[1] * v.y + self.m[2] * v.z,
            self.m[3] * v.x + self.m[4] * v.y + self.m[5] * v.z,
            self.m[6] * v.x + self.m[7] * v.y + self.m[8] * v.z,
        )
    }

    pub fn transpose(&self) -> Self {
        Self::from_rows([
            self.m[0], self.m[3], self.m[6], //
            self.m[1], self.m[4], self.m[7], //
            self.m[2], self.m[5], self.m[8],
        ])
    }

    /// Rotation about the z axis.
    pub fn rotation_z(angle: f64) -> Self {
        let (s, c) = (angle.sin(), angle.cos());
        Self::from_rows([c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0])
    }

    /// Rotation about an arbitrary unit axis (Rodrigues' formula).
    pub fn rotation_about_axis(axis: Vec3, angle: f64) -> Self {
        let a = axis.normalized();
        let (s, c) = (angle.sin(), angle.cos());
        let t = 1.0 - c;
        Self::from_rows([
            t * a.x * a.x + c,
            t * a.x * a.y - s * a.z,
            t * a.x * a.z + s * a.y,
            t * a.x * a.y + s * a.z,
            t * a.y * a.y + c,
            t * a.y * a.z - s * a.x,
            t * a.x * a.z - s * a.y,
            t * a.y * a.z + s * a.x,
            t * a.z * a.z + c,
        ])
    }

    /// Returns the inverse, or `None` when the matrix is singular.
    pub fn inverse(&self) -> Option<Self> {
        let d = self.determinant();
        if d.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / d;
        let m = &self.m;
        Some(Self::from_rows([
            (m[4] * m[8] - m[5] * m[7]) * inv,
            (m[2] * m[7] - m[1] * m[8]) * inv,
            (m[1] * m[5] - m[2] * m[4]) * inv,
            (m[5] * m[6] - m[3] * m[8]) * inv,
            (m[0] * m[8] - m[2] * m[6]) * inv,
            (m[2] * m[3] - m[0] * m[5]) * inv,
            (m[3] * m[7] - m[4] * m[6]) * inv,
            (m[1] * m[6] - m[0] * m[7]) * inv,
            (m[0] * m[4] - m[1] * m[3]) * inv,
        ]))
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.m;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6])
    }
}

/// Clamps `v` into `[lo, hi]`, tolerating an inverted range.
#[inline]
pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if lo > hi {
        return lo;
    }
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Smooth Hermite interpolation between `edge0` and `edge1`.
#[inline]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if (edge1 - edge0).abs() < f64::EPSILON {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth, zero-derivative-at-both-ends window over `[-1, 1]`.
///
/// Used for feature falloff (a range crest must not crease where it ends).
#[inline]
pub fn smooth_window(x: f64) -> f64 {
    let t = clamp(x.abs(), 0.0, 1.0);
    let s = 1.0 - t * t;
    s * s
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// The smaller of two values, propagating NaN like `f64::min`.
#[inline]
pub fn fmin(a: f64, b: f64) -> f64 {
    if a < b { a } else { b }
}

/// The greater of two values, propagating NaN like `f64::max`.
#[inline]
pub fn fmax(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn vec2_rotation_preserves_length() {
        let v = Vec2::new(3.0, 4.0);
        let r = v.rotated(PI / 3.0);
        assert!((r.length() - 5.0).abs() < 1e-12);
    }

    #[test]
    fn vec2_rotation_by_90_degrees() {
        let r = Vec2::new(1.0, 0.0).rotated(PI / 2.0);
        assert!(r.x.abs() < 1e-12);
        assert!((r.y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn vec2_zero_normalizes_to_zero() {
        assert_eq!(Vec2::ZERO.normalized(), Vec2::ZERO);
    }

    #[test]
    fn vec2_cross_sign_follows_orientation() {
        let x = Vec2::new(1.0, 0.0);
        let y = Vec2::new(0.0, 1.0);
        assert!(x.cross(y) > 0.0);
        assert!(y.cross(x) < 0.0);
    }

    #[test]
    fn vec3_cross_is_orthogonal() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(-4.0, 0.5, 2.0);
        let c = a.cross(b);
        assert!(c.dot(a).abs() < 1e-12);
        assert!(c.dot(b).abs() < 1e-12);
    }

    #[test]
    fn mat3_rotation_preserves_length() {
        let m = Mat3::rotation_about_axis(Vec3::new(0.3, -0.7, 0.2), 1.1);
        let v = Vec3::new(1.0, -2.0, 0.5);
        let r = m.multiply_vec3(v);
        assert!((r.length() - v.length()).abs() < 1e-12);
    }

    #[test]
    fn mat3_rotation_preserves_dot_product() {
        let m = Mat3::rotation_z(0.7);
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(-0.5, 4.0, 1.0);
        let (ra, rb) = (m.multiply_vec3(a), m.multiply_vec3(b));
        assert!((ra.dot(rb) - a.dot(b)).abs() < 1e-12);
    }

    #[test]
    fn mat3_inverse_round_trips() {
        let m = Mat3::rotation_about_axis(Vec3::new(1.0, 2.0, 3.0), 0.9);
        let inv = m.inverse().expect("rotation is invertible");
        let p = Vec3::new(0.7, -1.2, 2.5);
        let round = inv.multiply_vec3(m.multiply_vec3(p));
        assert!((round - p).length() < 1e-12);
    }

    #[test]
    fn mat3_singular_has_no_inverse() {
        let m = Mat3::from_rows([1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 1.0, 1.0, 1.0]);
        assert!(m.determinant().abs() < 1e-12);
        assert!(m.inverse().is_none());
    }

    #[test]
    fn transpose_is_an_involution() {
        let m = Mat3::rotation_about_axis(Vec3::new(0.1, 0.2, 0.3), 0.4);
        assert_eq!(m.transpose().transpose(), m);
    }

    #[test]
    fn clamp_handles_inverted_range() {
        // An inverted range collapses to `lo` rather than silently swapping the
        // bounds, so callers get a predictable (if degenerate) result.
        assert_eq!(clamp(5.0, 10.0, 0.0), 10.0);
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-20.0, -10.0, 10.0), -10.0);
        assert_eq!(clamp(20.0, -10.0, 10.0), 10.0);
    }

    #[test]
    fn smoothstep_endpoints_and_monotonicity() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = smoothstep(0.0, 1.0, i as f64 / 100.0);
            assert!(v >= prev, "smoothstep is not monotonic");
            prev = v;
        }
    }

    #[test]
    fn smoothstep_degenerate_range_is_a_step() {
        assert_eq!(smoothstep(1.0, 1.0, 0.0), 0.0);
        assert_eq!(smoothstep(1.0, 1.0, 2.0), 1.0);
    }

    #[test]
    fn smooth_window_peaks_at_zero_and_vanishes_at_edges() {
        assert_eq!(smooth_window(0.0), 1.0);
        assert_eq!(smooth_window(1.0), 0.0);
        assert_eq!(smooth_window(2.0), 0.0);
        let mid = smooth_window(0.5);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn lerp_endpoints() {
        assert_eq!(lerp(2.0, 5.0, 0.0), 2.0);
        assert_eq!(lerp(2.0, 5.0, 1.0), 5.0);
        assert_eq!(lerp(2.0, 5.0, 0.5), 3.5);
    }
}