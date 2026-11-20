//! One lyrics view: the timeline, its layout, the animator and the scroll,
//! kept between frames, and painted on the graphics card. A port of Spicy
//! Lyrics Mobile's `LyricsView`.

use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};

use egui::emath::TSTransform;
use egui::{Rect, Sense, pos2, vec2};

use super::animator::{self, Animator, Config, LineAnim, State};
use super::bloom;
use super::gpu::{Batch, Fill};
use super::layout::{self, LineLayout, Metrics};
use super::scroll::{Manager, Motion, Policy, Ripple};
use super::timeline::{self, BuildOptions, Kind, Timeline};
use crate::app::App;
use crate::lyrics::Lyrics;
use crate::model::Action;
use crate::theme::{self, Icon};

/// Where the line being sung sits.
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Its centre this far above the middle; the first line never rests
    /// above `rest_top` of the height.
    Center { lift: f32, rest_top: f32 },
    /// Its top this far below the view's top.
    Top(f32),
}

#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// The lead lines' size, Lyrics Size included.
    pub font: f32,
    pub pad_left: f32,
    pub pad_right: f32,
    pub anchor: Anchor,
    /// The edge fade: transparent for `.0` points, opaque from `.1`.
    pub mask: (f32, f32),
    /// Fade Away Previous Lines applies here.
    pub minimal: bool,
}

struct Stage {
    key: u64,
    layout_key: u64,
    timeline: Timeline,
    layouts: Vec<LineLayout>,
    animator: Animator,
    policy: Policy,
    manager: Manager,
    ripple: Ripple,
    anims: Vec<LineAnim>,
    offsets: Vec<f32>,
    footer: f32,
    center: f32,
    last_time: f64,
    still_since: f64,
    atlas_fill: f32,
    rippling: bool,
}

fn lyrics_key(lyrics: &Lyrics, uri: &str, options: &BuildOptions, config: &Config) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut hasher);
    lyrics.synced.hash(&mut hasher);
    for line in &lyrics.lines {
        line.at_ms.hash(&mut hasher);
        line.end_ms.hash(&mut hasher);
        line.text.hash(&mut hasher);
        line.syllables.len().hash(&mut hasher);
        line.background.len().hash(&mut hasher);
        line.translated.hash(&mut hasher);
        if let Some(first) = line.syllables.first() {
            first.at_ms.hash(&mut hasher);
        }
    }
    (
        options.bloom,
        options.minimal,
        options.merge as u8,
        options.pace,
        options.notes,
    )
        .hash(&mut hasher);
    (
        config.background_pop,
        config.keep_overlapping,
        config.blur,
        config.glow,
    )
        .hash(&mut hasher);
    hasher.finish()
}

fn boosted_scale(scale: f32, boost: f32) -> f32 {
    if scale > 1.0 {
        1.0 + (scale - 1.0) * boost
    } else {
        scale
    }
}

fn boosted_lift(y: f32, boost: f32) -> f32 {
    if y < 0.0 { y * boost } else { y }
}

fn scale_about(pivot: egui::Pos2, scale: f32) -> TSTransform {
    TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale)
}

fn shift(x: f32, y: f32) -> TSTransform {
    TSTransform::from_translation(vec2(x, y))
}

/// Where a syllable grows from: the first of a split word from its right
/// edge, the last from its left, the rest and whole words from the middle.
fn anchor(glued_before: bool, glued_after: bool, rtl: bool) -> f32 {
    let ltr = if glued_after && !glued_before {
        1.0
    } else if glued_before && !glued_after {
        0.0
    } else {
        0.5
    };
    if rtl { 1.0 - ltr } else { ltr }
}

/// The colour a line not being sung is painted in: its own shadow, blurred
/// by its distance from the line being sung.
fn inactive_alpha(config: &Config, anim: &LineAnim, background: bool, dim: f32) -> f32 {
    let alpha = if config.bloom {
        let (dim, bright) = if background {
            (config.background_dim(), config.background_bright())
        } else {
            (dim, config.bright())
        };
        dim + (bright - dim) * anim.lit
    } else if background {
        if anim.state == State::NotSung {
            config.background_dim()
        } else {
            config.background_bright()
        }
    } else if anim.state == State::NotSung {
        dim
    } else {
        config.bright()
    };
    (alpha * anim.opacity).clamp(0.0, 1.0)
}

