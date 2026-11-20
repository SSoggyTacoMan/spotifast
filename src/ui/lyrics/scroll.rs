//! How the lyrics follow the song, a port of Spicy Lyrics Mobile's
//! `ScrollManager`, `ScrollPolicyController` and `RippleScroll`.
//!
//! - Scrolling by hand stops auto-scroll and hides the distance blur.
//! - Auto-scroll comes back once nothing has moved the lyrics for 750 ms and
//!   at least 5 points of the line being sung are on screen; it then holds
//!   still until the line changes.
//! - A jump of over a second in the song snaps to the line; shorter moves
//!   glide like Chromium's `scroll-behavior: smooth`.

use super::bloom;
use super::motion::{Bezier, Spring};
use super::timeline::{PRE_HIDDEN_DOT_LINE_MS, Role, TLine};

const USER_SCROLL_COOLDOWN_MS: f64 = 750.0;
const MIN_VISIBLE: f32 = 5.0;
const MIN_FLING_VELOCITY: f32 = 10.0;
const OVERSCROLL: f32 = 60.0;
const EDGE_RETURN_RATE: f32 = 12.0;
const MAX_GLIDE_FRAMES: f32 = 90.0;
const GLIDE_EASE: Bezier = Bezier(0.4, 0.0, 0.0, 1.0);
const SMOOTH_SETTLE: f32 = 0.25;
const SMOOTH_SETTLE_SPEED: f32 = 0.05;
const MAX_SPRING_STEP: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    None,
    Snap,
    Smooth,
}

#[derive(Default)]
pub struct Policy {
    initialized: bool,
    last_time: f64,
    last_target: Option<usize>,
}

impl Policy {
    pub fn decide(&mut self, lines: &[TLine], time: f64, lead: f64) -> (Option<usize>, Motion) {
        let replay = self.initialized && time <= 100.0 && self.last_time > 1_000.0;
        let seek = self.initialized && (time - self.last_time).abs() > 1_000.0;
        let jumped = !self.initialized || replay || seek;
        let at = time + lead.max(0.0);
        let target = select_target(lines, at).or_else(|| {
            if jumped {
                last_started(lines, at)
            } else {
                self.last_target
            }
        });
        let motion = match target {
            None => Motion::None,
            Some(_) if jumped => Motion::Snap,
            Some(t) if Some(t) != self.last_target => Motion::Smooth,
            Some(_) => Motion::None,
        };
        self.initialized = true;
        self.last_time = time;
        self.last_target = target;
        (target, motion)
    }
}

fn sung_from(line: &TLine) -> f64 {
    line.words
        .iter()
        .find(|w| !w.text.trim().is_empty())
        .map_or(line.start, |w| w.start.max(line.start))
}

pub fn select_target(lines: &[TLine], t: f64) -> Option<usize> {
    let active: Vec<usize> = (0..lines.len())
        .filter(|&i| t >= lines[i].start && t <= lines[i].end)
        .collect();
    if active.is_empty() {
        return None;
    }
    if let Some(&index) = active.iter().find(|&&i| lines[i].role == Role::Interlude)
        && t > lines[index].end - PRE_HIDDEN_DOT_LINE_MS
        && let Some(next) = (index + 1..lines.len()).find(|&j| lines[j].role == Role::Lead)
    {
        return Some(next);
    }
    let is_bg = |i: usize| lines[i].role == Role::Background;
    let lead_of = |i: usize| {
        let mut j = i;
        while j > 0 && is_bg(j) {
            j -= 1;
        }
        j
    };
    let front = active.iter().map(|&i| lead_of(i)).max()?;
    let mut leads: Vec<usize> = active
        .iter()
        .filter(|&&i| !(is_bg(i) && lead_of(i) < front))
        .map(|&i| lead_of(i))
        .collect();
    leads.dedup();
    let anchor = leads[0];
    let lookahead = (anchor + 1..lines.len()).filter(|&i| !is_bg(i)).nth(1);
    let mut group_end = lines[anchor].end;
    let mut k = anchor + 1;
    while k < lines.len() && is_bg(k) {
        group_end = group_end.max(lines[k].end);
        k += 1;
    }
    if lookahead.is_none_or(|l| group_end <= sung_from(&lines[l])) {
        return Some(anchor);
    }
    let (first, last) = (leads[0], *leads.last().expect("leads"));
    let adjacent = (first + 1..last).all(is_bg);
    Some(if adjacent { first } else { last })
}

fn last_started(lines: &[TLine], t: f64) -> Option<usize> {
    lines
        .iter()
        .rposition(|l| l.role != Role::Background && l.start <= t)
        .or_else(|| lines.iter().position(|l| l.role != Role::Background))
}

