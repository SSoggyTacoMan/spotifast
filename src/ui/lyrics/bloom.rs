//! The Bloom look, worked out from the time alone, a port of Spicy Lyrics
//! Mobile's `BloomMotion`. Times are milliseconds; lengths are em of the
//! text's own size unless named otherwise.

use super::motion::Bezier;

pub const GAP_EM: f32 = 0.8;
pub const SPRING_GAP_EM: f32 = GAP_EM * 0.75;
pub const POSITION_SHIFT_MS: f64 = 33.5;
pub const LIT: f32 = 0.94;
pub const UNLIT: f32 = 0.18;
pub const BACKGROUND_LIT: f32 = 0.35;

pub const INACTIVE_SCALE: f32 = 0.98;
pub const SCALE_IN_MS: u32 = 500;
pub const SCALE_IN_DELAY_MS: u32 = 150;
pub const SCALE_OUT_MS: u32 = 350;
pub const SCALE_OUT_DELAY_MS: u32 = 50;
pub const SCALE_EASE: Bezier = Bezier(0.4, 0.1, 0.0, 1.0);
pub const DIM_MS: u32 = 350;
pub const DIM_DELAY_MS: u32 = 250;
pub const DIM_EASE: Bezier = Bezier(0.39, 0.575, 0.565, 1.0);
pub const LINE_LIGHT_MS: u32 = 300;
pub const LINE_DIM_MS: u32 = 400;
pub const LINE_TONE_EASE: Bezier = Bezier(0.33, 0.0, 0.2, 1.0);

pub const FADE_EM: f32 = 0.882;
pub const DEFAULT_GAP_EM: f32 = 0.19;
pub const RUSH_MS: f64 = 500.0;

