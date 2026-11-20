//! The animation primitives Spicy Lyrics is built from, ported exactly:
//! Fraktality's spr spring, the npm `cubic-spline` natural spline, CSS
//! cubic-bezier easing, and the timed tween Compose's `Animatable` runs.

use std::f32::consts::PI;

/// A damped spring, a port of spr.lua (MIT) as Spicy Lyrics uses it.
///
/// `frequency` is in hertz and becomes radians a second inside [`Self::step`];
/// `damping` is the ratio. `set_goal` keeps the velocity, so a retarget
/// carries its momentum. There is no settle snapping.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    goal: f32,
    position: f32,
    velocity: f32,
    frequency: f32,
    damping: f32,
}

impl Spring {
    pub fn new(start: f32, frequency: f32, damping: f32) -> Self {
        Self {
            goal: start,
            position: start,
            velocity: 0.0,
            frequency,
            damping,
        }
    }

    pub fn set_goal(&mut self, goal: f32) {
        self.goal = goal;
    }

    #[cfg(test)]
    fn position(&self) -> f32 {
        self.position
    }

    /// Whether the spring has come to rest at its goal, by spr's own test.
    #[cfg(test)]
    fn can_sleep(&self) -> bool {
        let offset = self.position - self.goal;
        self.velocity * self.velocity <= 1e-4 && offset * offset <= (1.0 / 3840.0_f32).powi(2)
    }

    /// Advances `dt` seconds and returns the new position.
    pub fn step(&mut self, dt: f32) -> f32 {
        const EPS: f32 = 1e-5;
        let d = self.damping;
        let f = self.frequency * 2.0 * PI;
        let g = self.goal;
        let mut p = self.position;
        let mut v = self.velocity;
        if d == 1.0 {
            let q = (-f * dt).exp();
            let w = dt * q;
            let c0 = q + w * f;
            let c2 = q - w * f;
            let c3 = w * f * f;
            let o = p - g;
            p = o * c0 + v * w + g;
            v = v * c2 - o * c3;
        } else if d < 1.0 {
            let q = (-d * f * dt).exp();
            let c = (1.0 - d * d).sqrt();
            let i = (dt * f * c).cos();
            let j = (dt * f * c).sin();
            let z = if c > EPS {
                j / c
            } else {
                let a = dt * f;
                a + ((a * a) * (c * c) * (c * c) / 20.0 - c * c) * (a * a * a) / 6.0
            };
            let y = if f * c > EPS {
                j / (f * c)
            } else {
                let b = f * c;
                dt + ((dt * dt) * (b * b) * (b * b) / 20.0 - b * b) * (dt * dt * dt) / 6.0
            };
            let o = p - g;
            p = (o * (i + z * d) + v * y) * q + g;
            v = (v * (i - z * d) - o * (z * f)) * q;
        } else {
            let c = (d * d - 1.0).sqrt();
            let r1 = -f * (d + c);
            let r2 = -f * (d - c);
            let ec1 = (r1 * dt).exp();
            let ec2 = (r2 * dt).exp();
            let o = p - g;
            let co2 = (v - o * r1) / (2.0 * f * c);
            let co1 = ec1 * (o - co2);
            p = co1 + co2 * ec2 + g;
            v = co1 * r1 + co2 * ec2 * r2;
        }
        self.position = p;
        self.velocity = v;
        p
    }
}

/// Natural cubic spline through (time, value) knots, a port of the npm
/// `cubic-spline` package. The input is clamped to the knots' range.
#[derive(Clone, Debug)]
pub struct Spline {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ks: Vec<f32>,
}

impl Spline {
    pub fn new(points: &[(f32, f32)]) -> Self {
        let mut sorted = points.to_vec();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let xs: Vec<f32> = sorted.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = sorted.iter().map(|p| p.1).collect();
        let ks = natural_coefficients(&xs, &ys);
        Self { xs, ys, ks }
    }

    pub fn at(&self, t: f32) -> f32 {
        let first = self.xs[0];
        let last = *self.xs.last().expect("a spline has knots");
        let t = t.clamp(first, last);
        let mut i = 1;
        while i < self.xs.len() - 1 && self.xs[i] < t {
            i += 1;
        }
        let s = i - 1;
        let dx = self.xs[s + 1] - self.xs[s];
        if dx == 0.0 {
            return self.ys[s];
        }
        let local = (t - self.xs[s]) / dx;
        let dy = self.ys[s + 1] - self.ys[s];
        let a = self.ks[s] * dx - dy;
        let b = -self.ks[s + 1] * dx + dy;
        (1.0 - local) * self.ys[s]
            + local * self.ys[s + 1]
            + local * (1.0 - local) * (a * (1.0 - local) + b * local)
    }
}