pub struct Manager {
    pub scroll: f32,
    pub top: f32,
    pub user_scrolling: bool,
    pub hide_blur: bool,
    pub smooth: bool,
    pub rows: bool,
    /// Points per desktop CSS px: glide durations are in those.
    pub per_reference: f32,
    velocity: f32,
    last_drag: f64,
    last_user_move: f64,
    last_auto: Option<usize>,
    hold_until_change: bool,
    snap_next: bool,
    glide_from: f32,
    glide_to: f32,
    glide_elapsed: f32,
    glide_duration: f32,
    spring: Spring,
    spring_active: bool,
    spring_last: f32,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            scroll: 0.0,
            top: 0.0,
            user_scrolling: false,
            hide_blur: false,
            smooth: false,
            rows: false,
            per_reference: 1.0,
            velocity: 0.0,
            last_drag: 0.0,
            last_user_move: 0.0,
            last_auto: None,
            hold_until_change: false,
            snap_next: true,
            glide_from: 0.0,
            glide_to: 0.0,
            glide_elapsed: 0.0,
            glide_duration: 0.0,
            spring: Spring::new(0.0, 1.0, 1.0),
            spring_active: false,
            spring_last: 0.0,
        }
    }
}

impl Manager {
    fn bottom(&self, content: f32) -> f32 {
        (-content.max(0.0)).min(self.top)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        now_ms: f64,
        dt: f32,
        content: f32,
        target_index: Option<usize>,
        target_y: Option<f32>,
        snap: bool,
        target_visible: f32,
    ) {
        let top = self.top;
        let bottom = self.bottom(content);
        let mut y = self.scroll;
        if target_y.is_none() && self.snap_next {
            self.scroll = top;
            self.snap_next = false;
            return;
        }
        if let Some(goal) = target_y
            && (snap || self.snap_next)
        {
            let y = goal.clamp(bottom, top);
            self.take_back(target_index, y);
            self.snap_next = false;
            self.scroll = y;
            return;
        }
        if self.user_scrolling {
            self.last_user_move = now_ms;
        } else if self.velocity.abs() > MIN_FLING_VELOCITY {
            y += self.velocity * dt;
            self.velocity *= (0.95_f32.ln() * dt.max(0.0) * 60.0).exp();
            if y > top + OVERSCROLL || y < bottom - OVERSCROLL {
                self.velocity = 0.0;
            }
            self.last_user_move = now_ms;
        } else {
            self.velocity = 0.0;
        }
        if self.hide_blur
            && !self.user_scrolling
            && self.velocity == 0.0
            && now_ms - self.last_user_move > USER_SCROLL_COOLDOWN_MS
            && target_visible >= MIN_VISIBLE
        {
            self.hide_blur = false;
            self.hold_until_change = true;
            self.settle(y);
        }
        if !self.hide_blur
            && let Some(goal) = target_y
        {
            if target_index != self.last_auto {
                self.last_auto = target_index;
                self.hold_until_change = false;
            }
            if !self.hold_until_change {
                y = self.glide(y, goal.clamp(bottom, top), dt);
            }
        }
        if !self.user_scrolling {
            let bound = y.clamp(bottom, top);
            if y != bound && self.velocity == 0.0 {
                y += (bound - y) * (dt * EDGE_RETURN_RATE).min(1.0);
                if (bound - y).abs() < 0.5 {
                    y = bound;
                }
                if !self.hide_blur {
                    self.settle(y);
                }
            }
        }
        self.scroll = y;
    }

    /// The same lyrics were laid out again: jump to the line's new place.
    pub fn relayout(&mut self) {
        if !self.hide_blur {
            self.snap_next = true;
        }
    }

    /// A wheel or trackpad moved the lyrics by `dy` points.
    pub fn wheel(&mut self, now_ms: f64, dy: f32, content: f32) {
        self.hide_blur = true;
        self.velocity = 0.0;
        self.last_user_move = now_ms;
        let bottom = self.bottom(content);
        self.scroll = (self.scroll + dy).clamp(bottom - OVERSCROLL, self.top + OVERSCROLL);
        self.settle(self.scroll);
    }

    pub fn drag_start(&mut self, now_ms: f64) {
        self.user_scrolling = true;
        self.hide_blur = true;
        self.velocity = 0.0;
        self.last_drag = now_ms;
        self.last_user_move = now_ms;
    }

    pub fn drag(&mut self, now_ms: f64, dy: f32, content: f32) {
        let dt = ((now_ms - self.last_drag) / 1000.0) as f32;
        if dt > 0.0 {
            self.velocity = self.velocity * 0.4 + (dy / dt) * 0.6;
        }
        self.last_drag = now_ms;
        self.last_user_move = now_ms;
        let outside = self.scroll > self.top || self.scroll < self.bottom(content);
        self.scroll += if outside { dy * 0.5 } else { dy };
    }