/// An inactive run's fill and shadow for a blur in points: crisp when it
/// would barely blur, else only the shadow, as the reference draws it.
fn shadow_only(alpha: f32, blur: f32) -> (Fill, Option<(f32, f32)>) {
    if blur < 0.4 {
        (Fill::Solid(alpha), None)
    } else {
        (Fill::None, Some((alpha, blur)))
    }
}

/// The horizontal wipe's fill across a word `full` points wide, for a
/// piece starting `start` into it.
fn wipe(position: f32, full: f32, start: f32, bright: f32, dim: f32, rtl: bool, fade: f32) -> Fill {
    let full = full.max(1.0);
    let (lo, hi) = if fade > 0.0 {
        bloom::wipe_stops(position, full, fade)
    } else {
        (position / 100.0, (position + 20.0) / 100.0)
    };
    let (lo, hi, left, right) = if rtl {
        (1.0 - hi, 1.0 - lo, dim, bright)
    } else {
        (lo, hi, bright, dim)
    };
    Fill::Wipe {
        along: start,
        lo: lo * full,
        hi: hi * full,
        left,
        right,
        vertical: false,
    }
}

pub struct Shown {
    /// The credits' rectangle, for the caller to draw them in.
    pub footer: Option<Rect>,
}

#[allow(clippy::too_many_lines)]
pub fn show(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    id: &str,
    lyrics: &Lyrics,
    look: Look,
) -> Shown {
    let Some(now) = app.now_playing() else {
        return Shown { footer: None };
    };
    let settings = &app.settings;
    let low = settings.lyrics_low_performance;
    let bloom_style = settings.lyrics_style == crate::settings::LyricsStyle::Bloom;
    let minimal = look.minimal && settings.lyrics_minimal_mode;
    let config = Config {
        bloom: bloom_style,
        minimal,
        blur: settings.lyrics_distance_blur && !low,
        glow: settings.lyrics_glow && !low,
        background_pop: settings.lyrics_background_pop,
        keep_overlapping: settings.lyrics_keep_overlapping_lines,
        pace: settings.lyrics_syllable_pace,
    };
    let options = BuildOptions {
        bloom: bloom_style,
        minimal,
        merge: settings.lyrics_syllable_merge,
        pace: settings.lyrics_syllable_pace,
        notes: settings.lyrics_show_translations,
    };
    let boost = if settings.lyrics_original_word_motion {
        1.0
    } else {
        1.25
    };
    let seek_early = settings.lyrics_seek_fade_compensation;
    let smooth = settings.lyrics_smooth_scroll;
    let reduced_motion = app.motion.reduced();
    let lead = f64::from(settings.lyrics_early_scroll_ms);
    let wide_duet = settings.lyrics_duet_padding;
    let time = (f64::from(now.position_ms) + f64::from(settings.lyrics_offset_ms)).max(0.0);

    let ctx = ui.ctx().clone();
    let shared: Arc<Mutex<Option<Stage>>> = ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::new(("lyrics-stage", id)), || {
            Arc::new(Mutex::new(None))
        })
        .clone()
    });
    let mut guard = shared.lock().expect("lyrics stage");
    let key = lyrics_key(lyrics, &now.uri, &options, &config);
    if guard.as_ref().is_none_or(|s| s.key != key) {
        *guard = Some(Stage {
            key,
            layout_key: 0,
            timeline: Timeline::build(lyrics, options),
            layouts: Vec::new(),
            animator: Animator::new(config),
            policy: Policy::default(),
            manager: Manager::default(),
            ripple: Ripple::default(),
            anims: Vec::new(),
            offsets: Vec::new(),
            footer: 0.0,
            center: 0.0,
            last_time: time,
            still_since: 0.0,
            atlas_fill: 0.0,
            rippling: false,
        });
    }
    let stage = guard.as_mut().expect("a stage");
    let kind = stage.timeline.kind;
    let width = rect.width();
    let height = rect.height();
    let ppp = ctx.pixels_per_point();
    let font = if kind == Kind::Static {
        layout::base_font(Kind::Static, width) * look.font
            / layout::base_font(Kind::Syllable, width)
    } else {
        look.font
    };
    let metrics = Metrics {
        font,
        spacing: settings.lyrics_line_spacing.clamp(50, 200) as f32 / 100.0,
        width,
        pad_left: look.pad_left,
        pad_right: look.pad_right,
        bloom: bloom_style,
        wide_duet,
    };
    let fill = ctx.fonts(|f| f.font_atlas_fill_ratio());
    let atlas_reset = fill + 0.01 < stage.atlas_fill;
    stage.atlas_fill = fill;
    let layout_key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            font.to_bits(),
            width.to_bits(),
            look.pad_left.to_bits(),
            look.pad_right.to_bits(),
            ppp.to_bits(),
            metrics.spacing.to_bits(),
        )
            .hash(&mut hasher);
        (bloom_style, wide_duet).hash(&mut hasher);
        hasher.finish()
    };
    // Laid out again every frame: egui rebuilds its font atlas now and then,
    // and a galley kept from before points at glyphs no longer there. Its
    // own galley cache makes this cheap.
    let relaid = stage.layout_key != 0 && (stage.layout_key != layout_key || atlas_reset);
    stage.layouts = layout::layout(ui.painter(), &stage.timeline.lines, kind, &metrics);
    stage.layout_key = layout_key;
    if relaid {
        stage.manager.relayout();
    }

    let now_s = ui.input(|i| i.time);
    let dt = ui.input(|i| i.unstable_dt).clamp(0.0, 1.0 / 30.0);
    let center = match look.anchor {
        Anchor::Center { lift, .. } => height * 0.5 - lift,
        Anchor::Top(top) => top,
    };
    stage.center = center;
    stage.manager.top = match look.anchor {
        Anchor::Center { rest_top, .. } => (height * rest_top - center).min(0.0),
        Anchor::Top(_) => 0.0,
    };
    stage.manager.smooth = smooth;
    stage.manager.rows = bloom_style;
    stage.manager.per_reference = metrics.font / 56.0;

    let lines = &stage.timeline.lines;
    let layouts = &stage.layouts;
    let gap = metrics.line_gap();
    let row_height = metrics.line_height(metrics.font);

    let timed_scroll = bloom_style && !minimal;
    let bloom_ms = if timed_scroll {
        bloom::scroll_ms(lines, time)
    } else {
        0
    };
    let (target, motion) = stage.policy.decide(lines, time, lead);
    let sweeps: Vec<Option<animator::Sweep>> = if bloom_style {
        layouts
            .iter()
            .zip(lines)
            .map(|(layout, line)| layout::sweep(layout, line))
            .collect()
    } else {
        Vec::new()
    };
    let hide_blur = stage.manager.hide_blur;
    stage.anims = stage.animator.animate(
        lines,
        kind,
        time,
        dt,
        now_s,
        hide_blur,
        target,
        &|i| sweeps.get(i).cloned().flatten(),
        reduced_motion,
    );
    let anims = &stage.anims;

    // Where each line is, interludes opening and background vocals folding.
    let n = layouts.len();
    let mut offsets = vec![0.0_f32; n];
    let mut settled = vec![0.0_f32; n];
    let mut accumulated = 0.0;
    let mut settled_y = 0.0;
    let scroll_time = time + lead;
    let interlude_room = |l: &LineLayout| {
        if bloom_style {
            l.height + gap
        } else {
            row_height
        }
    };
    let interlude_centre = |l: &LineLayout| {
        if bloom_style {
            l.height / 2.0
        } else {
            (row_height - gap) / 2.0
        }
    };
    let slot = |i: usize| {
        let l = &layouts[i];
        let next = layouts.get(i + 1);
        let next_y = next.map_or(l.y + l.height + gap, |n| n.y);
        let above = if i > 0 {
            layouts[i - 1].y + layouts[i - 1].height
        } else {
            l.y
        };
        let g = if next.is_some_and(|n| n.background) {
            0.0
        } else {
            gap
        };
        (next_y - above - g).max(0.0)
    };
    for i in 0..n {
        let l = &layouts[i];
        let anim = anims.get(i);
        if l.interlude {
            let line = &lines[i];
            let open = scroll_time >= line.start
                && scroll_time <= line.end - timeline::PRE_HIDDEN_DOT_LINE_MS;
            let open = if open { 1.0 } else { 0.0 };
            settled[i] = l.y + settled_y + interlude_centre(l) * open;
            settled_y += interlude_room(l) * open;
            let scale = anim.map_or(0.0, |a| a.scale.clamp(0.0, 1.0));
            offsets[i] = l.y + accumulated + interlude_centre(l) * scale;
            accumulated += interlude_room(l) * scale;
        } else {
            settled[i] = l.y + settled_y;
            if l.background {
                settled_y -= slot(i) * (1.0 - anim.map_or(1.0, |a| a.presence_goal));
            }
            let fold = if l.background {
                1.0 - anim.map_or(1.0, |a| a.presence)
            } else {
                0.0
            };
            offsets[i] = l.y + accumulated - slot(i) * fold * animator::BACKGROUND_FOLDED_SHIFT;
            accumulated -= slot(i) * fold;
        }
    }
    let footer_start = layouts.last().map_or(0.0, |last| {
        if last.background {
            last.y + last.height + accumulated
        } else {
            offsets[n - 1] + last.height
        }
    });

    // The line to hold: a lead with no words of its own lends it to its
    // background vocals.
    let target = target.map(|i| {
        let next = layouts.get(i + 1);
        if lines[i].words.is_empty()
            && next.is_some_and(|n| n.background)
            && lines.get(i + 1).is_some_and(|n| n.group == lines[i].group)
        {
            i + 1
        } else {
            i
        }
    });
    let align_top = matches!(look.anchor, Anchor::Top(_));
    let mut target_y = target.map(|i| {
        let half = if align_top {
            if layouts[i].interlude {
                -interlude_centre(&layouts[i])
            } else {
                0.0
            }
        } else if layouts[i].interlude {
            0.0
        } else {
            (layouts[i].height / 2.0).min((center - gap).max(0.0))
        };
        -(settled[i] + half)
    });
    let content = layouts.last().map_or(0.0, |l| l.y + l.height) + accumulated + stage.footer;
    let last_end = lines.iter().map(|l| l.end).fold(f64::MIN, f64::max);
    if kind != Kind::Static
        && let Some(aimed) = target_y
        && stage.footer > 0.0
        && time >= last_end
    {
        let reachable = height - look.mask.1 - center - content;
        if aimed > reachable {
            target_y = Some(reachable.max(aimed - height * 0.4));
        }
    }
    let mut scroll_index = target;
    let mut visible = target.map_or(f32::INFINITY, |i| {
        let top = center + stage.manager.scroll + offsets[i];
        let bottom = top + layouts[i].height;
        (bottom.min(height) - top.max(0.0)).max(0.0)
    });
    if kind == Kind::Static {
        target_y = None;
        if now.duration_ms > 0 && n > 0 {
            let through = (time / f64::from(now.duration_ms)).clamp(0.0, 1.0) as f32;
            let reached = through * (content - stage.footer);
            target_y = Some(-reached);
            scroll_index = Some(settled.iter().rposition(|y| *y <= reached).unwrap_or(0));
            visible = f32::INFINITY;
        }
    }

    // The hand on the lyrics.
    let response = ui.interact(
        rect,
        ui.id().with(("lyrics-stage", id)),
        Sense::click_and_drag(),
    );
    let now_ms = now_s * 1000.0;
    if response.drag_started() {
        stage.manager.drag_start(now_ms);
    }
    if response.dragged() {
        stage.manager.drag(now_ms, response.drag_delta().y, content);
    }
    if response.drag_stopped() {
        stage.manager.drag_end(now_ms);
    }
    if ui.rect_contains_pointer(rect) {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            stage.manager.wheel(now_ms, wheel, content);
            ui.ctx().input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    let before = stage.manager.scroll;
    stage.manager.update(
        now_ms,
        dt,
        content,
        scroll_index.filter(|_| target_y.is_some()),
        target_y,
        motion == Motion::Snap || reduced_motion,
        visible,
    );
    let rippling = if bloom_style {
        let tops: Vec<f32> = offsets.clone();
        let heights: Vec<f32> = layouts.iter().map(|l| l.height).collect();
        stage.ripple.update(
            before,
            stage.manager.scroll,
            scroll_index,
            &tops,
            &heights,
            center,
            height,
            dt,
            motion == Motion::Snap || reduced_motion,
            stage.manager.hide_blur,
            bloom_ms,
        )
    } else {
        false
    };
    stage.rippling = rippling;
    let scroll = stage.manager.scroll;
    let row_offset = |i: usize| {
        if bloom_style {
            stage.ripple.offset(i)
        } else {
            0.0
        }
    };
    let line_top = |i: usize| rect.top() + center + scroll + offsets[i] + row_offset(i);

    // Tapping a line plays it.
    if response.clicked()
        && kind != Kind::Static
        && let Some(pointer) = response.interact_pointer_pos()
    {
        for i in 0..n {
            let l = &layouts[i];
            let top = line_top(i);
            if pointer.y < top || pointer.y > top + l.height || l.interlude {
                continue;
            }
            if anims.get(i).is_some_and(|a| a.presence < 0.5) {
                continue;
            }
            if let Some(first) = lines[i].words.first() {
                let start = first.start.max(0.0) as u32;
                let at = if seek_early {
                    start.saturating_sub(300)
                } else {
                    start
                };
                app.actions.push(Action::Seek(at));
                stage.manager.return_to_active();
            }
            break;
        }
    }
    if response.hovered() && kind != Kind::Static {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Paint.
    let mut batch = Batch::new(&ctx);
    batch.mask(rect, look.mask.0, look.mask.1);
    for i in 0..n {
        let (Some(anim), l) = (anims.get(i), &layouts[i]) else {
            continue;
        };
        if anim.opacity <= 0.01 {
            continue;
        }
        let top = line_top(i);
        if top < rect.top() - l.height * 3.0 - 200.0 || top > rect.bottom() + l.height * 3.0 + 200.0
        {
            continue;
        }
        let x = rect.left() + l.start_x();
        let line = &lines[i];
        let lyric = l.font / if l.background { 0.75 } else { 1.0 };
        let css = lyric / 56.0;
        if l.interlude {
            paint_dots(
                &mut batch,
                l,
                anim,
                x,
                top,
                config.bloom,
                config.glow,
                metrics.font,
            );
            continue;
        }
        if kind == Kind::Static {
            for piece in &l.words {
                batch.glyphs(
                    &piece.galley,
                    pos2(x, top) + piece.offset,
                    TSTransform::IDENTITY,
                    Fill::Solid(anim.opacity),
                    None,
                );
            }
            paint_notes(
                &mut batch,
                l,
                x,
                top,
                TSTransform::IDENTITY,
                anim.opacity * 0.65,
                0.0,
            );
            continue;
        }
        let line_mode = kind == Kind::Line || line.line_timed;
        let pivot_y = if line_mode {
            top + if config.bloom && !config.minimal {
                l.lyric_height
            } else {
                l.height / 2.0
            }
        } else if l.background && config.background_pop {
            top
        } else if config.bloom && !config.minimal {
            top + l.lyric_height
        } else {
            top + l.height / 2.0
        };
        let pivot = pos2(
            if l.right_aligned {
                x + l.total_width
            } else {
                x
            },
            pivot_y,
        );
        let line_transform = if (anim.scale - 1.0).abs() > 1e-4 {
            scale_about(pivot, anim.scale)
        } else {
            TSTransform::IDENTITY
        };
        let blur = anim.blur * css;
        if line_mode {
            let bright = config.bright() * anim.opacity;
            let dim = config.line_dim() * anim.opacity;
            let active = anim.state == State::Active && !config.bloom;
            let glow_alpha = (anim.line_glow * 0.5).clamp(0.0, 1.0);
            let glow = (config.glow && glow_alpha > 0.02).then_some((
                glow_alpha * anim.opacity,
                (4.0 + 8.0 * anim.line_glow) * css,
            ));
            let full_h = l.lyric_height.max(1.0);
            for piece in &l.words {
                let origin = pos2(x, top) + piece.offset;
                if active {
                    let fill = if l.rtl {
                        wipe(
                            anim.line_gradient,
                            l.max_row_width,
                            piece.offset.x,
                            bright,
                            dim,
                            true,
                            0.0,
                        )
                    } else {
                        let pos = anim.line_gradient;
                        Fill::Wipe {
                            along: piece.offset.y,
                            lo: pos / 100.0 * full_h,
                            hi: (pos + 20.0) / 100.0 * full_h,
                            left: bright,
                            right: dim,
                            vertical: true,
                        }
                    };
                    batch.glyphs(&piece.galley, origin, line_transform, fill, glow);
                } else {
                    let alpha = inactive_alpha(&config, anim, l.background, config.line_dim());
                    let blur = if anim.active { 0.0 } else { blur };
                    let (fill, shadow) = shadow_only(alpha, blur);
                    batch.glyphs(&piece.galley, origin, line_transform, fill, shadow);
                }
            }
            let alpha = if active {
                bright
            } else {
                inactive_alpha(&config, anim, l.background, config.line_dim())
            };
            paint_notes(
                &mut batch,
                l,
                x,
                top,
                line_transform,
                alpha * 0.65,
                if active { 0.0 } else { blur },
            );
            continue;
        }
        let active = anim.state == State::Active;
        let (dim_alpha, lit_alpha) = if l.background {
            (config.background_dim(), config.background_bright())
        } else {
            (config.dim(), config.bright())
        };
        let inactive = inactive_alpha(&config, anim, l.background, config.dim());
        let em = l.font;
        let fade = if config.bloom {
            bloom::FADE_EM * em
        } else {
            0.0
        };
        for piece in &l.words {
            let Some(word) = anim.words.get(piece.word) else {
                continue;
            };
            let glued = |w: usize| l.glued.get(w).copied().unwrap_or(false);
            let a = anchor(
                piece.word > 0 && glued(piece.word),
                glued(piece.word + 1),
                l.rtl,
            );
            let origin = pos2(x, top) + piece.offset;
            let (w, h) = (piece.width(), piece.height());
            if let Some(k) = piece.letter {
                let Some(letter) = word.letters.get(k) else {
                    continue;
                };
                let word_pivot = pos2(
                    origin.x - piece.start_x + piece.full_width * a,
                    origin.y + h / 2.0,
                );
                let letter_pivot = pos2(origin.x + w / 2.0, origin.y + h / 2.0);
                let ly = if config.bloom {
                    boosted_lift(letter.y, boost) * em
                } else {
                    boosted_lift(letter.y, boost) * lyric * 2.0
                };
                let lx = if config.bloom {
                    letter.x * boost * em
                } else {
                    0.0
                };
                let transform = line_transform
                    * scale_about(word_pivot, boosted_scale(word.scale, boost))
                    * shift(0.0, boosted_lift(word.y, boost) * lyric)
                    * scale_about(letter_pivot, boosted_scale(letter.scale, boost))
                    * shift(lx, ly);
                let glow_blur = if config.bloom {
                    letter.glow_radius * em
                } else {
                    (4.0 + 12.0 * letter.glow) * css
                };
                let glow_alpha =
                    (letter.glow * if config.bloom { 1.0 } else { 1.85 }).clamp(0.0, 1.0);
                let glow = (config.glow && glow_alpha > 0.02)
                    .then_some((glow_alpha * anim.opacity, glow_blur));
                let settling =
                    config.bloom && !active && anim.state == State::Sung && letter.gradient < 99.5;
                if !active && !settling {
                    let (fill, shadow) = shadow_only(inactive, blur);
                    if config.bloom
                        && let Some(glow) = glow
                    {
                        batch.glyphs(&piece.galley, origin, transform, Fill::None, Some(glow));
                    }
                    batch.glyphs(&piece.galley, origin, transform, fill, shadow);
                } else {
                    let bright = if settling {
                        dim_alpha + (lit_alpha - dim_alpha) * anim.lit
                    } else {
                        lit_alpha
                    } * anim.opacity;
                    let (full, start) = if config.bloom {
                        (w, 0.0)
                    } else {
                        (piece.full_width, piece.start_x)
                    };
                    let fill = wipe(
                        letter.gradient,
                        full,
                        start,
                        bright,
                        dim_alpha * anim.opacity,
                        l.rtl,
                        fade,
                    );
                    batch.glyphs(&piece.galley, origin, transform, fill, glow);
                }
                continue;
            }
            let pivot = pos2(origin.x + w * a, origin.y + h / 2.0);
            let transform = line_transform
                * scale_about(pivot, boosted_scale(word.scale, boost))
                * shift(0.0, boosted_lift(word.y, boost) * lyric);
            if !active {
                let (fill, shadow) = shadow_only(inactive, blur);
                batch.glyphs(&piece.galley, origin, transform, fill, shadow);
                continue;
            }
            let glow_alpha = (word.glow * 0.35).clamp(0.0, 1.0);
            let glow = (config.glow && glow_alpha > 0.02)
                .then_some((glow_alpha * anim.opacity, (4.0 + 2.0 * word.glow) * css));
            let fill = wipe(
                word.gradient,
                piece.full_width,
                piece.start_x,
                lit_alpha * anim.opacity,
                dim_alpha * anim.opacity,
                l.rtl,
                fade,
            );
            batch.glyphs(&piece.galley, origin, transform, fill, glow);
        }
        let notes_alpha = if active {
            lit_alpha * anim.opacity
        } else {
            inactive
        };
        paint_notes(
            &mut batch,
            l,
            x,
            top,
            line_transform,
            notes_alpha * 0.65,
            if active { 0.0 } else { blur },
        );
    }
    #[cfg(test)]
    {
        let drawn: Vec<(String, Rect)> = (0..n)
            .filter(|&i| !layouts[i].interlude && anims.get(i).is_some_and(|a| a.opacity > 0.01))
            .filter_map(|i| {
                let top = line_top(i);
                let x = rect.left() + layouts[i].start_x();
                let line = Rect::from_min_size(
                    pos2(x, top),
                    vec2(layouts[i].total_width, layouts[i].height),
                );
                let words: Vec<&str> = lines[i].words.iter().map(|w| w.text.as_str()).collect();
                let mut text = String::new();
                for (w, word) in words.iter().enumerate() {
                    if w > 0 && !lines[i].words[w].glued {
                        text.push(' ');
                    }
                    text.push_str(word);
                }
                (line.intersects(rect)).then_some((text, line.intersect(rect)))
            })
            .collect();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("lyrics-drawn"), drawn));
    }
    batch.paint(ui.painter(), rect);

    // Back to the song, when the hand has left it out of sight.
    if stage.manager.hide_blur
        && kind != Kind::Static
        && let Some(i) = target
    {
        let top = line_top(i);
        let above = top + layouts[i].height <= rect.top();
        let below = top >= rect.bottom();
        if above || below {
            let size = 38.0;
            let at = Rect::from_center_size(
                pos2(rect.center().x, rect.bottom() - size / 2.0 - 16.0),
                vec2(size, size),
            );
            let button = ui.interact(at, ui.id().with(("lyrics-return", id)), Sense::click());
            let painter = ui.painter();
            painter.circle_filled(
                at.center(),
                size / 2.0,
                egui::Color32::from_white_alpha(if button.hovered() { 46 } else { 30 }),
            );
            painter.circle_stroke(
                at.center(),
                size / 2.0 - 0.5,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)),
            );
            theme::paint_icon(
                ui,
                if above {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                },
                Rect::from_center_size(at.center(), vec2(18.0, 18.0)),
                18.0,
                egui::Color32::WHITE,
            );
            if button
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                stage.manager.return_to_active();
            }
        }
    }

    // Keep the frames coming while anything moves.
    let moving = stage.animator.tweening(now_s) || stage.manager.moving() || stage.rippling;
    if (time - stage.last_time).abs() > 0.5 {
        stage.still_since = now_s;
    }
    stage.last_time = time;
    if now.playing {
        if low {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            ctx.request_repaint();
        }
    } else if moving || now_s - stage.still_since < 2.5 {
        ctx.request_repaint();
    }
    stage.offsets = offsets;

    let footer = (n > 0).then(|| {
        let top = rect.top() + center + scroll + footer_start + row_offset(n);
        Rect::from_min_size(
            pos2(rect.left() + look.pad_left, top),
            vec2(
                (width - look.pad_left - look.pad_right).max(1.0),
                stage.footer.max(1.0),
            ),
        )
    });
    Shown { footer }
}

