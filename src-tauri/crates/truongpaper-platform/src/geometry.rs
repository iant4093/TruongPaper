//! OS-independent rectangle math shared by cursor hit-testing (`cursor.rs`) and Smart Pause
//! (`policy.rs`). Kept free of Win32/AppKit types so it can be unit tested headlessly.

/// Axis-aligned rectangle in a top-left-origin coordinate space (Win32 virtual screen, Quartz
/// global display space, or Cocoa space where noted by the caller).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Builds a rect from Win32-style `left, top, right, bottom` edges.
    pub fn from_ltrb(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self::new(left, top, right - left, bottom - top)
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// Area clamped at zero for degenerate rects.
    pub fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    /// Area of the overlap with `other` (0 when disjoint).
    pub fn intersection_area(&self, other: &ScreenRect) -> f64 {
        let ix0 = self.x.max(other.x);
        let iy0 = self.y.max(other.y);
        let ix1 = self.right().min(other.right());
        let iy1 = self.bottom().min(other.bottom());
        if ix1 > ix0 && iy1 > iy0 {
            (ix1 - ix0) * (iy1 - iy0)
        } else {
            0.0
        }
    }

    /// `true` when `self` fully covers `other` (edges may coincide).
    pub fn covers(&self, other: &ScreenRect) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    /// Half-open containment `[x, right) x [y, bottom)` used for monitor membership.
    pub fn contains_half_open(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// Closed containment `[x, right] x [y, bottom]` used for window-covers-point checks.
    pub fn contains_inclusive(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.right() && py >= self.y && py <= self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_and_cover() {
        let a = ScreenRect::new(0.0, 0.0, 100.0, 100.0);
        let b = ScreenRect::new(50.0, 50.0, 100.0, 100.0);
        assert_eq!(a.intersection_area(&b), 2500.0);
        assert_eq!(
            a.intersection_area(&ScreenRect::new(100.0, 0.0, 10.0, 10.0)),
            0.0
        );
        assert!(ScreenRect::new(-1.0, -1.0, 102.0, 102.0).covers(&a));
        assert!(a.covers(&a));
        assert!(!b.covers(&a));
        assert_eq!(
            ScreenRect::from_ltrb(10.0, 20.0, 30.0, 60.0),
            ScreenRect::new(10.0, 20.0, 20.0, 40.0)
        );
        assert_eq!(ScreenRect::new(0.0, 0.0, -5.0, 10.0).area(), 0.0);
    }

    #[test]
    fn containment_edges() {
        let r = ScreenRect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains_half_open(0.0, 0.0));
        assert!(!r.contains_half_open(10.0, 5.0));
        assert!(r.contains_inclusive(10.0, 10.0));
        assert!(!r.contains_inclusive(10.1, 5.0));
    }
}