    pub fn drag_end(&mut self, now_ms: f64) {
        self.user_scrolling = false;
        if now_ms - self.last_drag > 80.0 {
            self.velocity = 0.0;
        }
    }

    /// A tapped line or the back-to-the-song button: auto-scroll takes over.
    pub fn return_to_active(&mut self) {
        self.take_back(None, self.scroll);
    }

    pub fn moving(&self) -> bool {
        self.spring_active
            || self.glide_elapsed < self.glide_duration
            || self.velocity != 0.0
            || self.user_scrolling
            || self.hide_blur
    }

    fn glide(&mut self, y: f32, goal: f32, dt: f32) -> f32 {
        if self.rows {
            return goal;
        }
        if self.smooth {
            return self.spring_glide(y, goal, dt);
        }
        if (goal - self.glide_to).abs() > 0.5 {
            self.glide_from = y;
            self.glide_to = goal;
            self.glide_elapsed = 0.0;
            let reference = (goal - y).abs() / self.per_reference.max(0.01);
            self.glide_duration = reference.sqrt().min(MAX_GLIDE_FRAMES) / 60.0;
        }
        if self.glide_elapsed >= self.glide_duration {
            return self.glide_to;
        }
        self.glide_elapsed += dt;
        let t = (self.glide_elapsed / self.glide_duration).clamp(0.0, 1.0);
        self.glide_from + (self.glide_to - self.glide_from) * GLIDE_EASE.ease(t)
    }

    fn spring_glide(&mut self, y: f32, goal: f32, dt: f32) -> f32 {
        if !self.spring_active {
            if (goal - y).abs() < SMOOTH_SETTLE {
                return goal;
            }
            self.spring = Spring::new(y, 1.0, 1.0);
            self.spring_active = true;
            self.spring_last = y;
        }
        self.spring.set_goal(goal);
        let position = self.spring.step(dt.min(MAX_SPRING_STEP));
        let moved = (position - self.spring_last).abs();
        self.spring_last = position;
        if (position - goal).abs() < SMOOTH_SETTLE && moved < SMOOTH_SETTLE_SPEED {
            self.spring_active = false;
            return goal;
        }
        position
    }

    fn settle(&mut self, y: f32) {
        self.spring_active = false;
        self.glide_from = y;
        self.glide_to = y;
        self.glide_elapsed = 0.0;
        self.glide_duration = 0.0;
    }

    fn take_back(&mut self, target: Option<usize>, y: f32) {
        self.settle(y);
        self.velocity = 0.0;
        self.hide_blur = false;
        self.hold_until_change = false;
        self.last_auto = target;
    }
}

#[derive(Clone, Copy)]
struct Row {
    p: f32,
    v: f32,
    goal: f32,
    next: f32,
    wait: f32,
    from: f32,
    at: f32,
    dur: f32,
}

impl Row {
    fn new(p: f32) -> Self {
        Self {
            p,
            v: 0.0,
            goal: p,
            next: p,
            wait: 0.0,
            from: p,
            at: 0.0,
            dur: 0.0,
        }
    }
}

/// Bloom: each row follows the list on its own, the rows below the line a
/// little later each.
#[derive(Default)]
pub struct Ripple {
    rows: Vec<Row>,
    scroll: f32,
    target: Option<usize>,
    clock: f32,
}