pub fn reach(letters: usize, split: bool, row_end: bool) -> f32 {
    if split {
        if letters <= 2 { 0.25 } else { 0.12 }
    } else if row_end {
        1.0
    } else if letters <= 2 {
        0.5
    } else {
        0.25
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub x: f32,
    pub w: f32,
    pub reach: f32,
    pub row_start: bool,
    pub row_end: bool,
}

pub fn sweep_edges(
    spans: &[Option<Span>],
    starts: &[f64],
    ends: &[f64],
    fade: f32,
    t: f64,
    paced: impl Fn(usize) -> Option<f64>,
) -> Vec<f64> {
    let mut edges = vec![f64::NEG_INFINITY; spans.len()];
    let half = f64::from(fade) / 2.0;
    let mut reached = f64::NEG_INFINITY;
    let mut carried = f64::NEG_INFINITY;
    for i in 0..spans.len() {
        let Some(span) = spans[i] else { continue };
        let start = f64::from(span.x) - half;
        let from = if span.row_start {
            start
        } else {
            start.max(carried)
        };
        let to = from.max(f64::from(span.x + span.w + fade * span.reach) - half);
        carried = to;
        let mut at = f64::NEG_INFINITY;
        if t >= ends[i] {
            at = to;
            let lit = f64::from(span.x + span.w) + half;
            if span.row_end && to < lit {
                at = to + (lit - to) * 1.0_f64.min((t - ends[i]) / RUSH_MS);
            }
        } else if t > starts[i] {
            let share =
                paced(i).unwrap_or_else(|| (t - starts[i]) / (ends[i] - starts[i]).max(1.0));
            at = from + (to - from) * share;
        }
        edges[i] = at;
        if at > reached {
            reached = at;
        }
    }
    let mut before = f64::NEG_INFINITY;
    for i in 0..spans.len() {
        let Some(span) = spans[i] else { continue };
        if span.row_start {
            before = f64::NEG_INFINITY;
        }
        let at = edges[i];
        if t >= ends[i] {
            edges[i] = reached;
        } else if t <= starts[i] {
            edges[i] = before;
        }
        if at > before {
            before = at;
        }
    }
    edges
}

pub fn paced_share(starts: &[f64], ends: &[f64], xs: &[f32], ws: &[f32], t: f64) -> Option<f64> {
    let n = starts.len();
    if n == 0 || xs.len() != n || ws.len() != n {
        return None;
    }
    let width = xs[n - 1] + ws[n - 1];
    if width <= 0.0 {
        return None;
    }
    let mut x = 0.0;
    for j in 0..n {
        if t < starts[j] {
            break;
        }
        let length = ends[j] - starts[j];
        let filled = if length > 0.0 {
            1.0_f64.min((t - starts[j]) / length)
        } else {
            1.0
        };
        x = f64::from(xs[j]) + f64::from(ws[j]) * filled;
        if filled < 1.0 {
            break;
        }
    }
    Some(1.0_f64.min(x / f64::from(width)))
}

pub fn gradient_position(edge: f64, x: f32, width: f32, fade: f32) -> f32 {
    if edge == f64::NEG_INFINITY {
        return -50.0;
    }
    let rest = (f64::from(width) + f64::from(fade) / 2.0 - (edge - f64::from(x)))
        / f64::from(width + fade);
    (100.0 - 150.0 * rest).clamp(-50.0, 100.0) as f32
}

pub fn letter_tone(gradient: f32) -> f32 {
    UNLIT + (LIT - UNLIT) * ((gradient + 50.0) / 150.0).clamp(0.0, 1.0)
}

/// The bright edge and the start of the dim part, as shares of a box.
pub fn wipe_stops(position: f32, width: f32, fade: f32) -> (f32, f32) {
    let w = width.max(1.0);
    let start = (position + 50.0) / 150.0 * (w + fade) - fade;
    (start / w, (start + fade) / w)
}

pub const LIFT_EM: f32 = 0.0588;
const LIFT_OMEGA: f64 = 5.0;
const LIFT_DAMPING: f64 = 0.93;

pub fn lift(elapsed_ms: f64) -> f64 {
    if elapsed_ms <= 0.0 {
        return 0.0;
    }
    let t = elapsed_ms / 1000.0;
    let decay = (-LIFT_DAMPING * LIFT_OMEGA * t).exp();
    if decay < 1e-4 {
        return 1.0;
    }
    let wd = LIFT_OMEGA * (1.0 - LIFT_DAMPING * LIFT_DAMPING).sqrt();
    1.0 - decay * ((wd * t).cos() + (LIFT_DAMPING * LIFT_OMEGA / wd) * (wd * t).sin())
}

const SCALE: f64 = 0.14;
const FROM_MS: f64 = 1000.0;
const TO_MS: f64 = 2000.0;
const STAGGER: f64 = 0.4;
const MAX_STAGGER_MS: f64 = 400.0;
const MAX_PHASE_MS: f64 = 3000.0;
pub const GLOW_ALPHA: f32 = 0.5;
pub const GLOW_RADIUS_EM: f32 = 0.17 + 1.0 / 56.0;
const EMPHASIS: Bezier = Bezier(0.25, 0.1, 0.25, 1.0);

fn ease(x: f64) -> f64 {
    f64::from(EMPHASIS.ease(x.clamp(0.0, 1.0) as f32))
}

/// Every letter of a held word at one moment. `rest` is set when all are at
/// rest: not started (0) or settled and lifted (1).
#[derive(Clone, Debug, Default)]
pub struct Flame {
    pub rest: Option<u8>,
    pub scale: Vec<f32>,
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    pub glow: Vec<f32>,
}

impl Flame {
    pub fn scale(&self, i: usize) -> f32 {
        self.scale.get(i).copied().unwrap_or(1.0)
    }
    pub fn x(&self, i: usize) -> f32 {
        self.x.get(i).copied().unwrap_or(0.0)
    }
    pub fn y(&self, i: usize) -> f32 {
        self.y
            .get(i)
            .copied()
            .unwrap_or(-LIFT_EM * f32::from(self.rest.unwrap_or(0)))
    }
    pub fn glow(&self, i: usize) -> f32 {
        self.glow.get(i).copied().unwrap_or(0.0)
    }
    pub fn slice(&self, from: usize, count: usize) -> Flame {
        if self.rest.is_some() {
            return self.clone();
        }
        let cut = |v: &Vec<f32>| v[from.min(v.len())..(from + count).min(v.len())].to_vec();
        Flame {
            rest: None,
            scale: cut(&self.scale),
            x: cut(&self.x),
            y: cut(&self.y),
            glow: cut(&self.glow),
        }
    }
}

pub fn flame(elapsed: f64, duration: f64, count: usize, widths_em: &[f32]) -> Flame {
    let n = count.max(1);
    let du = duration.max(1.0);
    let phase = du.min(MAX_PHASE_MS);
    let stagger = (STAGGER * du / n as f64).min(MAX_STAGGER_MS);
    let hold = 2.0 * du / n as f64;
    let peak = SCALE * ((du / FROM_MS).clamp(1.0, TO_MS / FROM_MS) - 1.0);
    let held = ease((hold / phase).min(1.0));
    if elapsed <= 0.0 {
        return Flame {
            rest: Some(0),
            ..Flame::default()
        };
    }
    let last_at = elapsed - (n - 1) as f64 * stagger;
    if last_at >= hold + phase && lift(last_at) == 1.0 {
        return Flame {
            rest: Some(1),
            ..Flame::default()
        };
    }
    let mut scale = vec![0.0; n];
    let mut glow = vec![0.0; n];
    let mut y = vec![0.0; n];
    for i in 0..n {
        let at = elapsed - i as f64 * stagger;
        let amount = if at > 0.0 && at < hold {
            ease((at / phase).min(1.0))
        } else if at >= hold {
            held * (1.0 - ease(((at - hold) / phase).min(1.0)))
        } else {
            0.0
        };
        scale[i] = (1.0 + peak * amount) as f32;
        glow[i] = GLOW_ALPHA * amount as f32;
        y[i] = -LIFT_EM * lift(at) as f32;
    }
    let half: Vec<f32> = (0..n)
        .map(|i| (scale[i] - 1.0) * widths_em.get(i).copied().unwrap_or(0.0) / 2.0)
        .collect();
    let mut x = vec![0.0_f32; n];
    let middle = if n.is_multiple_of(2) {
        n as f64 / 2.0 - 0.5
    } else {
        (n as f64 / 2.0).floor()
    };
    let whole = middle == middle.floor();
    let left_start: isize = if whole {
        middle as isize - 1
    } else {
        middle.floor() as isize
    };
    let right_start: usize = if whole {
        middle as usize + 1
    } else {
        middle.ceil() as usize
    };
    let mut i = left_start;
    while i >= 0 {
        let u = i as usize;
        let mut push = half[u];
        if (u + 1) as f64 <= middle {
            push += half[u + 1];
        }
        if ((u + 1) as f64) < middle {
            push += x[u + 1].abs();
        }
        x[u] = -push / 2.0;
        i -= 1;
    }
    for u in right_start..n {
        let mut push = half[u];
        if u >= 1 && (u - 1) as f64 >= middle {
            push += half[u - 1];
        }
        if u >= 1 && (u - 1) as f64 > middle {
            push += x[u - 1].abs();
        }
        x[u] = push / 2.0;
    }
    Flame {
        rest: None,
        scale,
        x,
        y,
        glow,
    }
}

const DOT_DIM: f64 = 0.18;
const DOT_LIT: f64 = 0.94;
const DOT_ENTER_MS: f64 = 750.0;
const DOT_ENTER_STAGGER_MS: f64 = 50.0;
const BREATH_MS: f64 = 4000.0;
const BREATH_SCALE: f64 = 0.2;
const EXIT_GROW_MS: f64 = 750.0;
const EXIT_SHRINK_MS: f64 = 250.0;
const EXIT_SCALE: f64 = 0.5;
const DOT_FILL: Bezier = Bezier(0.0, 0.25, 1.0, 0.58);
const DOT_EXIT: Bezier = Bezier(0.25, 0.0, 1.0, 0.2);
const DOT_BREATH: Bezier = Bezier(0.5, 0.02, 0.42, 0.98);

fn at(curve: Bezier, x: f64) -> f64 {
    f64::from(curve.ease(x.clamp(0.0, 1.0) as f32))
}

fn exit_at(duration: f64, close: f64) -> f64 {
    0.0_f64.max(duration - close - EXIT_GROW_MS)
}

pub fn dot_opacity(elapsed: f64, duration: f64, index: usize, count: usize, close: f64) -> f32 {
    let n = count.max(1);
    let exit = exit_at(duration, close);
    let share = (exit + EXIT_GROW_MS) / n as f64;
    let fill = if index < n - 1 {
        at(DOT_FILL, (elapsed - index as f64 * share) / share)
    } else {
        let from = index as f64 * share;
        let room = 1.0_f64.max(exit - from);
        let reached = 1.0_f64.min(room / share);
        if elapsed < exit {
            reached * at(DOT_FILL, (elapsed - from) / room)
        } else {
            reached + (1.0 - reached) * ((elapsed - exit) / EXIT_GROW_MS).clamp(0.0, 1.0)
        }
    };
    let enter = ((elapsed - index as f64 * DOT_ENTER_STAGGER_MS) / DOT_ENTER_MS).clamp(0.0, 1.0);
    ((DOT_DIM + (DOT_LIT - DOT_DIM) * fill) * enter * enter) as f32
}

fn breath(x: f64) -> f64 {
    if x < 0.5 {
        1.0 + BREATH_SCALE * at(DOT_BREATH, x * 2.0)
    } else {
        1.0 + BREATH_SCALE - BREATH_SCALE * at(DOT_BREATH, (x - 0.5) * 2.0)
    }
}

/// The dot group's breathing scale and its opacity as it leaves.
pub fn dot_group(elapsed: f64, duration: f64, close: f64) -> (f32, f32) {
    let exit = exit_at(duration, close);
    let breath_scale = |t: f64| {
        let cycles = (exit / BREATH_MS).floor();
        if cycles < 1.0 {
            return 1.0;
        }
        let length = exit / cycles;
        breath((t.clamp(0.0, exit) % length) / length)
    };
    if elapsed < exit {
        (breath_scale(elapsed) as f32, 1.0)
    } else if elapsed < exit + EXIT_GROW_MS {
        let from = breath_scale(exit);
        let top = 1.0 + BREATH_SCALE;
        (
            (from + (top - from) * ease((elapsed - exit) / EXIT_GROW_MS)) as f32,
            1.0,
        )
    } else {
        let x = at(DOT_EXIT, (elapsed - exit - EXIT_GROW_MS) / EXIT_SHRINK_MS);
        (
            (1.0 + BREATH_SCALE + (EXIT_SCALE - 1.0 - BREATH_SCALE) * x) as f32,
            (1.0 - x) as f32,
        )
    }
}

const SCROLL_MIN_MS: f64 = 480.0;
const SCROLL_MAX_MS: f64 = 750.0;
const SCROLL_GAP_FROM_MS: f64 = 200.0;
const SCROLL_GAP_TO_MS: f64 = 750.0;
pub const SCROLL_EASE: Bezier = Bezier(0.4, 0.1, 0.0, 1.0);
const ROW_DELAYS_MS: [u32; 4] = [25, 44, 56, 63];

/// How long the move into the line after `position` takes.
pub fn scroll_ms(lines: &[super::timeline::TLine], position: f64) -> u32 {
    let Some(next) = lines
        .iter()
        .position(|l| !l.is_background() && l.start > position)
    else {
        return SCROLL_MAX_MS as u32;
    };
    let mut prev_end = f64::NEG_INFINITY;
    for line in lines[..next].iter().rev() {
        let end = line
            .words
            .iter()
            .map(|w| w.end)
            .fold(f64::NEG_INFINITY, f64::max);
        let end = if end.is_finite() { end } else { line.end };
        prev_end = prev_end.max(end);
        if !line.is_background() {
            break;
        }
    }
    let room = if prev_end.is_finite() {
        lines[next].start - prev_end
    } else {
        f64::MAX
    };
    let gap = room.clamp(SCROLL_GAP_FROM_MS, SCROLL_GAP_TO_MS);
    let share = (gap - SCROLL_GAP_FROM_MS) / (SCROLL_GAP_TO_MS - SCROLL_GAP_FROM_MS);
    (SCROLL_MIN_MS + (SCROLL_MAX_MS - SCROLL_MIN_MS) * share).round() as u32
}

pub fn row_delay_ms(below: isize) -> u32 {
    if below <= 0 {
        0
    } else {
        ROW_DELAYS_MS[(below as usize).min(ROW_DELAYS_MS.len()) - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lift_settles_at_one() {
        assert_eq!(lift(0.0), 0.0);
        assert!((lift(300.0) - 0.4604).abs() < 1e-3);
        assert!(lift(1_500.0) > 0.99);
        assert_eq!(lift(5_000.0), 1.0);
    }

    #[test]
    fn a_held_word_swells_letter_by_letter() {
        let flame = flame(600.0, 2_000.0, 4, &[0.5; 4]);
        assert!(flame.rest.is_none());
        assert!(flame.scale(0) > flame.scale(3));
        assert!(flame.x(0) < 0.0 && flame.x(3) > 0.0);
        assert_eq!(super::flame(-1.0, 2_000.0, 4, &[]).rest, Some(0));
    }

    #[test]
    fn the_scroll_takes_longer_after_a_pause() {
        assert_eq!(row_delay_ms(0), 0);
        assert_eq!(row_delay_ms(9), 63);
    }
}
