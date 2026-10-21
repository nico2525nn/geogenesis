//! Minimal 2D vector used for planar kinematics and feature placement.

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A 2D vector / point in arbitrary units (the caller decides whether it is
/// metres, cells or lattice units).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    /// First component.
    pub x: f64,
    /// Second component.
    pub y: f64,
}

impl Vec2 {
    /// Constructs a vector.
    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The zero vector.
    #[inline]
    #[must_use]
    pub const fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    /// Unit vector at `angle_rad` measured from +x, counter-clockwise.
    #[inline]
    #[must_use]
    pub fn from_angle(angle_rad: f64) -> Self {
        Self::new(angle_rad.cos(), angle_rad.sin())
    }

    /// Dot product.
    #[inline]
    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// 2D cross product (z component of the 3D cross).
    #[inline]
    #[must_use]
    pub fn cross(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// Euclidean length.
    #[inline]
    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Squared length; avoids the square root in comparisons.
    #[inline]
    #[must_use]
    pub fn length_squared(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    /// Length-normalised vector; the zero vector maps to zero.
    #[inline]
    #[must_use]
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len > 1e-300 {
            self / len
        } else {
            Self::zero()
        }
    }

    /// Perpendicular vector, rotated 90° counter-clockwise.
    #[inline]
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Rotates by `angle_rad`.
    #[inline]
    #[must_use]
    pub fn rotated(self, angle_rad: f64) -> Self {
        let (s, c) = angle_rad.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// Component-wise multiplication.
    #[inline]
    #[must_use]
    pub fn hadamard(self, other: Self) -> Self {
        Self::new(self.x * other.x, self.y * other.y)
    }

    /// Clamps the length to at most `max_len`, keeping the direction.
    #[inline]
    #[must_use]
    pub fn clamped_length(self, max_len: f64) -> Self {
        let len = self.length();
        if len > max_len && len > 1e-300 {
            self * (max_len / len)
        } else {
            self
        }
    }

    /// Wraps both components into `[0, period)`.
    #[inline]
    #[must_use]
    pub fn wrapped(self, period: f64) -> Self {
        Self::new(self.x.rem_euclid(period), self.y.rem_euclid(period))
    }
}

impl Add for Vec2 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub for Vec2 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<f64> for Vec2 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f64) -> Self {
        Self::new(self.x / rhs, self.y / rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_geometry() {
        let a = Vec2::new(3.0, 4.0);
        assert_eq!(a.length(), 5.0);
        let b = Vec2::new(1.0, 0.0);
        let c = Vec2::new(0.0, 1.0);
        assert_eq!(b.dot(c), 0.0);
        assert_eq!(b.cross(c), 1.0);
        assert!((Vec2::from_angle(0.0) - b).length() < 1e-15);
        assert!((a.normalized().length() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn rotation_preserves_length() {
        let v = Vec2::new(2.5, -1.2);
        let r = v.rotated(0.7);
        assert!((r.length() - v.length()).abs() < 1e-12);
        assert!((r.dot(v) - v.length() * v.length() * 0.7f64.cos()).abs() < 1e-12);
    }

    #[test]
    fn clamp_length_keeps_direction() {
        let v = Vec2::new(10.0, 0.0).clamped_length(2.0);
        assert!((v.length() - 2.0).abs() < 1e-15);
        assert!(v.y.abs() < 1e-15);
    }

    #[test]
    fn wrap_uses_euclidean_modulo() {
        let v = Vec2::new(-1.0, 4.5).wrapped(3.0);
        assert!((v.x - 2.0).abs() < 1e-15);
        assert!((v.y - 1.5).abs() < 1e-15);
    }
}