fn natural_coefficients(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let n = xs.len();
    let mut ks = vec![0.0; n];
    if n == 2 {
        let slope = (ys[1] - ys[0]) / (xs[1] - xs[0]);
        return vec![slope, slope];
    }
    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    let mut c = vec![0.0; n];
    let mut d = vec![0.0; n];
    let dx0 = xs[1] - xs[0];
    b[0] = 2.0 / dx0;
    c[0] = 1.0 / dx0;
    d[0] = 3.0 * (ys[1] - ys[0]) / (dx0 * dx0);
    for i in 1..n - 1 {
        let l = xs[i] - xs[i - 1];
        let r = xs[i + 1] - xs[i];
        a[i] = 1.0 / l;
        b[i] = 2.0 * (1.0 / l + 1.0 / r);
        c[i] = 1.0 / r;
        d[i] = 3.0 * ((ys[i] - ys[i - 1]) / (l * l) + (ys[i + 1] - ys[i]) / (r * r));
    }
    let last = n - 1;
    let dl = xs[last] - xs[last - 1];
    a[last] = 1.0 / dl;
    b[last] = 2.0 / dl;
    d[last] = 3.0 * (ys[last] - ys[last - 1]) / (dl * dl);
    for i in 1..n {
        let m = a[i] / b[i - 1];
        b[i] -= m * c[i - 1];
        d[i] -= m * d[i - 1];
    }
    ks[last] = d[last] / b[last];
    for i in (0..last).rev() {
        ks[i] = (d[i] - c[i] * ks[i + 1]) / b[i];
    }
    ks
}

/// CSS `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier(pub f32, pub f32, pub f32, pub f32);

impl Bezier {
    pub const LINEAR: Self = Self(0.0, 0.0, 1.0, 1.0);

    pub fn ease(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if x <= 0.0 || x >= 1.0 || *self == Self::LINEAR {
            return x;
        }
        let Self(x1, y1, x2, y2) = *self;
        let curve = |t: f32, a: f32, b: f32| {
            let u = 1.0 - t;
            3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
        };
        // Bisection on x(t) = x, precise enough for a frame.
        let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
        let mut t = x;
        for _ in 0..30 {
            let at = curve(t, x1, x2);
            if (at - x).abs() < 1e-5 {
                break;
            }
            if at < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) / 2.0;
        }
        curve(t, y1, y2)
    }
}

/// The ease Spicy's interlude collapse uses: a piecewise-linear curve that
/// dips below zero and then overshoots (Mixed.css `.pre-hidden .dotGroup`).
pub fn dot_collapse(x: f32) -> f32 {
    const STOPS: [f32; 28] = [
        0.0, 0.0, 0.094, -0.006, 0.18, -0.029, 0.433, -0.157, 0.514, -0.185, 0.559, -0.189, 0.6,
        -0.182, 0.639, -0.163, 0.676, -0.133, 0.723, -0.074, 0.767, 0.006, 0.85, 0.238, 0.927,
        0.566, 1.0, 1.0,
    ];
    let mut index = 0;
    while index < STOPS.len() - 4 && x > STOPS[index + 2] {
        index += 2;
    }
    let (x0, y0, x1, y1) = (
        STOPS[index],
        STOPS[index + 1],
        STOPS[index + 2],
        STOPS[index + 3],
    );
    y0 + (y1 - y0) * ((x - x0) / (x1 - x0).max(0.0001))
}

/// The easing a [`Tween`] runs on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Bezier(Bezier),
    DotCollapse,
}

impl Ease {
    pub fn at(&self, x: f32) -> f32 {
        match self {
            Self::Bezier(curve) => curve.ease(x),
            Self::DotCollapse => dot_collapse(x.clamp(0.0, 1.0)),
        }
    }
}

/// A value that moves to each new target over a fixed time, starting from
/// wherever it is: Compose's `Animatable.animateTo(target, tween(...))`.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    target: f32,
    started: f64,
    delay: f64,
    duration: f64,
    ease: Ease,
}

