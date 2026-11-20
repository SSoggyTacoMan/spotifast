//! The lyrics animator, stepped once a frame with the song's position: a
//! port of Spicy Lyrics Mobile's `LyricsAnimator`.
//!
//! - Only active lines have their word, letter and dot springs retargeted.
//! - Lines still to come are never touched: they keep their resting pose.
//! - A sung line keeps settling while the next is not yet sung, or until its
//!   springs come to rest, so a finished line eases out instead of stopping.
//! - Line opacity tweens like the CSS class change it stands for; distance
//!   blur is worked out again only when the active line changes.

use std::collections::HashMap;

use super::bloom;
use super::motion::{Bezier, Ease, Spline, Spring, Tween, ease_sin_out};
use super::timeline::{self, Kind, Role, TLine, Word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotSung,
    Active,
    Sung,
}

pub fn state(t: f64, start: f64, end: f64) -> State {
    if t < start {
        State::NotSung
    } else if t >= end {
        State::Sung
    } else {
        State::Active
    }
}

pub fn progress(t: f64, start: f64, end: f64) -> f32 {
    if t <= start {
        0.0
    } else if t >= end {
        1.0
    } else {
        ((t - start) / (end - start)) as f32
    }
}

/// Moves a tween to `target`, or puts it there at once when less motion is
/// asked for.
fn aim(tween: &mut Tween, target: f32, now: f64, ms: u32, delay: u32, ease: Ease, reduced: bool) {
    if reduced {
        tween.snap(target);
    } else {
        tween.animate_to(target, now, ms, delay, ease);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LetterAnim {
    pub gradient: f32,
    pub scale: f32,
    pub y: f32,
    pub glow: f32,
    /// Bloom: the sideways push, in em.
    pub x: f32,
    /// Bloom: the glow's radius, in em.
    pub glow_radius: f32,
}

#[derive(Clone, Debug)]
pub struct WordAnim {
    pub scale: f32,
    pub y: f32,
    pub glow: f32,
    pub gradient: f32,
    pub state: State,
    /// Interlude dots: the halo.
    pub dot_glow: f32,
    pub letters: Vec<LetterAnim>,
}

impl WordAnim {
    fn rest(word: &Word, splines: &Splines) -> Self {
        Self {
            scale: splines.scale.at(0.0),
            y: splines.y.at(0.0),
            glow: 0.0,
            gradient: -20.0,
            state: State::NotSung,
            dot_glow: 0.0,
            letters: word
                .letters
                .iter()
                .map(|_| LetterAnim {
                    gradient: -20.0,
                    scale: splines.letter_scale.at(0.0),
                    y: splines.letter_y.at(0.0),
                    ..LetterAnim::default()
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LineAnim {
    pub opacity: f32,
    pub blur: f32,
    pub scale: f32,
    pub active: bool,
    pub state: State,
    pub words: Vec<WordAnim>,
    pub line_gradient: f32,
    pub line_glow: f32,
    /// Bloom: how lit the line still is, 1 lit, 0 at rest.
    pub lit: f32,
    /// Background pop-out: how far open, and where it heads.
    pub presence: f32,
    pub presence_goal: f32,
    /// Bloom interludes: the group's breathing.
    pub group_scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub bloom: bool,
    pub minimal: bool,
    pub blur: bool,
    pub glow: bool,
    pub background_pop: bool,
    pub keep_overlapping: bool,
    pub pace: bool,
}

impl Config {
    pub fn bright(&self) -> f32 {
        if self.bloom { bloom::LIT } else { 0.85 }
    }
    pub fn dim(&self) -> f32 {
        if self.bloom { bloom::UNLIT } else { 0.5 }
    }
    pub fn line_dim(&self) -> f32 {
        if self.bloom { bloom::UNLIT } else { 0.35 }
    }
    pub fn background_bright(&self) -> f32 {
        if self.bloom {
            bloom::BACKGROUND_LIT
        } else {
            0.6
        }
    }
    pub fn background_dim(&self) -> f32 {
        if self.bloom { bloom::UNLIT } else { 0.3 }
    }
    fn opacity_not_sung(&self) -> f32 {
        if self.bloom { 1.0 } else { 0.51 }
    }
    fn opacity_sung(&self) -> f32 {
        if self.bloom { 1.0 } else { 0.497 }
    }
}

const SCALE_FREQUENCY: f32 = 0.88;
const SCALE_DAMPING: f32 = 0.64;
const Y_FREQUENCY: f32 = 1.45;
const Y_DAMPING: f32 = 0.4;
const GLOW_FREQUENCY: f32 = 1.18;
const GLOW_DAMPING: f32 = 0.56;
const SUNG_LETTER_GLOW: f32 = 0.2;
pub const BLUR_MULTIPLIER: f32 = 1.25;
pub const MAX_BLUR: f32 = BLUR_MULTIPLIER * 5.0 + BLUR_MULTIPLIER * 0.465;

const OPACITY_EASE: Ease = Ease::Bezier(Bezier(0.61, 1.0, 0.88, 1.0));
const SCALE_EASE: Ease = Ease::Bezier(Bezier(0.37, 0.0, 0.63, 1.0));
const EASE_IN_OUT: Ease = Ease::Bezier(Bezier(0.42, 0.0, 0.58, 1.0));
const LINEAR: Ease = Ease::Bezier(Bezier::LINEAR);
const POP_EASE: Ease = Ease::Bezier(Bezier(0.22, 1.0, 0.36, 1.0));
const POP_FADE_EASE: Ease = Ease::Bezier(Bezier(0.0, 0.0, 0.58, 1.0));
pub const ACTIVE_LINE_SCALE: f32 = 1.05;
pub const POP_FOLDED_SCALE: f32 = 0.8;
pub const BACKGROUND_FOLDED_SHIFT: f32 = 0.3;

pub struct Splines {
    pub scale: Spline,
    pub glow: Spline,
    pub y: Spline,
    pub letter_scale: Spline,
    pub letter_y: Spline,
    dot_scale: Spline,
    dot_y: Spline,
    dot_glow: Spline,
    dot_opacity: Spline,
    line_glow: Spline,
}

impl Splines {
    fn new(bloom: bool) -> Self {
        Self {
            scale: Spline::new(&[(0.0, 0.95), (0.7, 1.0505), (1.0, 1.0)]),
            glow: Spline::new(&[(0.0, 0.0), (0.15, 1.0), (0.6, 1.0), (1.0, 0.0)]),
            y: if bloom {
                Spline::new(&[(0.0, 0.01), (1.0, -0.033)])
            } else {
                Spline::new(&[(0.0, 0.01), (0.9, -1.0 / 60.0), (1.0, 0.0)])
            },
            letter_scale: Spline::new(&[
                (0.0, 0.95),
                (0.7, if bloom { 1.07 } else { 1.175 }),
                (1.0, 1.0),
            ]),
            letter_y: Spline::new(&[
                (0.0, 0.01),
                (0.9, if bloom { -1.0 / 62.0 } else { -1.0 / 56.0 }),
                (1.0, 0.0),
            ]),
            dot_scale: Spline::new(&[(0.0, 0.75), (0.7, 1.05), (1.0, 1.0)]),
            dot_y: Spline::new(&[(0.0, 0.0), (0.9, -0.12), (1.0, 0.0)]),
            dot_glow: Spline::new(&[(0.0, 0.0), (0.6, 1.0), (1.0, 1.0)]),
            dot_opacity: Spline::new(&[
                (0.0, if bloom { 0.27 } else { 0.35 }),
                (0.6, 1.0),
                (1.0, 1.0),
            ]),
            line_glow: Spline::new(&[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)]),
        }
    }
}

struct Trio {
    scale: Spring,
    y: Spring,
    glow: Spring,
}

impl Trio {
    fn new(scale: f32, y: f32, glow: f32) -> Self {
        Self {
            scale: Spring::new(scale, SCALE_FREQUENCY, SCALE_DAMPING),
            y: Spring::new(y, Y_FREQUENCY, Y_DAMPING),
            glow: Spring::new(glow, GLOW_FREQUENCY, GLOW_DAMPING),
        }
    }
    fn aim(&mut self, scale: f32, y: f32, glow: f32, dt: f32) -> (f32, f32, f32) {
        self.scale.set_goal(scale);
        self.y.set_goal(y);
        self.glow.set_goal(glow);
        (self.scale.step(dt), self.y.step(dt), self.glow.step(dt))
    }
}

struct Dot {
    scale: Spring,
    y: Spring,
    glow: Spring,
    opacity: Spring,
}

/// Bloom's measurements of one line, for its single fill edge.
#[derive(Clone, Debug)]
pub struct Sweep {
    pub widths: Vec<f32>,
    pub gap: f32,
    pub letter_widths: Vec<Option<Vec<f32>>>,
    pub font_px: f32,
    pub rows: Vec<usize>,
}

pub struct Animator {
    pub config: Config,
    pub splines: Splines,
    words: HashMap<(usize, usize), Trio>,
    letters: HashMap<(usize, usize, usize), Trio>,
    dots: HashMap<(usize, usize), Dot>,
    line_glows: HashMap<usize, Spring>,
    opacity: HashMap<usize, Tween>,
    scale: HashMap<usize, Tween>,
    tone: HashMap<usize, Tween>,
    presence: HashMap<usize, Tween>,
    pop_fade: HashMap<usize, Tween>,
    cached: HashMap<usize, Vec<WordAnim>>,
    line_gradient: HashMap<usize, f32>,
    line_glow: HashMap<usize, f32>,
    bloom_final: HashMap<usize, (f64, Vec<WordAnim>)>,
    blurring: Option<usize>,
    blur: Vec<f32>,
}

impl Animator {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            splines: Splines::new(config.bloom),
            words: HashMap::new(),
            letters: HashMap::new(),
            dots: HashMap::new(),
            line_glows: HashMap::new(),
            opacity: HashMap::new(),
            scale: HashMap::new(),
            tone: HashMap::new(),
            presence: HashMap::new(),
            pop_fade: HashMap::new(),
            cached: HashMap::new(),
            line_gradient: HashMap::new(),
            line_glow: HashMap::new(),
            bloom_final: HashMap::new(),
            blurring: None,
            blur: Vec::new(),
        }
    }

    /// Every line's pose at `time_ms`, `dt` seconds after the last frame;
    /// `now` is the UI clock in seconds, for the tweens.
    #[allow(clippy::too_many_arguments)]
    pub fn animate(
        &mut self,
        lines: &[TLine],
        kind: Kind,
        time_ms: f64,
        dt: f32,
        now: f64,
        hide_blur: bool,
        scroll_target: Option<usize>,
        sweep: &dyn Fn(usize) -> Option<Sweep>,
        reduced: bool,
    ) -> Vec<LineAnim> {
        if kind == Kind::Static {
            return lines
                .iter()
                .map(|_| LineAnim {
                    opacity: 1.0,
                    blur: 0.0,
                    scale: 1.0,
                    active: false,
                    state: State::NotSung,
                    words: Vec::new(),
                    line_gradient: 100.0,
                    line_glow: 0.0,
                    lit: 0.0,
                    presence: 1.0,
                    presence_goal: 1.0,
                    group_scale: 1.0,
                })
                .collect();
        }
        let config = self.config;
        // Less motion: every spring reaches its goal within this frame.
        let dt = if reduced { 1.0 } else { dt };
        let t = time_ms
            - if config.bloom {
                bloom::POSITION_SHIFT_MS
            } else {
                0.0
            };
        if self.blur.len() != lines.len() {
            self.blur = vec![0.0; lines.len()];
        }
        let states: Vec<State> = lines.iter().map(|l| state(t, l.start, l.end)).collect();
        let minimal_look = config.minimal && !hide_blur;
        let all_sung = states.iter().all(|s| *s == State::Sung);
        let first_active = states.iter().position(|s| *s == State::Active);

        // Distance blur, worked out again only when the active lead changes,
        // before any line reads it.
        if let Some(i) =
            (0..lines.len()).find(|&i| states[i] == State::Active && !lines[i].is_background())
            && self.blurring != Some(i)
        {
            for (j, other) in lines.iter().enumerate() {
                let distance = j.abs_diff(i);
                self.blur[j] = if state(t, other.start, other.end) == State::Active || distance == 0
                {
                    0.0
                } else {
                    (BLUR_MULTIPLIER * distance as f32).min(MAX_BLUR)
                };
            }
            self.blurring = Some(i);
        }
        let mut out = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            let line_kind = if line.line_timed && kind == Kind::Syllable {
                Kind::Line
            } else {
                kind
            };
            let line_state = states[i];
            let active = line_state == State::Active;
            let finishing = line_state == State::Sung
                && (lines
                    .get(i + 1)
                    .is_none_or(|next| state(t, next.start, next.end) != State::Sung)
                    || !settled(self.cached.get(&i)));
            let kept_after = config.keep_overlapping
                && line_state == State::Sung
                && !line.is_interlude()
                && first_active.is_some_and(|first| first < i);
            let after_active =
                kept_after || !config.keep_overlapping && i > 0 && states[i - 1] == State::Active;
            let opacity = self.line_opacity(
                i,
                line,
                line_state,
                line_kind,
                minimal_look,
                all_sung,
                after_active,
                now,
                reduced,
            );
            let lit = if config.bloom && !line.is_interlude() {
                self.bloom_tone(i, active, line_kind, now, reduced)
            } else {
                0.0
            };
            let scale = if line.is_interlude() {
                let pre_hidden = t > line.end - timeline::PRE_HIDDEN_DOT_LINE_MS;
                let target = if active && !pre_hidden { 1.0 } else { 0.0 };
                let tween = self.scale.entry(i).or_insert_with(|| Tween::new(target));
                if target == 0.0 {
                    aim(tween, 0.0, now, 400, 0, Ease::DotCollapse, reduced);
                } else {
                    aim(tween, 1.0, now, 300, 0, SCALE_EASE, reduced);
                }
                tween.value(now)
            } else if config.minimal {
                let target = if active || all_sung || !minimal_look || kept_after {
                    1.0
                } else if line_state == State::Sung {
                    0.95
                } else {
                    0.965
                };
                self.tween_scale(i, target, now, 400, EASE_IN_OUT, reduced)
            } else if config.bloom && !line.is_background() {
                let focused = active || scroll_target == Some(i);
                let tween = self.scale.entry(i).or_insert_with(|| {
                    Tween::new(if focused { 1.0 } else { bloom::INACTIVE_SCALE })
                });
                if focused {
                    aim(
                        tween,
                        1.0,
                        now,
                        bloom::SCALE_IN_MS,
                        bloom::SCALE_IN_DELAY_MS,
                        Ease::Bezier(bloom::SCALE_EASE),
                        reduced,
                    );
                } else {
                    aim(
                        tween,
                        bloom::INACTIVE_SCALE,
                        now,
                        bloom::SCALE_OUT_MS,
                        bloom::SCALE_OUT_DELAY_MS,
                        Ease::Bezier(bloom::SCALE_EASE),
                        reduced,
                    );
                }
                tween.value(now)
            } else if config.bloom {
                1.0
            } else if line_kind == Kind::Line {
                self.tween_scale(
                    i,
                    if active { ACTIVE_LINE_SCALE } else { 1.0 },
                    now,
                    200,
                    SCALE_EASE,
                    reduced,
                )
            } else {
                1.0
            };

            let mut gradient = self.line_gradient.get(&i).copied().unwrap_or(-20.0);
            let mut glow = self.line_glow.get(&i).copied().unwrap_or(0.0);
            let words = if line_kind == Kind::Line && !line.is_interlude() {
                match line_state {
                    State::Active => {
                        let pct = progress(t, line.start, line.end);
                        if config.bloom {
                            gradient = 100.0;
                            glow = 0.0;
                        } else {
                            gradient = pct * 100.0;
                            let spring = self
                                .line_glows
                                .entry(i)
                                .or_insert_with(|| Spring::new(0.0, 1.0, 0.5));
                            spring.set_goal(self.splines.line_glow.at(pct));
                            glow = spring.step(dt);
                        }
                    }
                    State::NotSung => gradient = if config.bloom { 100.0 } else { -20.0 },
                    State::Sung => gradient = 100.0,
                }
                self.line_gradient.insert(i, gradient);
                self.line_glow.insert(i, glow);
                Vec::new()
            } else if config.bloom {
                if line.is_interlude() {
                    self.bloom_dots(line, t)
                } else {
                    let at = if line_state == State::NotSung {
                        f64::NEG_INFINITY
                    } else {
                        t
                    };
                    self.bloom_line(i, &line.words, at, line.end, sweep(i))
                }
            } else if active {
                let states = if line.is_interlude() {
                    self.dots(i, line, t, dt, false)
                } else {
                    (0..line.words.len())
                        .map(|w| self.word(i, w, &line.words[w], t, dt))
                        .collect()
                };
                self.cached.insert(i, states.clone());
                states
            } else if finishing {
                let states = if line.is_interlude() {
                    self.dots(i, line, t, dt, true)
                } else {
                    (0..line.words.len())
                        .map(|w| self.finalize(i, w, &line.words[w], dt))
                        .collect()
                };
                self.cached.insert(i, states.clone());
                states
            } else {
                if line_state == State::NotSung
                    && self
                        .cached
                        .get(&i)
                        .is_some_and(|c| c.iter().any(|w| w.state != State::NotSung))
                {
                    self.forget(i);
                }
                let splines = &self.splines;
                self.cached
                    .entry(i)
                    .or_insert_with(|| {
                        line.words
                            .iter()
                            .map(|w| WordAnim::rest(w, splines))
                            .collect()
                    })
                    .clone()
            };

            let blur = if !config.blur || hide_blur || active || minimal_look && kept_after {
                0.0
            } else {
                self.blur.get(i).copied().unwrap_or(0.0)
            };
            let (presence, presence_goal, presence_alpha) =
                self.pop(lines, &states, i, now, reduced);
            let group_scale = if config.bloom && line.is_interlude() {
                bloom::dot_group(
                    t - line.start,
                    line.duration(),
                    timeline::PRE_HIDDEN_DOT_LINE_MS,
                )
                .0
            } else {
                1.0
            };
            out.push(LineAnim {
                opacity: opacity * presence_alpha,
                blur,
                scale: if presence == 1.0 {
                    scale
                } else {
                    scale * (POP_FOLDED_SCALE + (1.0 - POP_FOLDED_SCALE) * presence)
                },
                active,
                state: line_state,
                words,
                line_gradient: gradient,
                line_glow: glow,
                lit,
                presence,
                presence_goal,
                group_scale,
            });
        }
        out
    }

    /// Whether any tween is still running, so frames must keep coming.
    pub fn tweening(&self, now: f64) -> bool {
        [
            &self.opacity,
            &self.scale,
            &self.tone,
            &self.presence,
            &self.pop_fade,
        ]
        .iter()
        .any(|map| map.values().any(|t| t.running(now)))
    }

    #[allow(clippy::too_many_arguments)]
    fn line_opacity(
        &mut self,
        i: usize,
        line: &TLine,
        line_state: State,
        kind: Kind,
        minimal_look: bool,
        all_sung: bool,
        after_active: bool,
        now: f64,
        reduced: bool,
    ) -> f32 {
        let config = self.config;
        let target = if line.is_interlude() {
            if line_state == State::Active {
                1.0
            } else {
                0.0
            }
        } else if minimal_look && all_sung {
            config.opacity_sung()
        } else if minimal_look {
            match line_state {
                State::Active => 1.0,
                State::NotSung => {
                    if config.bloom {
                        1.0
                    } else {
                        0.5
                    }
                }
                State::Sung => {
                    if !after_active {
                        0.0
                    } else if config.bloom {
                        1.0
                    } else {
                        0.45
                    }
                }
            }
        } else {
            match line_state {
                State::Active => 1.0,
                State::NotSung => config.opacity_not_sung(),
                State::Sung => config.opacity_sung(),
            }
        };
        let tween = self.opacity.entry(i).or_insert_with(|| Tween::new(target));
        let (ms, ease) = if line.is_interlude() {
            (140, OPACITY_EASE)
        } else if config.minimal {
            (400, LINEAR)
        } else if kind == Kind::Line {
            (200, SCALE_EASE)
        } else {
            (200, OPACITY_EASE)
        };
        aim(tween, target, now, ms, 0, ease, reduced);
        tween.value(now)
    }

    fn tween_scale(
        &mut self,
        i: usize,
        target: f32,
        now: f64,
        ms: u32,
        ease: Ease,
        reduced: bool,
    ) -> f32 {
        let tween = self.scale.entry(i).or_insert_with(|| Tween::new(target));
        aim(tween, target, now, ms, 0, ease, reduced);
        tween.value(now)
    }

    fn bloom_tone(&mut self, i: usize, active: bool, kind: Kind, now: f64, reduced: bool) -> f32 {
        let target = if active { 1.0 } else { 0.0 };
        let tween = self.tone.entry(i).or_insert_with(|| Tween::new(target));
        if tween.target() != target {
            let line_synced = kind == Kind::Line;
            if target == 1.0 && !line_synced {
                tween.snap(1.0);
            } else if target == 1.0 {
                aim(
                    tween,
                    1.0,
                    now,
                    bloom::LINE_LIGHT_MS,
                    0,
                    Ease::Bezier(bloom::LINE_TONE_EASE),
                    reduced,
                );
            } else if line_synced {
                aim(
                    tween,
                    0.0,
                    now,
                    bloom::LINE_DIM_MS,
                    0,
                    Ease::Bezier(bloom::LINE_TONE_EASE),
                    reduced,
                );
            } else {
                aim(
                    tween,
                    0.0,
                    now,
                    bloom::DIM_MS,
                    bloom::DIM_DELAY_MS,
                    Ease::Bezier(bloom::DIM_EASE),
                    reduced,
                );
            }
        }
        tween.value(now)
    }

    fn pop(
        &mut self,
        lines: &[TLine],
        states: &[State],
        i: usize,
        now: f64,
        reduced: bool,
    ) -> (f32, f32, f32) {
        let line = &lines[i];
        if !self.config.background_pop || !line.is_background() {
            return (1.0, 1.0, 1.0);
        }
        let lead = lines
            .iter()
            .position(|l| l.group == line.group && l.role == Role::Lead);
        let sung = states[i] == State::Active || lead.is_some_and(|l| states[l] == State::Active);
        let moved_on = states
            .iter()
            .enumerate()
            .skip(i + 1)
            .any(|(j, s)| *s == State::Active && lines[j].group != line.group);
        let kept = !sung
            && self.presence.get(&i).is_some_and(|t| t.target() == 1.0)
            && states[i] == State::Sung
            && lead.is_none_or(|l| states[l] == State::Sung)
            && !moved_on;
        let target = if sung || kept { 1.0 } else { 0.0 };
        let presence = self.presence.entry(i).or_insert_with(|| Tween::new(target));
        let fade = self.pop_fade.entry(i).or_insert_with(|| Tween::new(target));
        if presence.target() != target {
            aim(presence, target, now, 300, 0, POP_EASE, reduced);
            if target == 1.0 {
                fade.snap(1.0);
            } else {
                aim(fade, 0.0, now, 120, 0, POP_FADE_EASE, reduced);
            }
        }
        let value = presence.value(now);
        let alpha = if target == 1.0 {
            let x = ((value - 0.3) / 0.7).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        } else {
            fade.value(now)
        };
        (value, target, alpha)
    }

    fn trio(&mut self, key: (usize, usize)) -> &mut Trio {
        let s = &self.splines;
        let (scale, y) = (s.scale.at(0.0), s.y.at(0.0));
        self.words
            .entry(key)
            .or_insert_with(|| Trio::new(scale, y, 0.0))
    }

    fn word(&mut self, line: usize, index: usize, word: &Word, t: f64, dt: f32) -> WordAnim {
        let word_state = state(t, word.start, word.end);
        let pct = progress(t, word.start, word.end);
        let s = &self.splines;
        let (scale, y, glow, gradient) = match word_state {
            State::Active => (
                s.scale.at(pct),
                s.y.at(pct),
                s.glow.at(pct),
                -20.0 + 120.0 * pct,
            ),
            State::NotSung => (s.scale.at(0.0), s.y.at(0.0), s.glow.at(0.0), -20.0),
            State::Sung => (s.scale.at(1.0), s.y.at(1.0), s.glow.at(1.0), 100.0),
        };
        let (scale, y, glow) = self.trio((line, index)).aim(scale, y, glow, dt);
        let letters = if word.is_letter_group() {
            self.letters_of(line, index, word, word_state, t, dt)
        } else {
            Vec::new()
        };
        WordAnim {
            scale,
            y,
            glow,
            gradient,
            state: word_state,
            dot_glow: 0.0,
            letters,
        }
    }

    fn letter_trio(&mut self, key: (usize, usize, usize)) -> &mut Trio {
        let s = &self.splines;
        let (scale, y) = (s.letter_scale.at(0.0), s.letter_y.at(0.0));
        self.letters
            .entry(key)
            .or_insert_with(|| Trio::new(scale, y, 0.0))
    }

    fn letters_of(
        &mut self,
        line: usize,
        index: usize,
        word: &Word,
        word_state: State,
        t: f64,
        dt: f32,
    ) -> Vec<LetterAnim> {
        if word_state != State::Active {
            let at = if word_state == State::NotSung {
                0.0
            } else {
                1.0
            };
            let gradient = if word_state == State::NotSung {
                -20.0
            } else {
                100.0
            };
            return (0..word.letters.len())
                .map(|k| {
                    let s = &self.splines;
                    let goal = (s.letter_scale.at(at), s.letter_y.at(at), s.glow.at(at));
                    let (scale, y, glow) = self
                        .letter_trio((line, index, k))
                        .aim(goal.0, goal.1, goal.2, dt);
                    LetterAnim {
                        gradient,
                        scale,
                        y,
                        glow,
                        ..LetterAnim::default()
                    }
                })
                .collect();
        }
        let mut active = None;
        for (k, letter) in word.letters.iter().enumerate() {
            if state(t, letter.start, letter.end) == State::Active {
                active = Some((k, progress(t, letter.start, letter.end)));
                break;
            }
        }
        (0..word.letters.len())
            .map(|k| {
                let letter = &word.letters[k];
                let letter_state = state(t, letter.start, letter.end);
                let s = &self.splines;
                let rest = (s.letter_scale.at(0.0), s.letter_y.at(0.0), s.glow.at(0.0));
                let mut goal = rest;
                if let Some((a, pct)) = active {
                    let d = k.abs_diff(a) as f32;
                    let falloff = 1.0 / (1.0 + d.powf(2.8));
                    let glow_falloff = 1.0 / (1.0 + d * 0.9);
                    goal = (
                        rest.0 + (s.letter_scale.at(pct) - rest.0) * falloff,
                        rest.1 + (s.letter_y.at(pct) - rest.1) * falloff,
                        rest.2 + (s.glow.at(pct) - rest.2) * glow_falloff,
                    );
                }
                if letter_state == State::NotSung {
                    goal = rest;
                } else if letter_state == State::Sung && active.is_none() {
                    goal.2 = s.glow.at(SUNG_LETTER_GLOW);
                }
                let gradient = match letter_state {
                    State::NotSung => -20.0,
                    State::Sung => 100.0,
                    State::Active => match active {
                        Some((a, pct)) if a == k => -20.0 + 120.0 * ease_sin_out(pct),
                        _ => -20.0,
                    },
                };
                let (scale, y, glow) = self
                    .letter_trio((line, index, k))
                    .aim(goal.0, goal.1, goal.2, dt);
                LetterAnim {
                    gradient,
                    scale,
                    y,
                    glow,
                    ..LetterAnim::default()
                }
            })
            .collect()
    }

    fn finalize(&mut self, line: usize, index: usize, word: &Word, dt: f32) -> WordAnim {
        let s = &self.splines;
        let goal = (s.scale.at(1.0), s.y.at(1.0), s.glow.at(1.0));
        let letter_goal = (s.letter_scale.at(1.0), s.letter_y.at(1.0), s.glow.at(1.0));
        let (scale, y, glow) = self.trio((line, index)).aim(goal.0, goal.1, goal.2, dt);
        let letters = (0..word.letters.len())
            .map(|k| {
                let (scale, y, glow) = self.letter_trio((line, index, k)).aim(
                    letter_goal.0,
                    letter_goal.1,
                    letter_goal.2,
                    dt,
                );
                LetterAnim {
                    gradient: 100.0,
                    scale,
                    y,
                    glow,
                    ..LetterAnim::default()
                }
            })
            .collect();
        WordAnim {
            scale,
            y,
            glow,
            gradient: 100.0,
            state: State::Sung,
            dot_glow: 0.0,
            letters,
        }
    }

    fn dots(
        &mut self,
        line_index: usize,
        line: &TLine,
        t: f64,
        dt: f32,
        finalize: bool,
    ) -> Vec<WordAnim> {
        let span = (line.duration() - 550.0).max(90.0) / 3.0;
        (0..3)
            .map(|d| {
                let start = line.start + (d as f64 * span).floor();
                let end = line.start + ((d + 1) as f64 * span).floor();
                let dot_state = if finalize {
                    State::Sung
                } else {
                    state(t, start, end)
                };
                let pct = progress(t, start, end);
                let s = &self.splines;
                let sample = |spline: &Spline| match dot_state {
                    State::Active => spline.at(pct),
                    State::NotSung => spline.at(0.0),
                    State::Sung => spline.at(1.0),
                };
                let goals = (
                    sample(&s.dot_scale),
                    sample(&s.dot_y),
                    sample(&s.dot_glow),
                    sample(&s.dot_opacity),
                );
                let rest = (
                    s.dot_scale.at(0.0),
                    s.dot_y.at(0.0),
                    s.dot_glow.at(0.0),
                    s.dot_opacity.at(0.0),
                );
                let dot = self.dots.entry((line_index, d)).or_insert_with(|| Dot {
                    scale: Spring::new(rest.0, 0.7, 0.6),
                    y: Spring::new(rest.1, 1.25, 0.4),
                    glow: Spring::new(rest.2, 1.0, 0.5),
                    opacity: Spring::new(rest.3, 1.0, 0.5),
                });
                dot.scale.set_goal(goals.0);
                dot.y.set_goal(goals.1);
                dot.glow.set_goal(goals.2);
                dot.opacity.set_goal(goals.3);
                WordAnim {
                    scale: dot.scale.step(dt),
                    y: dot.y.step(dt),
                    glow: dot.opacity.step(dt),
                    dot_glow: dot.glow.step(dt),
                    gradient: if dot_state == State::Sung { 100.0 } else { 0.0 },
                    state: dot_state,
                    letters: Vec::new(),
                }
            })
            .collect()
    }

    fn bloom_dots(&self, line: &TLine, t: f64) -> Vec<WordAnim> {
        let elapsed = t - line.start;
        let (_, group_opacity) =
            bloom::dot_group(elapsed, line.duration(), timeline::PRE_HIDDEN_DOT_LINE_MS);
        let dot_state = state(t, line.start, line.end);
        (0..3)
            .map(|d| WordAnim {
                scale: 1.0,
                y: 0.0,
                glow: bloom::dot_opacity(
                    elapsed,
                    line.duration(),
                    d,
                    3,
                    timeline::PRE_HIDDEN_DOT_LINE_MS,
                ) * group_opacity,
                dot_glow: 0.0,
                gradient: 0.0,
                state: dot_state,
                letters: Vec::new(),
            })
            .collect()
    }

    fn bloom_line(
        &mut self,
        key: usize,
        words: &[Word],
        t: f64,
        end: f64,
        geometry: Option<Sweep>,
    ) -> Vec<WordAnim> {
        if t < end {
            self.bloom_final.remove(&key);
        } else if let Some((from, states)) = self.bloom_final.get(&key)
            && t >= *from
        {
            return states.clone();
        }
        let n = words.len();
        let fits = geometry
            .as_ref()
            .is_some_and(|g| g.widths.len() == n && g.font_px > 0.0);
        let font_px = geometry.as_ref().map_or(0.0, |g| g.font_px);
        let fade = if fits { font_px * bloom::FADE_EM } else { 0.0 };
        let run_starts = timeline::run_starts(words);
        let is_group = |i: usize| words[i].is_letter_group();
        let mut x = vec![0.0_f32; n];
        let mut spans: Vec<Option<bloom::Span>> = vec![None; n];
        if let (true, Some(g)) = (fits, geometry.as_ref()) {
            let boxes: Vec<f32> = (0..n)
                .map(|i| {
                    g.widths[i]
                        + if i + 1 < n && !words[i + 1].glued {
                            g.gap
                        } else {
                            0.0
                        }
                })
                .collect();
            let mut run = 0.0;
            for i in 0..n {
                x[i] = run;
                run += boxes[i];
            }
            let mut word_letters = vec![0usize; n];
            for i in 0..n {
                word_letters[run_starts[i]] += timeline::graphemes(&words[i].text).len();
            }
            let mut prev: Option<usize> = None;
            for i in 0..n {
                if boxes[i] <= 0.0 {
                    continue;
                }
                let row = g.rows.get(i).copied().unwrap_or(0);
                let row_start = prev.is_none_or(|p| row > g.rows.get(p).copied().unwrap_or(0));
                let row_end = i == n - 1 || g.rows.get(i + 1).copied().unwrap_or(0) > row;
                spans[i] = Some(bloom::Span {
                    x: x[i],
                    w: boxes[i],
                    reach: bloom::reach(word_letters[run_starts[i]], is_group(i), row_end),
                    row_start,
                    row_end,
                });
                prev = Some(i);
            }
        }
        let letter_x = |i: usize| -> Option<&Vec<f32>> {
            geometry
                .as_ref()
                .and_then(|g| g.letter_widths.get(i))
                .and_then(Option::as_ref)
                .filter(|w| fits && w.len() == words[i].letters.len())
        };
        let starts: Vec<f64> = words.iter().map(|w| w.start).collect();
        let ends: Vec<f64> = words.iter().map(|w| w.end).collect();
        let edges = if fits {
            let pace = self.config.pace;
            bloom::sweep_edges(&spans, &starts, &ends, fade, t, |i| {
                let widths = letter_x(i)?;
                if !pace || !is_group(i) {
                    return None;
                }
                let mut xs = vec![0.0; widths.len()];
                for j in 1..widths.len() {
                    xs[j] = xs[j - 1] + widths[j - 1];
                }
                let letters = &words[i].letters;
                bloom::paced_share(
                    &letters.iter().map(|l| l.start).collect::<Vec<_>>(),
                    &letters.iter().map(|l| l.end).collect::<Vec<_>>(),
                    &xs,
                    widths,
                    t,
                )
            })
        } else {
            vec![f64::NEG_INFINITY; n]
        };
        let ended = t >= end;
        let lift_left = if ended {
            1.0 - bloom::lift(t - end)
        } else {
            1.0
        };
        let mut flames: HashMap<usize, (bloom::Flame, Vec<usize>)> = HashMap::new();
        let mut settled = ended
            && lift_left == 0.0
            && t >= words.iter().map(|w| w.end).fold(0.0, f64::max) + bloom::RUSH_MS;
        let mut float_cap = 1.0_f64;
        let mut states = Vec::with_capacity(n);
        for i in 0..n {
            let word = &words[i];
            let word_state = state(t, word.start, word.end);
            let width = if fits {
                geometry.as_ref().map_or(0.0, |g| g.widths[i])
            } else {
                0.0
            };
            let position = if spans[i].is_some() {
                bloom::gradient_position(edges[i], x[i], width, fade)
            } else {
                -50.0 + 150.0 * progress(t, word.start, word.end)
            };
            if !is_group(i) {
                float_cap = float_cap.min(bloom::lift(t - word.start));
                states.push(WordAnim {
                    scale: 1.0,
                    y: (-f64::from(bloom::LIFT_EM) * float_cap * lift_left) as f32,
                    glow: 0.0,
                    gradient: position,
                    state: word_state,
                    dot_glow: 0.0,
                    letters: Vec::new(),
                });
                continue;
            }
            let first = run_starts[i];
            let (whole, offsets) = flames.entry(first).or_insert_with(|| {
                let mut last = first;
                while last + 1 < n && run_starts[last + 1] == first {
                    last += 1;
                }
                let members: Vec<usize> = (first..=last)
                    .filter(|m| words[*m].is_letter_group())
                    .collect();
                let mut offsets = vec![0usize; n];
                let mut offset = 0;
                let mut widths = Vec::new();
                for &m in &members {
                    offsets[m] = offset;
                    offset += words[m].letters.len();
                    let w = geometry
                        .as_ref()
                        .and_then(|g| g.letter_widths.get(m))
                        .and_then(Option::as_ref)
                        .filter(|w| fits && w.len() == words[m].letters.len());
                    for k in 0..words[m].letters.len() {
                        widths.push(
                            w.map_or(0.0, |w| if font_px > 0.0 { w[k] / font_px } else { 0.0 }),
                        );
                    }
                }
                let head = &words[members[0]];
                let start = head.letters[0].start;
                let last_end = members
                    .iter()
                    .map(|m| words[*m].end)
                    .fold(f64::MIN, f64::max);
                (
                    bloom::flame(t - start, last_end - head.start, offset, &widths),
                    offsets,
                )
            });
            let flame = whole.slice(offsets[i], word.letters.len());
            if whole.rest != Some(1) {
                settled = false;
            }
            let widths = letter_x(i).cloned();
            let mut lx = 0.0;
            let letters = (0..word.letters.len())
                .map(|k| {
                    let position = if let Some(w) = &widths {
                        let p = bloom::gradient_position(edges[i], x[i] + lx, w[k], fade);
                        lx += w[k];
                        p
                    } else {
                        let l = &word.letters[k];
                        -50.0 + 150.0 * progress(t, l.start, l.end)
                    };
                    LetterAnim {
                        gradient: position,
                        scale: flame.scale(k),
                        y: (f64::from(flame.y(k)) * lift_left) as f32,
                        x: flame.x(k),
                        glow: flame.glow(k) * bloom::letter_tone(position),
                        glow_radius: bloom::GLOW_RADIUS_EM,
                    }
                })
                .collect();
            states.push(WordAnim {
                scale: 1.0,
                y: 0.0,
                glow: 0.0,
                gradient: position,
                state: word_state,
                dot_glow: 0.0,
                letters,
            });
        }
        if settled {
            self.bloom_final.insert(key, (t, states.clone()));
        }
        states
    }

    fn forget(&mut self, line: usize) {
        self.cached.remove(&line);
        self.line_gradient.remove(&line);
        self.line_glow.remove(&line);
        self.line_glows.remove(&line);
        self.words.retain(|(l, _), _| *l != line);
        self.letters.retain(|(l, _, _), _| *l != line);
        self.dots.retain(|(l, _), _| *l != line);
        self.bloom_final.remove(&line);
    }
}

fn settled(states: Option<&Vec<WordAnim>>) -> bool {
    states.is_some_and(|states| {
        states.iter().all(|w| {
            w.gradient >= 99.5
                && w.glow < 0.02
                && w.letters
                    .iter()
                    .all(|l| l.gradient >= 99.5 && l.glow < 0.02)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, end: f64, words: &[(f64, f64)]) -> TLine {
        TLine {
            words: words
                .iter()
                .map(|(s, e)| Word {
                    text: "la".into(),
                    start: *s,
                    end: *e,
                    glued: false,
                    letters: Vec::new(),
                })
                .collect(),
            start,
            end,
            role: Role::Lead,
            group: 0,
            opposite: false,
            line_timed: false,
            text: String::new(),
            notes: Vec::new(),
        }
    }

    fn spring_config() -> Config {
        Config {
            bloom: false,
            minimal: false,
            blur: true,
            glow: true,
            background_pop: false,
            keep_overlapping: true,
            pace: false,
        }
    }

    #[test]
    fn lines_rest_at_spicy_opacities_and_blur_with_distance() {
        let lines = vec![
            line(0.0, 1000.0, &[(0.0, 1000.0)]),
            line(1000.0, 2000.0, &[(1000.0, 2000.0)]),
            line(2000.0, 3000.0, &[(2000.0, 3000.0)]),
            line(3000.0, 4000.0, &[(3000.0, 4000.0)]),
        ];
        let mut animator = Animator::new(spring_config());
        let states = animator.animate(
            &lines,
            Kind::Syllable,
            1500.0,
            1.0 / 60.0,
            0.0,
            false,
            None,
            &|_| None,
            false,
        );
        assert!(states[1].active);
        assert_eq!(states[1].blur, 0.0);
        assert_eq!(states[0].blur, 1.25);
        assert_eq!(states[3].blur, 2.5);
        assert!((states[2].opacity - 0.51).abs() < 1e-6);
        // Half way through the word, the fill is at -20 + 60.
        assert!((states[1].words[0].gradient - 40.0).abs() < 0.01);
        // Unsung words wait at the rest pose.
        assert!((states[2].words[0].scale - 0.95).abs() < 1e-6);
    }

    #[test]
    fn the_word_springs_follow_the_spline() {
        let lines = vec![line(0.0, 2000.0, &[(0.0, 2000.0)])];
        let mut animator = Animator::new(spring_config());
        let mut peak = 0.0_f32;
        for frame in 0..120 {
            let t = frame as f64 * 1000.0 / 60.0;
            let states = animator.animate(
                &lines,
                Kind::Syllable,
                t,
                1.0 / 60.0,
                t / 1000.0,
                false,
                None,
                &|_| None,
                false,
            );
            peak = peak.max(states[0].words[0].scale);
        }
        assert!(peak > 1.0 && peak < 1.07, "{peak}");
    }

    /// With less motion asked for, a line takes its dimmed or lit place in
    /// the frame it changes instead of easing there over 200 ms.
    #[test]
    fn less_motion_settles_a_line_at_once() {
        let lines = vec![
            line(0.0, 1000.0, &[(0.0, 1000.0)]),
            line(1000.0, 2000.0, &[(1000.0, 2000.0)]),
        ];
        let mut animator = Animator::new(spring_config());
        let states = animator.animate(
            &lines,
            Kind::Syllable,
            1200.0,
            1.0 / 60.0,
            0.0,
            false,
            None,
            &|_| None,
            true,
        );
        assert!(
            (states[0].opacity - 0.497).abs() < 1e-6,
            "the sung line is already dim: {}",
            states[0].opacity
        );
        assert!(
            (states[1].opacity - 1.0).abs() < 1e-6,
            "the active line is already lit: {}",
            states[1].opacity
        );
    }
}