/// The lines the last frame drew, their text and where, for tests.
#[cfg(test)]
pub fn drawn(ctx: &egui::Context) -> Vec<(String, Rect)> {
    ctx.data(|d| d.get_temp(egui::Id::new("lyrics-drawn")))
        .unwrap_or_default()
}

/// Records how tall the credits under the lyrics are, for the scroll.
pub fn set_footer_height(ctx: &egui::Context, id: &str, height: f32) {
    let shared: Option<Arc<Mutex<Option<Stage>>>> =
        ctx.data(|d| d.get_temp(egui::Id::new(("lyrics-stage", id))));
    if let Some(shared) = shared
        && let Ok(mut guard) = shared.lock()
        && let Some(stage) = guard.as_mut()
    {
        stage.footer = height;
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_dots(
    batch: &mut Batch,
    l: &LineLayout,
    anim: &LineAnim,
    x: f32,
    centre_y: f32,
    bloom: bool,
    glow: bool,
    font: f32,
) {
    let group_scale = (if bloom { 1.0 } else { anim.scale } * anim.group_scale).clamp(0.0, 1.3);
    if group_scale < 0.01 {
        return;
    }
    let (Some(first), Some(last)) = (l.words.first(), l.words.last()) else {
        return;
    };
    let centre_x =
        x + (first.offset.x + first.width() / 2.0 + last.offset.x + last.width() / 2.0) / 2.0;
    let group = scale_about(pos2(centre_x, centre_y), group_scale);
    for (d, piece) in l.words.iter().enumerate() {
        let Some(dot) = anim.words.get(d) else {
            continue;
        };
        let opacity = dot.glow.clamp(0.0, 1.0) * anim.opacity;
        let (w, h) = (piece.width(), piece.height());
        let origin = pos2(x + piece.offset.x, centre_y - h / 2.0);
        let pivot = pos2(origin.x + w / 2.0, centre_y);
        let scale = dot.scale.clamp(0.0, 1.5);
        let transform = group * scale_about(pivot, scale) * shift(0.0, dot.y * font);
        let halo = dot.dot_glow.clamp(0.0, 1.0);
        let shadow = (glow && !bloom && halo * 0.9 > 0.02)
            .then_some((halo * 0.9 * anim.opacity, (4.0 + 6.0 * halo) * font / 56.0));
        batch.glyphs(
            &piece.galley,
            origin,
            transform,
            Fill::Solid(opacity),
            shadow,
        );
    }
}

fn paint_notes(
    batch: &mut Batch,
    l: &LineLayout,
    x: f32,
    top: f32,
    transform: TSTransform,
    alpha: f32,
    blur: f32,
) {
    for note in &l.notes {
        let origin = pos2(x, top) + note.offset;
        let (fill, shadow) = shadow_only(alpha.clamp(0.0, 1.0), blur);
        batch.glyphs(&note.galley, origin, transform, fill, shadow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_words_grow_from_their_seams() {
        assert_eq!(anchor(false, true, false), 1.0);
        assert_eq!(anchor(true, false, false), 0.0);
        assert_eq!(anchor(true, true, false), 0.5);
        assert_eq!(anchor(false, false, false), 0.5);
        assert_eq!(anchor(false, true, true), 0.0);
    }

    #[test]
    fn the_motion_boost_only_grows_and_lifts() {
        assert!((boosted_scale(1.04, 1.25) - 1.05).abs() < 1e-6);
        assert_eq!(boosted_scale(0.95, 1.25), 0.95);
        assert!((boosted_lift(-0.02, 1.25) + 0.025).abs() < 1e-6);
        assert_eq!(boosted_lift(0.01, 1.25), 0.01);
    }
}