impl Ripple {
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        from: f32,
        to: f32,
        target: Option<usize>,
        tops: &[f32],
        heights: &[f32],
        anchor: f32,
        viewport: f32,
        dt: f32,
        snap: bool,
        user: bool,
        timed_ms: u32,
    ) -> bool {
        let elapsed = dt.clamp(0.0, 0.1);
        self.clock += elapsed;
        if snap || (to - from).abs() > viewport * 0.7 {
            self.rows = vec![Row::new(to); tops.len()];
            self.scroll = to;
            self.target = target;
            return false;
        }
        if self.rows.len() != tops.len() {
            self.rows = vec![Row::new(from); tops.len()];
        }
        if user {
            for row in &mut self.rows {
                row.p += to - self.scroll;
                row.dur = 0.0;
            }
        }
        if (to - self.scroll).abs() >= 0.01 && timed_ms > 0 && !user {
            let changed = target != self.target;
            let clock = self.clock;
            for (i, row) in self.rows.iter_mut().enumerate() {
                row.v = 0.0;
                row.wait = 0.0;
                row.goal = to;
                row.next = to;
                if !changed && row.dur > 0.0 {
                    continue;
                }
                row.from = row.p;
                let below = target.map_or(0, |t| i as isize - t as isize);
                row.at = clock + bloom::row_delay_ms(below) as f32 / 1000.0;
                row.dur = timed_ms as f32 / 1000.0;
            }
            self.target = target;
        } else if (to - self.scroll).abs() >= 0.01 {
            let mut delay = 0.0;
            let mut step = 0.042;
            let changed = target != self.target;
            for (i, row) in self.rows.iter_mut().enumerate() {
                row.next = to;
                if user || anchor + tops[i] + heights[i] + from <= 0.0 {
                    row.goal = to;
                    row.wait = 0.0;
                } else if changed {
                    row.wait = delay;
                    if delay == 0.0 {
                        row.goal = to;
                    }
                    delay += step;
                    if i >= target.unwrap_or(0) {
                        step *= 0.9;
                    }
                } else if row.wait == 0.0 {
                    row.goal = to;
                }
            }
            self.target = target;
        }
        self.scroll = to;
        let mut active = false;
        let w = 2.0 * std::f32::consts::PI * 2.1;
        for row in &mut self.rows {
            if user {
                row.goal = to;
                row.wait = 0.0;
            }
            if row.dur > 0.0 {
                let u = ((self.clock - row.at) / row.dur).clamp(0.0, 1.0);
                row.p = row.from + (row.goal - row.from) * bloom::SCROLL_EASE.ease(u);
                if u >= 1.0 && row.goal == to {
                    row.p = to;
                    row.dur = 0.0;
                } else {
                    active = true;
                }
                continue;
            }
            if row.wait > 0.0 {
                row.wait = (row.wait - elapsed).max(0.0);
                if row.wait == 0.0 {
                    row.goal = row.next;
                }
            }
            let q = (-w * elapsed).exp();
            let o = row.p - row.goal;
            let k = row.v + w * o;
            row.p = (o + k * elapsed) * q + row.goal;
            row.v = (row.v - w * k * elapsed) * q;
            if row.wait == 0.0 && row.goal == to && (row.p - to).abs() < 0.2 && row.v.abs() < 4.0 {
                row.p = to;
                row.v = 0.0;
            } else {
                active = true;
            }
        }
        active
    }

    pub fn offset(&self, index: usize) -> f32 {
        if self.rows.is_empty() {
            return 0.0;
        }
        self.rows[index.min(self.rows.len() - 1)].p - self.scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::lyrics::timeline::Word;

    fn line(start: f64, end: f64, role: Role) -> TLine {
        TLine {
            words: vec![Word {
                text: "x".into(),
                start,
                end,
                glued: false,
                letters: Vec::new(),
            }],
            start,
            end,
            role,
            group: 0,
            opposite: false,
            line_timed: false,
            text: String::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn the_target_is_the_line_being_sung_not_its_background() {
        let lines = vec![
            line(0.0, 1000.0, Role::Lead),
            line(500.0, 1500.0, Role::Background),
            line(1200.0, 2000.0, Role::Lead),
        ];
        assert_eq!(select_target(&lines, 600.0), Some(0));
        assert_eq!(select_target(&lines, 1300.0), Some(2));
    }

    #[test]
    fn a_seek_snaps_and_a_new_line_glides() {
        let lines = vec![
            line(0.0, 1000.0, Role::Lead),
            line(1000.0, 2000.0, Role::Lead),
        ];
        let mut policy = Policy::default();
        assert_eq!(policy.decide(&lines, 100.0, 0.0), (Some(0), Motion::Snap));
        assert_eq!(policy.decide(&lines, 200.0, 0.0).1, Motion::None);
        assert_eq!(
            policy.decide(&lines, 1100.0, 0.0),
            (Some(1), Motion::Smooth)
        );
    }

    #[test]
    fn a_glide_lands_and_a_hand_scroll_waits() {
        let mut manager = Manager::default();
        manager.update(0.0, 0.016, 1000.0, Some(0), Some(-100.0), false, 100.0);
        assert_eq!(manager.scroll, -100.0, "the first frame snaps");
        for frame in 1..200 {
            manager.update(
                frame as f64 * 16.0,
                0.016,
                1000.0,
                Some(1),
                Some(-300.0),
                false,
                100.0,
            );
        }
        assert!((manager.scroll + 300.0).abs() < 0.01);
        manager.wheel(4000.0, 50.0, 1000.0);
        manager.update(4100.0, 0.016, 1000.0, Some(2), Some(-500.0), false, 100.0);
        assert!(manager.hide_blur && (manager.scroll + 250.0).abs() < 0.01);
        manager.update(5000.0, 0.016, 1000.0, Some(2), Some(-500.0), false, 100.0);
        assert!(!manager.hide_blur, "back after 750 ms");
    }
}