impl Tween {
    pub fn new(value: f32) -> Self {
        Self {
            from: value,
            target: value,
            started: 0.0,
            delay: 0.0,
            duration: 0.0,
            ease: Ease::Bezier(Bezier::LINEAR),
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Heads for `target` over `duration_ms` after `delay_ms`, unless it
    /// already is.
    pub fn animate_to(
        &mut self,
        target: f32,
        now: f64,
        duration_ms: u32,
        delay_ms: u32,
        ease: Ease,
    ) {
        if self.target == target {
            return;
        }
        self.from = self.value(now);
        self.target = target;
        self.started = now;
        self.delay = f64::from(delay_ms) / 1000.0;
        self.duration = f64::from(duration_ms) / 1000.0;
        self.ease = ease;
    }

    pub fn snap(&mut self, value: f32) {
        *self = Self::new(value);
    }

    pub fn value(&self, now: f64) -> f32 {
        if self.duration <= 0.0 {
            return self.target;
        }
        let elapsed = now - self.started - self.delay;
        if elapsed <= 0.0 {
            return self.from;
        }
        let x = (elapsed / self.duration).min(1.0) as f32;
        if x >= 1.0 {
            return self.target;
        }
        self.from + (self.target - self.from) * self.ease.at(x)
    }

    pub fn running(&self, now: f64) -> bool {
        self.duration > 0.0 && now - self.started - self.delay < self.duration
    }
}

pub fn ease_sin_out(t: f32) -> f32 {
    (t * PI / 2.0).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_word_scale_spline_peaks_where_the_reference_does() {
        let scale = Spline::new(&[(0.0, 0.95), (0.7, 1.0505), (1.0, 1.0)]);
        assert!((scale.at(0.0) - 0.95).abs() < 1e-6);
        assert!((scale.at(0.7) - 1.0505).abs() < 1e-6);
        assert!((scale.at(1.0) - 1.0).abs() < 1e-6);
        assert!((scale.at(2.0) - 1.0).abs() < 1e-6, "clamped");
        // Natural spline between: smooth, above the straight line.
        assert!(scale.at(0.35) > 0.95 + (1.0505 - 0.95) * 0.5);
    }

    #[test]
    fn a_two_point_spline_is_a_straight_line() {
        let line = Spline::new(&[(0.0, 0.01), (1.0, -0.033)]);
        assert!((line.at(0.5) - (0.01 - 0.043 / 2.0)).abs() < 1e-6);
    }

    #[test]
    fn springs_settle_on_their_goal() {
        for (frequency, damping) in [(0.88, 0.64), (1.45, 0.4), (1.0, 1.0), (2.0, 2.0)] {
            let mut spring = Spring::new(0.0, frequency, damping);
            spring.set_goal(1.0);
            for _ in 0..2_000 {
                spring.step(1.0 / 60.0);
            }
            assert!(
                (spring.position() - 1.0).abs() < 1e-3,
                "{frequency} {damping}"
            );
            assert!(spring.can_sleep());
        }
    }

    #[test]
    fn an_underdamped_spring_overshoots() {
        let mut spring = Spring::new(0.0, 1.45, 0.4);
        spring.set_goal(1.0);
        let peak = (0..120)
            .map(|_| spring.step(1.0 / 60.0))
            .fold(0.0, f32::max);
        assert!(peak > 1.1, "{peak}");
    }

    #[test]
    fn beziers_match_css() {
        let ease = Bezier(0.25, 0.1, 0.25, 1.0);
        assert!((ease.ease(0.5) - 0.8024).abs() < 0.01);
        assert_eq!(ease.ease(0.0), 0.0);
        assert_eq!(ease.ease(1.0), 1.0);
        // Chromium's smooth scroll curve is front-loaded: ~86% by halfway.
        let glide = Bezier(0.4, 0.0, 0.0, 1.0);
        assert!(glide.ease(0.5) > 0.8, "{}", glide.ease(0.5));
    }

    #[test]
    fn the_collapse_dips_then_overshoots() {
        assert!(dot_collapse(0.55) < 0.0);
        assert!((dot_collapse(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_tween_starts_from_where_it_is() {
        let mut tween = Tween::new(0.0);
        tween.animate_to(1.0, 0.0, 200, 0, Ease::Bezier(Bezier::LINEAR));
        assert!((tween.value(0.1) - 0.5).abs() < 1e-4);
        tween.animate_to(0.0, 0.1, 200, 100, Ease::Bezier(Bezier::LINEAR));
        assert!(
            (tween.value(0.15) - 0.5).abs() < 1e-4,
            "held through the delay"
        );
        assert!((tween.value(0.3) - 0.25).abs() < 1e-4);
        assert_eq!(tween.value(1.0), 0.0);
    }
}
