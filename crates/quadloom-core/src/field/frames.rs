//! Per-face tangent frames, complex numbers for power fields, and the dual
//! edges (face pairs) the smoothness energy runs over.

use hashbrown::HashMap;
use std::ops::{Add, Mul, Sub};

use glam::DVec3;

/// A complex number, enough for power fields (no external complex type).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cx {
    pub re: f64,
    pub im: f64,
}

impl Cx {
    pub const ZERO: Cx = Cx { re: 0.0, im: 0.0 };
    pub const ONE: Cx = Cx { re: 1.0, im: 0.0 };

    /// e^{iθ}.
    pub fn from_angle(theta: f64) -> Self {
        Self { re: theta.cos(), im: theta.sin() }
    }

    pub fn conj(self) -> Self {
        Self { re: self.re, im: -self.im }
    }

    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    pub fn scale(self, s: f64) -> Self {
        Self { re: self.re * s, im: self.im * s }
    }
}

impl Add for Cx {
    type Output = Cx;
    fn add(self, o: Cx) -> Cx {
        Cx { re: self.re + o.re, im: self.im + o.im }
    }
}

impl Sub for Cx {
    type Output = Cx;
    fn sub(self, o: Cx) -> Cx {
        Cx { re: self.re - o.re, im: self.im - o.im }
    }
}

impl Mul for Cx {
    type Output = Cx;
    fn mul(self, o: Cx) -> Cx {
        Cx { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

/// Each face's tangent frame: `e1` along its first edge, `e2 = normal × e1`.
/// A degenerate face (zero area) has all three zero.
#[derive(Clone, Debug)]
pub struct FaceFrames {
    pub e1: Vec<DVec3>,
    pub e2: Vec<DVec3>,
    pub normal: Vec<DVec3>,
}

impl FaceFrames {
    pub fn new(positions: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        let (mut e1, mut e2, mut normal) = (Vec::new(), Vec::new(), Vec::new());
        for t in triangles {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            let x = if n == DVec3::ZERO { DVec3::ZERO } else { (b - a).normalize_or_zero() };
            e1.push(x);
            e2.push(n.cross(x));
            normal.push(n);
        }
        Self { e1, e2, normal }
    }

    pub fn is_degenerate(&self, f: u32) -> bool {
        self.normal[f as usize] == DVec3::ZERO
    }

    /// Angle of tangent vector `d` in face `f`'s frame.
    pub fn angle(&self, f: u32, d: DVec3) -> f64 {
        d.dot(self.e2[f as usize]).atan2(d.dot(self.e1[f as usize]))
    }

    /// The unit tangent vector at angle `theta` in face `f`'s frame.
    pub fn direction(&self, f: u32, theta: f64) -> DVec3 {
        self.e1[f as usize] * theta.cos() + self.e2[f as usize] * theta.sin()
    }

    /// The power-`n` representation of tangent direction `d` on face `f`.
    pub fn power(&self, f: u32, d: DVec3, n: f64) -> Cx {
        Cx::from_angle(n * self.angle(f, d))
    }
}

/// An interior edge seen from its two faces: `a → b` is the edge as `f`
/// traverses it (so `g` traverses `b → a`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DualEdge {
    pub f: u32,
    pub g: u32,
    pub a: u32,
    pub b: u32,
}

/// Every interior edge shared by exactly two faces, in a deterministic order
/// (by the face that completes the pair, then its corner). Edges of more than two faces (not
/// expected after repair) are skipped.
pub fn dual_edges(triangles: &[[u32; 3]]) -> Vec<DualEdge> {
    let mut first: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::new();
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edges = Vec::new();
    for (f, t) in triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            let key = (a.min(b), a.max(b));
            *count.entry(key).or_default() += 1;
            match first.get(&key) {
                None => {
                    first.insert(key, (f as u32, a, b));
                }
                Some(&(g, ga, gb)) => edges.push((key, DualEdge { f: g, g: f as u32, a: ga, b: gb })),
            }
        }
    }
    edges.into_iter().filter(|(key, _)| count.get(key) == Some(&2)).map(|(_, e)| e).collect()
}
