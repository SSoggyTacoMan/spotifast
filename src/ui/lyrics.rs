//! The words of the playing track, in a side panel that follows the song.

use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Galley, Layout, Margin, Rect, Sense, UiBuilder,
    pos2, vec2,
};

use crate::app::App;
use crate::i18n::{gettext, pgettext};
use crate::lyrics::{Line, Lyrics};
use crate::model::{Action, Loadable, Page};
use crate::settings::{LyricsAlign, LyricsStyle};
use crate::theme::{self, Icon};

use super::widgets;

const LINE_SIZE: f32 = 23.0;
const LINE_GAP: f32 = 15.0;
/// The gap between two rows of a word-synced line that wrapped.
const WORD_ROW_GAP: f32 = 5.0;
/// The gap above a line's translation or backing vocals.
const EXTRA_GAP: f32 = 5.0;
/// Where the line being sung sits in the side panel, as a fraction of the
/// visible lyrics from the top: high up, so the lines to come fill most of
/// the view. Full screen uses the user's own position instead.
const SUNG_LINE_AT: f32 = 0.2;

/// Scrolls so the middle of `line` sits `at` of the way down the visible
/// lyrics.
fn show_sung_line(
    ui: &egui::Ui,
    line: Rect,
    at: f32,
    animation: Option<egui::style::ScrollAnimation>,
) {
    let above = (ui.clip_rect().height() * at - line.height() / 2.0).max(0.0);
    let target = Rect::from_min_max(pos2(line.left(), line.top() - above), line.max);
    match animation {
        Some(animation) => ui.scroll_to_rect_animation(target, Some(Align::Min), animation),
        None => ui.scroll_to_rect(target, Some(Align::Min)),
    }
}
/// The soft light around sung words. Egui has no text shadow, so the same
/// glyphs are stamped at a few sub-pixel offsets in a translucent colour.
/// Eight stamps read as a glow; four keep a slow machine lighter, and only
/// the live line pays for them.
fn paint_glow(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: &std::sync::Arc<Galley>,
    color: Color32,
    radius: f32,
    dense: bool,
) {
    let stamps: &[(f32, f32)] = if dense {
        &[
            (-1.0, 0.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-0.7, -0.7),
            (0.7, -0.7),
            (-0.7, 0.7),
            (0.7, 0.7),
        ]
    } else {
        &[(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
    };
    let stamp_color = color.gamma_multiply(if dense { 0.25 } else { 0.45 });
    for &(x, y) in stamps {
        painter.galley_with_override_text_color(
            pos + vec2(x * radius, y * radius),
            galley.clone(),
            stamp_color,
        );
    }
}

/// A word swells and lifts smoothly as it starts sounding, then settles
/// onto the baseline, the way Spicy Lyrics' word spring moves. The timing is
/// in the lyric clock, so seeking backwards replays the bounce and pausing
/// freezes it where it is. `strength` scales the whole gesture.
fn word_bounce(position_ms: u32, at_ms: u32, size: f32, strength: f32) -> (f32, f32) {
    const SECONDS: f32 = 0.48;
    if strength <= 0.0 {
        return (1.0, 0.0);
    }
    let elapsed = position_ms.saturating_sub(at_ms) as f32 / 1000.0;
    if elapsed >= SECONDS {
        return (1.0, 0.0);
    }
    let t = (elapsed / SECONDS).clamp(0.0, 1.0);
    let pulse = (t * std::f32::consts::PI).sin();
    let ease = pulse * pulse * (1.0 - 0.25 * t) * strength;
    (1.0 + 0.06 * ease, -size * 0.05 * ease)
}

fn word_bounce_varied(
    position_ms: u32,
    at_ms: u32,
    size: f32,
    strength: f32,
    seed: usize,
) -> (f32, f32) {
    let (scale, lift) = word_bounce(position_ms, at_ms, size, strength);
    if (scale - 1.0).abs() <= 0.0001 && lift.abs() <= 0.0001 {
        return (scale, lift);
    }
    let factor = 1.0 + 0.12 * ((seed as f32 * 1.618_034).sin());
    (1.0 + (scale - 1.0) * factor, lift * factor)
}

/// Paints one galley with a scale-and-lift around `pivot`.
fn paint_lifted(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: std::sync::Arc<Galley>,
    color: Color32,
    pivot: egui::Pos2,
    scale: f32,
    lift: f32,
) {
    let transform =
        egui::emath::TSTransform::new(pivot.to_vec2() * (1.0 - scale) + vec2(0.0, lift), scale);
    let mut shape = egui::epaint::TextShape::new(pos, galley, color);
    shape.transform(transform);
    painter.add(shape);
}

/// The same, for the soft stamps around a lifted word.
#[allow(clippy::too_many_arguments)]
fn paint_lifted_glow(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: &std::sync::Arc<Galley>,
    color: Color32,
    pivot: egui::Pos2,
    scale: f32,
    lift: f32,
    radius: f32,
    dense: bool,
) {
    let transform =
        egui::emath::TSTransform::new(pivot.to_vec2() * (1.0 - scale) + vec2(0.0, lift), scale);
    let stamp_color = color.gamma_multiply(if dense { 0.25 } else { 0.45 });
    let mut stamp = egui::epaint::TextShape::new(pos, galley.clone(), stamp_color)
        .with_override_text_color(stamp_color);
    stamp.transform(transform);
    let stamps: &[(f32, f32)] = if dense {
        &[
            (-1.0, 0.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-0.7, -0.7),
            (0.7, -0.7),
            (-0.7, 0.7),
            (0.7, 0.7),
        ]
    } else {
        &[(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
    };
    for &(x, y) in stamps {
        let mut copy = stamp.clone();
        copy.pos += vec2(x * radius, y * radius);
        painter.add(copy);
    }
}

fn blend(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    egui::Color32::from(egui::Rgba::from(from) * (1.0 - t) + egui::Rgba::from(to) * t)
}

/// How far a row shifts to sit left, centred or right inside its column.
fn align_offset(align: LyricsAlign, room: f32, width: f32) -> f32 {
    let spare = (room - width).max(0.0);
    match align {
        LyricsAlign::Left => 0.0,
        LyricsAlign::Center => spare / 2.0,
        LyricsAlign::Right => spare,
    }
}

/// The scene-wide choices shared by every line: how strongly words move and
/// glow, how far away lines fade, and how the view follows the song.
struct MotionLook {
    bounce: f32,
    glow: Color32,
    glow_strength: f32,
    fade_width: f32,
    line_fade: f32,
    focus: f32,
    dim: f32,
    rise: bool,
    active_growth: f32,
    join_words: bool,
    low_performance: bool,
    align: LyricsAlign,
    duet_padding: bool,
    show_background: bool,
    show_translation: bool,
    background_pop: bool,
}

impl MotionLook {
    fn from_settings(settings: &crate::settings::Settings, glow_alpha: u8) -> Self {
        let simple = settings.lyrics_simple_mode;
        let glow_on = settings.lyrics_glow && !simple;
        let glow_strength = if glow_on {
            f32::from(settings.lyrics_glow_strength) / 100.0
        } else {
            0.0
        };
        let alpha = (f32::from(glow_alpha) * glow_strength)
            .round()
            .clamp(0.0, 255.0) as u8;
        Self {
            bounce: if settings.lyrics_bounce && !simple {
                f32::from(settings.lyrics_bounce_strength) / 100.0
            } else {
                0.0
            },
            glow: Color32::from_white_alpha(alpha),
            glow_strength,
            fade_width: f32::from(settings.lyrics_fade_width) / 100.0,
            line_fade: settings.lyrics_line_fade_ms as f32 / 1000.0,
            focus: f32::from(settings.lyrics_focus_strength) / 100.0,
            dim: 0.15 + 0.85 * f32::from(settings.lyrics_dim) / 100.0,
            rise: settings.lyrics_rise && !simple,
            active_growth: if settings.lyrics_scale && !simple {
                settings.lyrics_style.active_growth()
            } else {
                1.0
            },
            join_words: settings.lyrics_join_words,
            low_performance: settings.lyrics_low_performance,
            align: settings.lyrics_align,
            duet_padding: settings.lyrics_duet_padding,
            show_background: !settings.lyrics_minimal_mode,
            show_translation: settings.lyrics_show_translations && !settings.lyrics_minimal_mode,
            background_pop: settings.lyrics_background_pop,
        }
    }

    /// How much a line `distance` lines away from the sung one keeps.
    fn focus_factor(&self, distance: usize) -> f32 {
        if distance == 0 || self.focus <= 0.0 {
            1.0
        } else {
            0.62_f32.powf(distance as f32 * self.focus)
        }
    }

    fn line(&self, font: FontId, base: Color32, fill: Color32, wrap: f32, rise: f32) -> LineLook {
        LineLook {
            font,
            base,
            fill,
            glow: self.glow,
            glow_strength: self.glow_strength,
            wrap,
            fade_width: self.fade_width,
            bounce: self.bounce,
            join_words: self.join_words,
            rise,
            align: self.align,
            duet_padding: self.duet_padding,
            show_background: self.show_background,
            show_translation: self.show_translation,
            glow_dense: !self.low_performance,
            background_pop: self.background_pop,
        }
    }
}

/// Everything about how one line is painted that does not change from line
/// to line.
struct LineLook {
    font: FontId,
    base: Color32,
    fill: Color32,
    glow: Color32,
    glow_strength: f32,
    wrap: f32,
    fade_width: f32,
    bounce: f32,
    join_words: bool,
    rise: f32,
    align: LyricsAlign,
    duet_padding: bool,
    show_background: bool,
    show_translation: bool,
    glow_dense: bool,
    background_pop: bool,
}

/// One syllable placed inside a word.
struct WordPiece {
    pos: egui::Pos2,
    /// The glyphs in the line's quiet colour.
    galley: std::sync::Arc<Galley>,
    bright: bool,
    glow: bool,
    at_ms: u32,
    end_ms: u32,
}

/// A word's syllables, which wrap as one piece.
struct WordCluster {
    pieces: Vec<WordPiece>,
    /// The row of the wrapped line this word landed on.
    row: usize,
}

/// A word-synced line laid out for this frame.
struct WordLayout {
    clusters: Vec<WordCluster>,
    background_clusters: Vec<WordCluster>,
    /// Translations, already coloured.
    extras: Vec<(egui::Pos2, std::sync::Arc<Galley>)>,
    height: f32,
}

#[allow(clippy::too_many_arguments)]
fn layout_syllable_clusters(
    painter: &egui::Painter,
    syllables: &[crate::lyrics::Syllable],
    font: &FontId,
    color: Color32,
    wrap: f32,
    indent: f32,
    align: LyricsAlign,
    bright: bool,
    glow: bool,
    y_offset: f32,
) -> (Vec<WordCluster>, f32) {
    let sample = painter.layout_no_wrap("Ag".to_owned(), font.clone(), color);
    let line_height = sample.size().y.max(font.size * 1.25);
    let space = painter
        .layout_no_wrap(" ".to_owned(), font.clone(), color)
        .size()
        .x
        .max(font.size * 0.18);

    let mut clusters: Vec<WordCluster> = Vec::new();
    for (index, syllable) in syllables.iter().enumerate() {
        // A syllable starts a new word unless the one before it continues
        // into it.
        if index == 0 || !syllables[index - 1].part_of_word {
            clusters.push(WordCluster {
                pieces: Vec::new(),
                row: 0,
            });
        }
        let galley = painter.layout_no_wrap(syllable.text.clone(), font.clone(), color);
        clusters
            .last_mut()
            .expect("a cluster was just opened")
            .pieces
            .push(WordPiece {
                pos: egui::Pos2::ZERO,
                galley,
                bright,
                glow,
                at_ms: syllable.at_ms,
                end_ms: syllable.end_ms.max(syllable.at_ms),
            });
    }

    let room = (wrap - indent).max(1.0);
    let mut x = 0.0_f32;
    let mut y = y_offset;
    let mut row = 0_usize;
    let mut row_widths: Vec<f32> = vec![0.0];
    for cluster in &mut clusters {
        let width: f32 = cluster
            .pieces
            .iter()
            .map(|piece| piece.galley.size().x)
            .sum();
        let mut left = x;
        if left > 0.0 {
            left += space;
        }
        // A word too wide for the column still starts on its own row.
        if left > 0.0 && left + width > room + 0.5 {
            left = 0.0;
            row += 1;
            row_widths.push(0.0);
            y += line_height + WORD_ROW_GAP;
        }
        cluster.row = row;
        let mut pen = left;
        for piece in &mut cluster.pieces {
            piece.pos = pos2(pen, y);
            pen += piece.galley.size().x;
        }
        x = pen;
        row_widths[row] = row_widths[row].max(pen);
    }
    let height = if clusters.is_empty() {
        0.0
    } else {
        (y - y_offset) + line_height
    };

    for cluster in &mut clusters {
        let offset = indent + align_offset(align, room, row_widths[cluster.row]);
        for piece in &mut cluster.pieces {
            piece.pos.x += offset;
        }
    }

    (clusters, height)
}

/// Lays a word-synced line out in words, wrapping between them. Every syllable
/// inside a word stays together, so a word is never split across two rows.
fn layout_word_line(
    ui: &egui::Ui,
    line: &Line,
    look: &LineLook,
    bright: bool,
    glow: bool,
    bg_pop: f32,
) -> WordLayout {
    let painter = ui.painter();
    let font = &look.font;
    let base = look.base;
    let wrap = look.wrap;
    let sample = painter.layout_no_wrap("Ag".to_owned(), font.clone(), base);
    let line_height = sample.size().y.max(font.size * 1.25);

    // Duet lines step in from the singer's side, and alignment happens
    // inside what remains of the column.
    let indent = if look.duet_padding && line.opposite_aligned {
        (wrap * 0.07).min(56.0)
    } else {
        0.0
    };
    let room = (wrap - indent).max(1.0);

    let (clusters, lead_height) = layout_syllable_clusters(
        painter,
        &line.syllables,
        font,
        base,
        wrap,
        indent,
        look.align,
        bright,
        glow,
        0.0,
    );
    let main_height = if clusters.is_empty() {
        if line.background.is_empty() {
            line_height
        } else {
            0.0
        }
    } else {
        lead_height
    };

    let bg_y_offset = if main_height > 0.0 {
        main_height + EXTRA_GAP
    } else {
        0.0
    };
    let (background_clusters, background_height) =
        if look.show_background && !line.background.is_empty() && bg_pop > 0.01 {
            let bg_font = theme::regular((font.size * 0.72).max(11.0));
            let bg_base = base.gamma_multiply(0.72);
            layout_syllable_clusters(
                painter,
                &line.background,
                &bg_font,
                bg_base,
                wrap,
                indent,
                look.align,
                true,
                glow,
                bg_y_offset,
            )
        } else {
            (Vec::new(), 0.0)
        };

    let mut extras = Vec::new();
    let mut extra_y = main_height;
    if background_height > 0.0 {
        let gap = if main_height > 0.0 { EXTRA_GAP } else { 0.0 };
        extra_y += (background_height + gap) * bg_pop;
    }
    let mut add_extra = |text: &str, size: f32, color: Color32| {
        if text.trim().is_empty() {
            return;
        }
        let galley = painter.layout(text.to_string(), theme::regular(size), color, wrap.max(1.0));
        let x = indent + align_offset(look.align, room, galley.size().x);
        extras.push((pos2(x, extra_y + EXTRA_GAP), galley.clone()));
        extra_y += galley.size().y + EXTRA_GAP;
    };
    if look.show_translation {
        let transliterated = line
            .transliterated
            .clone()
            .or_else(|| joined_transliteration(&line.syllables));
        if let Some(text) = transliterated {
            add_extra(
                &text,
                (font.size * 0.68).max(11.0),
                base.gamma_multiply(0.62),
            );
        }
        if let Some(text) = &line.translated {
            add_extra(
                text,
                (font.size * 0.68).max(11.0),
                base.gamma_multiply(0.55),
            );
        }
    }

    WordLayout {
        clusters,
        background_clusters,
        extras,
        height: extra_y.max(main_height),
    }
}

/// Syllables as a line of words, with spaces only between words.
#[cfg(test)]
fn joined_text(syllables: &[crate::lyrics::Syllable]) -> String {
    joined_field_owned(
        syllables,
        |syllable| Some(syllable.text.as_str()),
        |syllable| syllable.part_of_word,
    )
    .unwrap_or_default()
}

/// The romanisation, joined by its own word markers when the source gave
/// them, since it can split words where the original text does not.
fn joined_transliteration(syllables: &[crate::lyrics::Syllable]) -> Option<String> {
    joined_field_owned(
        syllables,
        |syllable| syllable.transliterated.as_deref(),
        |syllable| {
            syllable
                .transliterated_part_of_word
                .unwrap_or(syllable.part_of_word)
        },
    )
}

fn joined_field_owned(
    syllables: &[crate::lyrics::Syllable],
    field: impl Fn(&crate::lyrics::Syllable) -> Option<&str>,
    joins_next: impl Fn(&crate::lyrics::Syllable) -> bool,
) -> Option<String> {
    let mut text = String::new();
    let mut previous_joined = None;
    for syllable in syllables {
        let Some(part) = field(syllable).filter(|part| !part.trim().is_empty()) else {
            continue;
        };
        if previous_joined == Some(false) {
            text.push(' ');
        }
        text.push_str(part.trim());
        previous_joined = Some(joins_next(syllable));
    }
    (!text.is_empty()).then_some(text)
}

/// Paints the karaoke fill up to `progress` of a syllable, with a soft
/// leading edge rather than a hard clip. The edge is a handful of translucent
/// slices, which reads as Apple Music's word mask without a shader.
#[allow(clippy::too_many_arguments)]
fn paint_word_fill(
    painter: &egui::Painter,
    pos: egui::Pos2,
    galley: &std::sync::Arc<Galley>,
    color: Color32,
    progress: f32,
    fade_width: f32,
    bounds: Rect,
    transform: Option<egui::emath::TSTransform>,
) {
    fn slice(
        painter: &egui::Painter,
        pos: egui::Pos2,
        galley: &std::sync::Arc<Galley>,
        color: Color32,
        bounds: Rect,
        transform: Option<egui::emath::TSTransform>,
        from: f32,
        to: f32,
        opacity: f32,
    ) {
        if to <= from {
            return;
        }
        let mut clip = Rect::from_min_max(
            egui::pos2(pos.x + from, pos.y),
            egui::pos2(pos.x + to, pos.y + galley.size().y),
        );
        if let Some(transform) = transform {
            clip = Rect::from_min_max(transform.mul_pos(clip.min), transform.mul_pos(clip.max));
        }
        let mut shape = egui::epaint::TextShape::new(pos, galley.clone(), color)
            .with_override_text_color(color);
        if let Some(transform) = transform {
            shape.transform(transform);
        }
        shape.opacity_factor = opacity.clamp(0.0, 1.0);
        painter.with_clip_rect(clip.intersect(bounds)).add(shape);
    }

    let width = galley.size().x;
    if width <= 0.0 || progress <= 0.0 {
        return;
    }
    let filled = width * progress.clamp(0.0, 1.0);
    if progress >= 1.0 || fade_width <= 0.0 {
        slice(
            painter, pos, galley, color, bounds, transform, 0.0, filled, 1.0,
        );
        return;
    }
    let edge = (width * fade_width.min(1.0)).min(filled);
    let solid = filled - edge;
    if solid > 0.0 {
        slice(
            painter, pos, galley, color, bounds, transform, 0.0, solid, 1.0,
        );
    }
    const STEPS: usize = 10;
    for step in 1..=STEPS {
        let from = solid + edge * (step as f32 - 1.0) / STEPS as f32;
        let to = solid + edge * step as f32 / STEPS as f32;
        let t = step as f32 / STEPS as f32;
        let opacity = t * t * (3.0 - 2.0 * t);
        slice(
            painter, pos, galley, color, bounds, transform, from, to, opacity,
        );
    }
}

fn background_pop(
    ui: &egui::Ui,
    line: &Line,
    position_ms: u32,
    active: bool,
    look: &LineLook,
    line_index: usize,
) -> f32 {
    if !look.show_background || line.background.is_empty() {
        0.0
    } else if look.background_pop {
        let bg_active = active
            || line
                .background
                .iter()
                .any(|s| position_ms >= s.at_ms && position_ms <= s.end_ms);
        let pop_id =
            egui::Id::new("bg-pop").with((line_index, line.at_ms, look.font.size.to_bits()));
        ui.ctx()
            .animate_bool_with_time(pop_id, bg_active, crate::motion::seconds(ui.ctx(), 0.3))
    } else {
        1.0
    }
}

/// Draws a word-synced line, filling each syllable from left to right while it
/// sounds. Returns the clickable row and its height.
fn paint_word_line(
    ui: &mut egui::Ui,
    line: &Line,
    position_ms: u32,
    active: bool,
    look: &LineLook,
    line_index: usize,
) -> (egui::Response, f32) {
    let top = ui.cursor().top();
    let clip = ui.clip_rect();
    let bg_pop = background_pop(ui, line, position_ms, active, look, line_index);
    let cache_id = egui::Id::new("word-line-height").with((
        &line.text,
        look.font.size.to_bits(),
        look.wrap.to_bits(),
        look.show_background && !line.background.is_empty(),
        look.background_pop,
        look.show_translation && (line.translated.is_some() || line.transliterated.is_some()),
    ));
    let cached_height: Option<f32> = ui.data(|d| d.get_temp(cache_id));
    if !active
        && bg_pop <= 0.01
        && let Some(height) = cached_height
        && (top > clip.bottom() + 100.0 || top + height < clip.top() - 100.0)
    {
        let (_rect, response) =
            ui.allocate_exact_size(vec2(look.wrap.max(1.0), height), Sense::click());
        return (response, height);
    }
    let layout = layout_word_line(ui, line, look, active, active && look.glow.a() > 0, bg_pop);
    if bg_pop <= 0.01 && cached_height != Some(layout.height) {
        ui.data_mut(|d| d.insert_temp(cache_id, layout.height));
    }
    let (rect, response) =
        ui.allocate_exact_size(vec2(look.wrap.max(1.0), layout.height), Sense::click());
    if !ui.is_rect_visible(rect) || layout.height < 0.5 {
        return (response, layout.height);
    }
    let painter = ui.painter();
    let bounds = Rect::from_min_max(
        rect.min - vec2(8.0, 8.0),
        rect.max + vec2(8.0, look.rise.max(0.0) + 8.0),
    );
    for (cluster_idx, cluster) in layout.clusters.iter().enumerate() {
        let start = cluster.pieces.first().map_or(0, |piece| piece.at_ms);
        let (scale, lift) = if active {
            word_bounce_varied(
                position_ms,
                start,
                look.font.size,
                look.bounce,
                line_index * 31 + cluster_idx,
            )
        } else {
            (1.0, 0.0)
        };
        let lifted = (scale - 1.0).abs() > 0.001 || lift.abs() > 0.01;
        let pivot = cluster.pieces.first().map_or(rect.center(), |first| {
            let right = cluster
                .pieces
                .last()
                .map_or(first.pos.x + first.galley.size().x, |last| {
                    last.pos.x + last.galley.size().x
                });
            rect.min
                + vec2(
                    (first.pos.x + right) / 2.0,
                    look.rise + first.pos.y + first.galley.size().y * 0.78,
                )
        });
        let transform = lifted.then(|| {
            egui::emath::TSTransform::new(pivot.to_vec2() * (1.0 - scale) + vec2(0.0, lift), scale)
        });
        let cluster_end = cluster
            .pieces
            .iter()
            .map(|piece| piece.end_ms)
            .max()
            .unwrap_or(start)
            .max(start);
        for piece in &cluster.pieces {
            let pos = rect.min + piece.pos.to_vec2() + vec2(0.0, look.rise);
            if lifted {
                paint_lifted(
                    painter,
                    pos,
                    piece.galley.clone(),
                    look.base,
                    pivot,
                    scale,
                    lift,
                );
            } else {
                painter.galley(pos, piece.galley.clone(), look.base);
            }
            if !piece.bright {
                continue;
            }
            let progress = if look.join_words {
                crate::lyrics::span_progress(start, cluster_end, position_ms)
            } else {
                crate::lyrics::span_progress(piece.at_ms, piece.end_ms, position_ms)
            };
            if progress <= 0.0 {
                continue;
            }
            if piece.glow && look.glow.a() > 0 {
                let radius =
                    (look.font.size * 0.09).clamp(1.5, 3.2) * (0.6 + 0.4 * look.glow_strength);
                let mut glow_clip = Rect::from_min_max(
                    pos,
                    pos2(
                        pos.x + piece.galley.size().x * progress,
                        pos.y + piece.galley.size().y,
                    ),
                );
                if let Some(transform) = transform {
                    glow_clip = Rect::from_min_max(
                        transform.mul_pos(glow_clip.min),
                        transform.mul_pos(glow_clip.max),
                    );
                }
                let glow_clip = glow_clip
                    .expand2(vec2(radius + 1.0, radius + 1.0))
                    .intersect(bounds);
                let clipped_glow = painter.with_clip_rect(glow_clip);
                if lifted {
                    paint_lifted_glow(
                        &clipped_glow,
                        pos,
                        &piece.galley,
                        look.glow,
                        pivot,
                        scale,
                        lift,
                        radius,
                        look.glow_dense,
                    );
                } else {
                    paint_glow(
                        &clipped_glow,
                        pos,
                        &piece.galley,
                        look.glow,
                        radius,
                        look.glow_dense,
                    );
                }
            }
            paint_word_fill(
                painter,
                pos,
                &piece.galley,
                look.fill,
                progress,
                look.fade_width,
                bounds,
                transform,
            );
        }
    }
    if bg_pop > 0.01 && !layout.background_clusters.is_empty() {
        let bg_size = (look.font.size * 0.72).max(11.0);
        let bg_base = look.base.gamma_multiply(0.72 * bg_pop);
        let bg_fill = look.fill.gamma_multiply(0.88 * bg_pop);
        let bg_glow = look.glow.gamma_multiply(0.85 * bg_pop);
        for (cluster_idx, cluster) in layout.background_clusters.iter().enumerate() {
            let start = cluster.pieces.first().map_or(0, |piece| piece.at_ms);
            let cluster_end = cluster
                .pieces
                .iter()
                .map(|piece| piece.end_ms)
                .max()
                .unwrap_or(start)
                .max(start);
            let (scale, lift) = if active {
                word_bounce_varied(
                    position_ms,
                    start,
                    bg_size,
                    look.bounce * 0.65,
                    line_index * 53 + cluster_idx + 7,
                )
            } else {
                (1.0, 0.0)
            };
            let lifted = (scale - 1.0).abs() > 0.001 || lift.abs() > 0.01;
            let pivot = cluster.pieces.first().map_or(rect.center(), |first| {
                let right = cluster
                    .pieces
                    .last()
                    .map_or(first.pos.x + first.galley.size().x, |last| {
                        last.pos.x + last.galley.size().x
                    });
                rect.min
                    + vec2(
                        (first.pos.x + right) / 2.0,
                        look.rise + first.pos.y + first.galley.size().y * 0.78,
                    )
            });
            let transform = lifted.then(|| {
                egui::emath::TSTransform::new(
                    pivot.to_vec2() * (1.0 - scale) + vec2(0.0, lift),
                    scale,
                )
            });
            for piece in &cluster.pieces {
                let pos = rect.min + piece.pos.to_vec2() + vec2(0.0, look.rise);
                if lifted {
                    paint_lifted(
                        painter,
                        pos,
                        piece.galley.clone(),
                        bg_base,
                        pivot,
                        scale,
                        lift,
                    );
                } else {
                    painter.galley(pos, piece.galley.clone(), bg_base);
                }
                let progress = if look.join_words {
                    crate::lyrics::span_progress(start, cluster_end, position_ms)
                } else {
                    crate::lyrics::span_progress(piece.at_ms, piece.end_ms, position_ms)
                };
                if progress <= 0.0 {
                    continue;
                }
                if piece.glow && bg_glow.a() > 0 {
                    let radius =
                        (bg_size * 0.08).clamp(1.2, 2.6) * (0.6 + 0.4 * look.glow_strength);
                    let mut glow_clip = Rect::from_min_max(
                        pos,
                        pos2(
                            pos.x + piece.galley.size().x * progress,
                            pos.y + piece.galley.size().y,
                        ),
                    );
                    if let Some(transform) = transform {
                        glow_clip = Rect::from_min_max(
                            transform.mul_pos(glow_clip.min),
                            transform.mul_pos(glow_clip.max),
                        );
                    }
                    let glow_clip = glow_clip
                        .expand2(vec2(radius + 1.0, radius + 1.0))
                        .intersect(bounds);
                    let clipped_glow = painter.with_clip_rect(glow_clip);
                    if lifted {
                        paint_lifted_glow(
                            &clipped_glow,
                            pos,
                            &piece.galley,
                            bg_glow,
                            pivot,
                            scale,
                            lift,
                            radius,
                            look.glow_dense,
                        );
                    } else {
                        paint_glow(
                            &clipped_glow,
                            pos,
                            &piece.galley,
                            bg_glow,
                            radius,
                            look.glow_dense,
                        );
                    }
                }
                paint_word_fill(
                    painter,
                    pos,
                    &piece.galley,
                    bg_fill,
                    progress,
                    look.fade_width,
                    bounds,
                    transform,
                );
            }
        }
    }
    for (pos, galley) in &layout.extras {
        painter.galley(
            rect.min + pos.to_vec2() + vec2(0.0, look.rise),
            galley.clone(),
            look.base,
        );
    }
    (response, layout.height)
}

/// Whether a line can use the word renderer: it has syllables and reads left
/// to right, so the left-to-right fill means what it should.
fn word_line(line: &Line) -> bool {
    (line.has_syllables() || !line.background.is_empty()) && !crate::bidi::is_rtl(&line.text)
}

/// A line without syllables, painted by hand so the live one can carry the
/// same glow as a word-synced one.
fn paint_plain_line(
    ui: &mut egui::Ui,
    text: &str,
    look: &LineLook,
    color: Color32,
    active: bool,
    indent: f32,
    sense: Sense,
) -> egui::Response {
    let top = ui.cursor().top();
    let clip = ui.clip_rect();
    let room = (look.wrap - indent).max(1.0);
    let cache_id = egui::Id::new("plain-line-height").with((
        text,
        look.font.size.to_bits(),
        room.to_bits(),
        active,
    ));
    let cached_height: Option<f32> = ui.data(|d| d.get_temp(cache_id));
    if !active
        && let Some(height) = cached_height
        && (top > clip.bottom() + 100.0 || top + height < clip.top() - 100.0)
    {
        let (_rect, response) = ui.allocate_exact_size(vec2(look.wrap.max(1.0), height), sense);
        return response;
    }
    let painter = ui.painter().clone();
    let galley = crate::bidi::layout(
        &painter,
        text,
        look.font.clone(),
        color,
        room,
        usize::MAX,
        None,
    );
    let height = galley.size().y.max(1.0);
    if cached_height != Some(height) {
        ui.data_mut(|d| d.insert_temp(cache_id, height));
    }
    let (rect, response) = ui.allocate_exact_size(vec2(look.wrap.max(1.0), height), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let width = galley.size().x;
    let left = if crate::bidi::is_rtl(text) {
        match look.align {
            LyricsAlign::Left => rect.right() - indent - width,
            LyricsAlign::Center => rect.left() + indent + (room - width).max(0.0) / 2.0,
            LyricsAlign::Right => rect.left() + indent,
        }
    } else {
        rect.left() + indent + align_offset(look.align, room, width)
    };
    let pos = pos2(left, rect.top() + look.rise);
    if active && look.glow.a() > 0 {
        let radius = (look.font.size * 0.07).clamp(1.2, 2.8) * (0.6 + 0.4 * look.glow_strength);
        paint_glow(painter, pos, &galley, look.glow, radius, look.glow_dense);
    }
    painter.galley(pos, galley, color);
    response
}

/// The three dots a musical interlude shows. Each dot swells and lifts in
/// turn while the break progresses, matching Spicy Lyrics.
#[allow(clippy::too_many_arguments)]
fn paint_interlude(
    ui: &mut egui::Ui,
    active: bool,
    color: Color32,
    glow: Color32,
    size: f32,
    wrap: f32,
    playing: bool,
    start_ms: u32,
    end_ms: u32,
    position_ms: u32,
) -> egui::Response {
    let height = size * 1.2;
    let (rect, response) = ui.allocate_exact_size(vec2(wrap.max(1.0), height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let base_radius = (size * 0.075).clamp(2.0, 3.6);
    let gap = base_radius * 4.4;
    let time = ui.input(|input| input.time) as f32;
    let dot_times =
        (end_ms > start_ms).then(|| crate::lyrics::interlude_dot_times(start_ms, end_ms));

    for index in 0..3 {
        let x = rect.center().x + (index as f32 - 1.0) * gap;
        let (dot_scale, dot_lift, dot_alpha, dot_glowing) = if let Some(times) = dot_times {
            let (d_start, d_end) = times[index];
            if !active {
                (1.0, 0.0, 0.35, false)
            } else if position_ms < d_start {
                (0.85, 0.0, 0.35, false)
            } else if position_ms < d_end {
                let p = crate::lyrics::span_progress(d_start, d_end, position_ms);
                let pulse = (p * std::f32::consts::PI).sin();
                (0.85 + 0.35 * pulse, -size * 0.14 * pulse, 1.0, true)
            } else {
                (1.0, 0.0, 1.0, true)
            }
        } else {
            let lift = if active && playing {
                (time * 4.6 + index as f32 * 1.1).sin().max(0.0) * size * 0.09
            } else {
                0.0
            };
            (1.0, -lift, if active { 1.0 } else { 0.35 }, active)
        };

        let radius = base_radius * dot_scale;
        let center = pos2(x, rect.center().y + dot_lift);
        if active && dot_glowing && glow.a() > 0 {
            for ring in 1..=4 {
                let spread = radius * (1.0 + ring as f32 * 0.7);
                painter.circle_filled(center, spread, glow.gamma_multiply(0.20 / ring as f32));
            }
        }
        painter.circle_filled(center, radius, color.gamma_multiply(dot_alpha));
    }
    response
}

/// The song's own colours, or the interface accent when it has none.
fn playing_colors(app: &App) -> Vec<Color32> {
    let mut colors: Vec<Color32> = app
        .now_playing_palette()
        .unwrap_or_default()
        .iter()
        .take(3)
        .map(|[r, g, b]| Color32::from_rgb(*r, *g, *b))
        .collect();
    if colors.is_empty() {
        colors.push(app.palette.accent);
    }
    colors
}

/// Large soft colour fields, drifting while the song plays. They borrow the
/// cover's palette where it has one, and keep the words readable everywhere
/// by staying very transparent.
fn paint_ambient(ui: &egui::Ui, rect: Rect, colors: &[Color32], alpha: u8, animate: bool) {
    let painter = ui.painter().with_clip_rect(rect);
    let time = ui.input(|input| input.time) as f32;
    let drift = vec2(
        (time * 0.07).sin() * rect.width() * 0.035,
        (time * 0.05).cos() * rect.height() * 0.025,
    );
    let orbs = [
        (
            pos2(
                rect.left() + rect.width() * 0.16,
                rect.top() + rect.height() * 0.22,
            ) + drift,
            rect.width() * 0.52,
        ),
        (
            pos2(
                rect.right() - rect.width() * 0.12,
                rect.top() + rect.height() * 0.34,
            ) + drift * -0.6,
            rect.width() * 0.46,
        ),
        (
            pos2(rect.center().x, rect.bottom() - rect.height() * 0.1) + drift * 0.35,
            rect.width() * 0.62,
        ),
    ];
    for (index, (center, radius)) in orbs.into_iter().enumerate() {
        let color = colors[index % colors.len()];
        // Each ring adds a little, so the centre builds to roughly `alpha`
        // while the edge dissolves into the background.
        for ring in 0..12 {
            let edge = 1.0 - ring as f32 / 12.0;
            let a = ((f32::from(alpha) * 0.07).round() as u8).max(1);
            painter.circle_filled(
                center,
                radius * edge,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
            );
        }
    }
    if animate {
        // The drift is slow; a few frames a second are enough to keep it
        // alive without spending the whole frame budget on the backdrop.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// The credits required beside the words, at the end of the scroll.
fn attribution(
    ui: &mut egui::Ui,
    lyrics: &Lyrics,
    palette: &theme::Palette,
    locale: crate::i18n::Locale,
) {
    let Some(attribution) = &lyrics.attribution else {
        return;
    };
    ui.add_space(24.0);
    let (fill, stroke) = if palette.dark {
        (Color32::from_white_alpha(8), Color32::from_white_alpha(18))
    } else {
        (Color32::from_black_alpha(6), Color32::from_black_alpha(14))
    };
    Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::text(
                ui,
                crate::lyrics::attribution_title(attribution, locale),
                theme::semibold(12.0),
                palette.text.gamma_multiply(0.9),
            );
            if attribution.source.is_empty() {
                ui.add_space(3.0);
                theme::text(
                    ui,
                    gettext(locale, "Source unknown"),
                    theme::regular(11.0),
                    palette.dim,
                );
            }
            if attribution.source == "spicy_lyrics" {
                for (verb, credit) in [
                    (
                        gettext(locale, "Uploaded by"),
                        attribution.uploader.as_ref(),
                    ),
                    (gettext(locale, "Made by"), attribution.maker.as_ref()),
                ] {
                    let Some(credit) = credit else {
                        continue;
                    };
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        theme::text(ui, verb, theme::regular(11.0), palette.dim);
                        let label = egui::RichText::new(&credit.name)
                            .font(theme::semibold(11.0))
                            .color(palette.accent);
                        let response = ui.add(egui::Label::new(label).sense(Sense::click()));
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(if credit.url.is_some() {
                                egui::CursorIcon::PointingHand
                            } else {
                                egui::CursorIcon::Default
                            });
                        }
                        if response.clicked()
                            && let Some(url) = &credit.url
                        {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                    });
                }
            }
            if !lyrics.song_writers.is_empty() {
                ui.add_space(5.0);
                theme::text(
                    ui,
                    gettext(locale, "Written by {names}")
                        .replace("{names}", &lyrics.song_writers.join(", ")),
                    theme::regular(11.0),
                    palette.dim,
                );
            }
        });
    ui.add_space(14.0);
}

/// The player keeps the karaoke fill moving, but only while the panel is on
/// screen and the song is playing.
fn request_word_repaint(
    ctx: &egui::Context,
    now: &crate::app::NowPlaying,
    lyrics: &Lyrics,
    low_performance: bool,
) {
    // The fill is progress, not motion: it keeps moving even when the
    // desktop asks for less animation.
    if now.playing && lyrics.word_synced() {
        if low_performance {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            ctx.request_repaint();
        }
    } else if now.playing && lyrics.synced {
        // Interlude dots still need to breathe on a line-synced track.
        let step = if low_performance { 100 } else { 50 };
        ctx.request_repaint_after(std::time::Duration::from_millis(step));
    }
}

/// A percentage of a base alpha, for the backdrop's dimming controls.
fn scaled_alpha(base: u8, percent: u8) -> u8 {
    (u16::from(base) * u16::from(percent) / 100).min(255) as u8
}

/// A rich blurred artwork backdrop with dark overlay and ambient color orbs,
/// exactly like fullscreen lyrics. Lyrics are an immersive media surface and
/// stay dark and vibrant in both light and dark desktop themes.
fn panel_background(app: &mut App, ui: &egui::Ui) {
    let rect = ui.max_rect();
    let art = app
        .now_playing()
        .and_then(|now| preferred_backdrop_art(now.art_small, now.art_url));
    if let Some(texture) = app
        .lyrics_backdrop
        .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        ui.painter().with_clip_rect(rect).image(
            texture.id(),
            rect,
            cover_uv(rect.size(), texture.size_vec2()),
            Color32::from_gray(160),
        );
    }
    let dim = app.settings.lyrics_background_dim;
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_black_alpha(scaled_alpha(135, dim)));
    let colors = playing_colors(app);
    let playing = app.now_playing().is_some_and(|now| now.playing);
    if !app.settings.lyrics_simple_mode {
        paint_ambient(
            ui,
            rect,
            &colors,
            22,
            playing && !app.settings.lyrics_low_performance,
        );
    }
    widgets::paint_vertical_gradient(
        ui,
        rect,
        Color32::from_black_alpha(scaled_alpha(20, dim)),
        Color32::from_black_alpha(scaled_alpha(100, dim)),
    );
}

pub fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let fit = super::yielding_panel(
        ui.ctx(),
        "lyrics-panel",
        theme::SIDE_PANEL_MIN_WIDTH..=640.0,
        app.settings.lyrics_width,
        ui.available_width() - super::topbar::least_width(ui.ctx()),
    );
    let panel = egui::Panel::right("lyrics-panel")
        .resizable(true)
        .default_size(app.settings.lyrics_width)
        .size_range(fit.range.clone())
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(Color32::from_rgb(18, 18, 20))
                .inner_margin(Margin::symmetric(14, 12)),
        );
    let response = panel.show(ui, |ui| {
        theme::apply_local(ui, &palette);
        panel_background(app, ui);
        let window_controls = super::window_controls_reservation(
            ui.ctx(),
            app.show_queue_panel,
            app.show_lyrics_panel,
            ui.available_width(),
        );
        ui.add_space(window_controls.lyrics_top);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let locale = app.locale;
            let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
            let mut close = false;
            let mut fullscreen = false;
            let mut follow = false;
            // The buttons are measured first so the title truncates into what
            // is left instead of running under them at the narrowest panel.
            egui::Sides::new().shrink_left().spacing(6.0).show(
                ui,
                |ui| {
                    theme::text(
                        ui,
                        gettext(locale, "Now playing"),
                        theme::bold(18.0),
                        palette.text,
                    );
                },
                |ui| {
                    close = theme::icon_button(
                        ui,
                        Icon::X,
                        18.0,
                        palette.secondary,
                        palette.text,
                        &gettext(locale, "Close"),
                    )
                    .clicked();
                    fullscreen = theme::icon_button(
                        ui,
                        Icon::Expand,
                        18.0,
                        palette.secondary,
                        palette.text,
                        &gettext(locale, "Full screen lyrics"),
                    )
                    .clicked();
                    let settings_btn = theme::icon_button(
                        ui,
                        Icon::Settings,
                        18.0,
                        palette.secondary,
                        palette.text,
                        &gettext(locale, "Lyrics settings"),
                    );
                    lyrics_settings_popup(app, &settings_btn, &palette, false);
                    if loaded
                        && !app.lyrics_following
                        && theme::pill_button(
                            ui,
                            &palette,
                            &pgettext(locale, "lyrics", "Follow"),
                            false,
                        )
                        .clicked()
                    {
                        follow = true;
                    }
                },
            );
            if close {
                app.actions.push(Action::ToggleLyricsPanel);
            }
            if fullscreen {
                app.actions.push(Action::SetLyricsFullscreen(true));
            }
            if follow {
                app.lyrics_following = true;
                app.lyrics_line_shown = None;
            }
        });
        ui.add_space(10.0);
        now_playing_stage(app, ui);
        if app.now_playing().is_none() || !app.lyrics_card_collapsed {
            ui.add_space(8.0);
            contents(app, ui);
        }
    });
    let current_width = response.response.rect.width();
    if (app.settings.lyrics_width - current_width).abs() > 1.0
        && super::panel_width_chosen(ui.ctx(), "lyrics-panel", &fit)
    {
        app.settings.lyrics_width = current_width;
        app.actions.push(Action::SettingsChanged);
    }
}

/// A caption that groups the rows of the lyrics popup.
fn lyrics_popup_group(ui: &mut egui::Ui, palette: &theme::Palette, title: &str) {
    ui.add_space(2.0);
    theme::text(ui, title, theme::semibold(11.5), palette.secondary);
    ui.add_space(2.0);
}

/// One line of the lyrics popup: a label on the left, a switch on the right.
fn lyrics_popup_switch(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    label: &str,
    id: &str,
    value: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        theme::text(ui, label, theme::regular(13.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            changed = widgets::switch(ui, palette, id, value).changed();
        });
    });
    ui.add_space(4.0);
    changed
}

/// One line of the lyrics popup: a label on the left and -/value/+ on the
/// right. Returns the new value when the buttons moved it.
fn lyrics_popup_step(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    label: &str,
    value: i32,
    step: i32,
    range: std::ops::RangeInclusive<i32>,
    display: impl Fn(i32) -> String,
) -> Option<i32> {
    let mut now = value;
    ui.horizontal(|ui| {
        theme::text(ui, label, theme::regular(13.0), palette.text);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::soft_button(ui, palette, None, "+", false).clicked() {
                now = (now + step).clamp(*range.start(), *range.end());
            }
            theme::text(ui, display(now), theme::medium(12.0), palette.text);
            if theme::soft_button(ui, palette, None, "-", false).clicked() {
                now = (now - step).clamp(*range.start(), *range.end());
            }
        });
    });
    ui.add_space(4.0);
    (now != value).then_some(now)
}

/// Floating customization menu for lyrics appearance, timing and animation.
fn lyrics_settings_popup(
    app: &mut App,
    response: &egui::Response,
    palette: &theme::Palette,
    in_fullscreen: bool,
) {
    egui::Popup::menu(response)
        .frame(widgets::menu_frame(palette).inner_margin(Margin::same(12)))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let locale = app.locale;
            let mut changed = false;
            ui.set_width(252.0);
            theme::text(
                ui,
                gettext(locale, "Lyrics settings"),
                theme::bold(14.0),
                palette.text,
            );
            ui.add_space(6.0);

            // A preset writes a whole look; every row below stays adjustable.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                for style in LyricsStyle::ALL {
                    if theme::soft_button(
                        ui,
                        palette,
                        None,
                        &style.label(locale),
                        app.settings.lyrics_style == style,
                    )
                    .clicked()
                    {
                        app.settings.apply_lyrics_style(style);
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                for align in LyricsAlign::ALL {
                    if theme::soft_button(
                        ui,
                        palette,
                        None,
                        &align.label(locale),
                        app.settings.lyrics_align == align,
                    )
                    .clicked()
                    {
                        app.settings.lyrics_align = align;
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            widgets::menu_separator(ui, palette);

            let max_height = (ui.ctx().content_rect().height() * 0.62).max(280.0);
            egui::ScrollArea::vertical()
                .id_salt("lyrics-settings-scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    lyrics_popup_group(ui, palette, &gettext(locale, "Look"));
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Text size"),
                        app.settings.lyrics_font_size as i32,
                        10,
                        70..=160,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_font_size = value as u32;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Active line position"),
                        i32::from(app.settings.lyrics_position),
                        5,
                        15..=75,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_position = value as u8;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Line spacing"),
                        i32::from(app.settings.lyrics_spacing),
                        10,
                        50..=200,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_spacing = value as u8;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Inactive brightness"),
                        i32::from(app.settings.lyrics_dim),
                        5,
                        20..=100,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_dim = value as u8;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Focus mode"),
                        "distance_blur",
                        &mut app.settings.lyrics_distance_blur,
                    );
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Focus strength"),
                        i32::from(app.settings.lyrics_focus_strength),
                        5,
                        0..=100,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_focus_strength = value as u8;
                        changed = true;
                    }

                    lyrics_popup_group(ui, palette, &gettext(locale, "Motion"));
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Word bounce"),
                        "bounce",
                        &mut app.settings.lyrics_bounce,
                    );
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Bounce strength"),
                        i32::from(app.settings.lyrics_bounce_strength),
                        10,
                        0..=200,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_bounce_strength = value as u8;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Word glow"),
                        "glow",
                        &mut app.settings.lyrics_glow,
                    );
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Glow strength"),
                        i32::from(app.settings.lyrics_glow_strength),
                        10,
                        0..=200,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_glow_strength = value as u8;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Rise into place"),
                        "rise",
                        &mut app.settings.lyrics_rise,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Active line scale"),
                        "scale",
                        &mut app.settings.lyrics_scale,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Smooth scrolling"),
                        "smooth_scroll",
                        &mut app.settings.lyrics_smooth_scroll,
                    );
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Scroll duration"),
                        app.settings.lyrics_scroll_ms as i32,
                        25,
                        0..=1000,
                        |value| format!("{value} ms"),
                    ) {
                        app.settings.lyrics_scroll_ms = value as u32;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Early scroll"),
                        app.settings.lyrics_early_scroll_ms as i32,
                        25,
                        0..=800,
                        |value| format!("{value} ms"),
                    ) {
                        app.settings.lyrics_early_scroll_ms = value as u32;
                        changed = true;
                    }

                    lyrics_popup_group(ui, palette, &gettext(locale, "Words"));
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Word fade"),
                        i32::from(app.settings.lyrics_fade_width),
                        5,
                        0..=100,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_fade_width = value as u8;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Line fade duration"),
                        app.settings.lyrics_line_fade_ms as i32,
                        25,
                        0..=1000,
                        |value| format!("{value} ms"),
                    ) {
                        app.settings.lyrics_line_fade_ms = value as u32;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Join split words"),
                        "join_words",
                        &mut app.settings.lyrics_join_words,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Duet padding"),
                        "duet_padding",
                        &mut app.settings.lyrics_duet_padding,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Background vocal pop-out"),
                        "background_pop",
                        &mut app.settings.lyrics_background_pop,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Translations"),
                        "translations",
                        &mut app.settings.lyrics_show_translations,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Credits"),
                        "credits",
                        &mut app.settings.lyrics_show_credits,
                    );

                    lyrics_popup_group(ui, palette, &gettext(locale, "Scene"));
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Backdrop dimming"),
                        i32::from(app.settings.lyrics_background_dim),
                        5,
                        0..=100,
                        |value| format!("{value}%"),
                    ) {
                        app.settings.lyrics_background_dim = value as u8;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Cover colours"),
                        "cover_color",
                        &mut app.settings.lyrics_cover_color,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Hide passed lines"),
                        "hide_passed",
                        &mut app.settings.lyrics_hide_passed_lines,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Simple mode"),
                        "simple_mode",
                        &mut app.settings.lyrics_simple_mode,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Minimal mode"),
                        "minimal_mode",
                        &mut app.settings.lyrics_minimal_mode,
                    );
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "Low performance"),
                        "low_performance",
                        &mut app.settings.lyrics_low_performance,
                    );

                    lyrics_popup_group(ui, palette, &gettext(locale, "Timing"));
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Line hold"),
                        app.settings.lyrics_line_hold_ms as i32,
                        50,
                        0..=500,
                        |value| format!("{value} ms"),
                    ) {
                        app.settings.lyrics_line_hold_ms = value as u32;
                        changed = true;
                    }
                    if let Some(value) = lyrics_popup_step(
                        ui,
                        palette,
                        &gettext(locale, "Timing offset"),
                        app.settings.lyrics_offset_ms,
                        25,
                        -500..=500,
                        |value| format!("{value:+} ms"),
                    ) {
                        app.settings.lyrics_offset_ms = value;
                        changed = true;
                    }
                    changed |= lyrics_popup_switch(
                        ui,
                        palette,
                        &gettext(locale, "In-window full screen"),
                        "windowed_fullscreen",
                        &mut app.settings.lyrics_windowed_fullscreen,
                    );
                });

            ui.add_space(6.0);
            widgets::menu_separator(ui, palette);
            if widgets::menu_item(
                ui,
                palette,
                Some(Icon::Settings),
                &gettext(locale, "All settings..."),
            ) {
                if in_fullscreen {
                    app.actions.push(Action::SetLyricsFullscreen(false));
                }
                app.actions.push(Action::Open(Page::Settings));
            }
            if changed {
                app.actions.push(Action::SettingsChanged);
            }
        });
}

/// The playback context, cover stage with bottom gradient fade, track metadata with
/// save action, and collapsible lyrics card header.
fn now_playing_stage(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let Some(now) = app.now_playing() else {
        return;
    };
    let saved = !now.is_episode && app.is_saved(&now.uri).unwrap_or(false);
    let save_tooltip = if saved {
        gettext(app.locale, "Remove from Liked Songs")
    } else {
        gettext(app.locale, "Save to Liked Songs")
    };
    let mut toggle_save = false;

    if app.lyrics_card_collapsed {
        if let Some(from) = app.playing_from() {
            let label = gettext(app.locale, "Playing from").to_uppercase();
            ui.horizontal(|ui| {
                theme::text(ui, label, theme::semibold(10.5), palette.secondary);
                match from.page {
                    Some(page) => {
                        if theme::link(ui, from.name, theme::semibold(11.0), palette.text).clicked()
                        {
                            app.actions.push(Action::Open(page));
                        }
                    }
                    None => {
                        theme::text(ui, from.name, theme::semibold(11.0), palette.text);
                    }
                }
            });
            ui.add_space(8.0);
        }

        let width = ui.available_width().max(60.0);
        let stage_height = width.min((ui.available_height() - 140.0).max(120.0));
        let (stage_rect, _) = ui.allocate_exact_size(vec2(width, stage_height), Sense::hover());
        let radius = theme::RADIUS as f32;
        widgets::playing_cover(
            ui,
            &palette,
            app.backend.art(),
            "lyrics-stage-cover",
            app.small_art(&now, width),
            stage_rect,
            radius,
        );
        ui.painter()
            .rect_filled(stage_rect, radius, Color32::from_black_alpha(51));
        let fade_top = stage_rect.top() + stage_rect.height() * 0.55;
        let fade_rect =
            Rect::from_min_max(pos2(stage_rect.left(), fade_top), stage_rect.right_bottom());
        widgets::paint_vertical_gradient(
            ui,
            fade_rect,
            Color32::TRANSPARENT,
            Color32::from_black_alpha(230),
        );

        ui.add_space(8.0);

        egui::Sides::new().shrink_left().spacing(8.0).show(
            ui,
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&now.title)
                                .font(theme::bold(15.0))
                                .color(palette.text),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&now.subtitle)
                                .font(theme::regular(12.5))
                                .color(palette.secondary),
                        )
                        .truncate(),
                    );
                });
            },
            |ui| {
                if !now.is_episode {
                    let (icon, color) = if saved {
                        (Icon::HeartFilled, palette.accent)
                    } else {
                        (Icon::Heart, palette.secondary)
                    };
                    if theme::icon_button(ui, icon, 18.0, color, palette.text, &save_tooltip)
                        .clicked()
                    {
                        toggle_save = true;
                    }
                }
            },
        );

        ui.add_space(10.0);
    } else {
        egui::Sides::new().shrink_left().spacing(8.0).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    let size = 56.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
                    widgets::playing_cover(
                        ui,
                        &palette,
                        app.backend.art(),
                        "lyrics-now-playing",
                        app.small_art(&now, size),
                        rect,
                        theme::RADIUS as f32,
                    );
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&now.title)
                                    .font(theme::semibold(15.0))
                                    .color(palette.text),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&now.subtitle)
                                    .font(theme::regular(12.0))
                                    .color(palette.secondary),
                            )
                            .truncate(),
                        );
                    });
                });
            },
            |ui| {
                if !now.is_episode {
                    let (icon, color) = if saved {
                        (Icon::HeartFilled, palette.accent)
                    } else {
                        (Icon::Heart, palette.secondary)
                    };
                    if theme::icon_button(ui, icon, 18.0, color, palette.text, &save_tooltip)
                        .clicked()
                    {
                        toggle_save = true;
                    }
                }
            },
        );

        ui.add_space(6.0);
    }

    if toggle_save {
        app.actions.push(Action::ToggleSaved(now.uri));
    }

    let card_header_height = 26.0;
    let (card_rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(1.0), card_header_height),
        Sense::click(),
    );
    let card_hovered = response.hovered();
    let header_bg = if card_hovered {
        Color32::from_white_alpha(20)
    } else {
        Color32::from_white_alpha(10)
    };
    ui.painter()
        .rect_filled(card_rect, CornerRadius::same(6), header_bg);
    let header_ui_rect = card_rect.shrink2(vec2(8.0, 0.0));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(header_ui_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            let chevron = if app.lyrics_card_collapsed {
                Icon::ChevronDown
            } else {
                Icon::ChevronUp
            };
            theme::paint_icon(
                ui,
                chevron,
                Rect::from_center_size(
                    pos2(header_ui_rect.left() + 8.0, header_ui_rect.center().y),
                    vec2(14.0, 14.0),
                ),
                14.0,
                palette.secondary,
            );
            ui.add_space(20.0);
            theme::text(
                ui,
                gettext(app.locale, "Lyrics"),
                theme::semibold(13.0),
                palette.text,
            );
        },
    );
    if response.clicked() {
        app.actions.push(Action::ToggleLyricsCard);
    }
}

fn contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
            } else {
                widgets::loading_row(ui, &palette, app.locale);
            }
            return;
        }
        Loadable::Failed(error) => {
            if !app.network_reachable {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::WifiOff,
                    &gettext(app.locale, "You're offline"),
                    &gettext(
                        app.locale,
                        "Spotifast will reconnect automatically when your connection returns.",
                    ),
                );
                return;
            }
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.secondary);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.request_lyrics();
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics
            .clone()
            .with_interludes(app.settings.lyrics_minimal_mode),
    };

    let position_ms = (now.position_ms as i64 + app.settings.lyrics_offset_ms as i64).max(0) as u32;
    let active_at = position_ms.saturating_add(app.settings.lyrics_early_scroll_ms);
    let active = lyrics.active_line_with_hold(active_at, app.settings.lyrics_line_hold_ms);
    let follow = app.lyrics_following && app.lyrics_line_shown != Some(active);
    let motion = MotionLook::from_settings(&app.settings, 22);
    let animation = if app.settings.lyrics_smooth_scroll {
        crate::motion::scroll(ui.ctx(), app.settings.lyrics_scroll_ms as f32 / 1000.0)
    } else {
        egui::style::ScrollAnimation::none()
    };
    let font_scale = (app.settings.lyrics_font_size as f32 / 100.0).clamp(0.7, 1.8);
    let line_size = LINE_SIZE * font_scale;
    let line_gap = LINE_GAP * font_scale * (f32::from(app.settings.lyrics_spacing) / 100.0);
    let active_color = if app.settings.lyrics_cover_color && !app.settings.lyrics_simple_mode {
        blend(playing_colors(app)[0], Color32::WHITE, 0.45)
    } else {
        Color32::WHITE
    };
    let base_quiet = palette.text.gamma_multiply(0.35);
    let base_sung = palette.text.gamma_multiply(0.48);
    let base_active = palette.text.gamma_multiply(0.40);
    ui.spacing_mut().scroll.fade.strength = 0.0;
    ui.spacing_mut().scroll.fade.size = 0.0;
    let scroll = crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("lyrics-scroll")
            .auto_shrink([false, false]),
        egui::Vec2b::new(false, true),
        |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                    animation,
                );
            }
            ui.add_space(16.0);
            // The scroll area's clip is the panel's inner edge; available
            // width can include the scrollbar's lane, which put the ends of
            // long lines under the margin.
            let wrap = (ui.clip_rect().right() - ui.cursor().min.x).max(1.0);
            for (index, line) in lyrics.lines.iter().enumerate() {
                let is_active = active == Some(index);
                let lit = ui.ctx().animate_bool_with_time(
                    egui::Id::new("lyric-line").with(index),
                    is_active,
                    crate::motion::seconds(ui.ctx(), motion.line_fade),
                );
                let lit_eased = lit * lit * (3.0 - 2.0 * lit);
                let distance = active.map_or(0, |active_idx| index.abs_diff(active_idx));
                let focus = if app.settings.lyrics_distance_blur {
                    motion.focus_factor(distance)
                } else {
                    1.0
                };
                let dim = focus * motion.dim;
                let quiet = base_quiet.gamma_multiply(dim);
                let sung = base_sung.gamma_multiply(dim);
                let active_base = base_active.gamma_multiply(focus);
                let color = blend(quiet, active_color, lit_eased);
                let rise = if motion.rise && is_active {
                    (1.0 - lit_eased) * line_size * 0.12
                } else {
                    0.0
                };
                let look = motion.line(
                    if is_active {
                        theme::lyrics_sung(line_size)
                    } else {
                        theme::lyrics_quiet(line_size)
                    },
                    if is_active {
                        active_base
                    } else if line.at_ms.is_some_and(|at| at < position_ms) {
                        sung
                    } else {
                        quiet
                    },
                    active_color,
                    wrap,
                    rise,
                );
                let indent = if motion.duet_padding && line.opposite_aligned {
                    (wrap * 0.07).min(56.0)
                } else {
                    0.0
                };
                if line.text.is_empty()
                    && !line.background.is_empty()
                    && background_pop(ui, line, position_ms, is_active, &look, index) <= 0.01
                {
                    continue;
                }
                let response =
                    if line.text.is_empty() && line.background.is_empty() && lyrics.synced {
                        paint_interlude(
                            ui,
                            is_active,
                            color,
                            look.glow,
                            line_size,
                            wrap,
                            now.playing,
                            line.at_ms.unwrap_or(0),
                            line.end_ms.unwrap_or(0),
                            position_ms,
                        )
                    } else if word_line(line) {
                        paint_word_line(ui, line, position_ms, is_active, &look, index).0
                    } else {
                        let sense = if lyrics.synced {
                            Sense::click()
                        } else {
                            Sense::hover()
                        };
                        paint_plain_line(ui, &line.text, &look, color, is_active, indent, sense)
                    };
                let response = if lyrics.synced {
                    response.on_hover_cursor(egui::CursorIcon::PointingHand)
                } else {
                    response
                };
                crate::autoscroll::row(ui, &response);
                let rect = response.rect;
                if lyrics.synced
                    && response.clicked()
                    && let Some(at_ms) = line.at_ms
                {
                    app.actions.push(Action::Seek(at_ms));
                    app.lyrics_following = true;
                }
                if is_active && app.lyrics_following {
                    show_sung_line(ui, rect, SUNG_LINE_AT, Some(animation));
                }
                ui.add_space(line_gap);
            }
            if app.settings.lyrics_show_credits {
                attribution(ui, &lyrics, &palette, app.locale);
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if app.lyrics_following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                    animation,
                );
            }
            // Room for the last line to rise to where a sung line sits.
            ui.add_space((ui.clip_rect().height() * (1.0 - SUNG_LINE_AT)).max(60.0));
        },
    );
    crate::autoscroll::lyrics(ui, scroll.id);
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if ui.rect_contains_pointer(scroll.inner_rect)
        && ui.input(|input| input.smooth_scroll_delta.y != 0.0)
    {
        app.lyrics_following = false;
    }
    request_word_repaint(ui.ctx(), &now, &lyrics, app.settings.lyrics_low_performance);
    app.lyrics_line_shown = Some(active);
}

pub fn fullscreen(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(theme::Palette::dark().window))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            background(app, ui, rect);
            // The empty space is the backdrop's switch, as the player bar's
            // is its visualizer's. Everything drawn after this takes its own
            // clicks, and a drag is no click.
            #[cfg(feature = "milkdrop")]
            if ui
                .interact(rect, ui.id().with("lyrics-backdrop"), Sense::click())
                .clicked()
            {
                app.actions.push(Action::ToggleLyricsMilkdrop);
            }
            let top = theme::titlebar_inset(ui.ctx()) + 24.0;
            if app.now_playing().is_some() && rect.width() >= COVER_BESIDE_MIN_WIDTH {
                with_cover(app, ui, rect, top);
                return;
            }
            let width = fullscreen_content_width(rect.width());
            let region = Rect::from_min_max(
                pos2(rect.center().x - width / 2.0, rect.top() + top),
                pos2(rect.center().x + width / 2.0, rect.bottom()),
            );
            let mut content = ui.new_child(UiBuilder::new().max_rect(region));
            fullscreen_header(app, &mut content);
            content.add_space(20.0);
            track_heading(app, &mut content);
            content.add_space(16.0);
            fullscreen_contents(app, &mut content);
        });
}

/// The widest the lyrics get beside the cover, so lines stay easy to read.
const LYRICS_BESIDE_WIDTH: f32 = 640.0;

/// The narrowest window that shows the cover beside the lyrics; narrower
/// ones keep a single column with a small cover in the heading.
const COVER_BESIDE_MIN_WIDTH: f32 = 900.0;

/// Full screen with the cover large: beside the lyrics when there are
/// words to follow, and alone in the middle when there are none, a calm
/// view of what is playing.
fn with_cover(app: &mut App, ui: &mut egui::Ui, rect: Rect, top: f32) {
    let outer = Rect::from_min_max(
        pos2(rect.left() + 48.0, rect.top() + top),
        pos2(rect.right() - 48.0, rect.bottom() - 40.0),
    );
    let mut header = ui.new_child(UiBuilder::new().max_rect(outer));
    fullscreen_header(app, &mut header);
    let below = Rect::from_min_max(
        pos2(outer.left(), header.min_rect().bottom() + 24.0),
        outer.max,
    );
    // The cover moves aside only for words to read. While they load it
    // stays in the middle, so a song that turns out to have none never
    // moves at all.
    let words = matches!(&app.lyrics, Loadable::Loaded(Some(lyrics)) if !lyrics.instrumental);
    // The queue docks right while its flag is on, in both arrangements.
    // Without it the room below is untouched, preserving the stock layout.
    let gap = 64.0;
    let queue_width = if app.show_queue_panel {
        // Shared with the side panel's persisted width; the column's drag
        // strip below writes the same field.
        app.settings.queue_width.clamp(
            super::queue::FULLSCREEN_COLUMN_MIN,
            super::queue::FULLSCREEN_COLUMN_MAX,
        )
    } else {
        0.0
    };
    let queue_gap = if app.show_queue_panel { gap } else { 0.0 };
    let lyrics_end = below.right() - queue_width - queue_gap;
    if app.show_queue_panel {
        let queue_rect = Rect::from_min_max(
            pos2(below.right() - queue_width, below.top()),
            pos2(below.right(), below.bottom()),
        );
        let mut queue_ui = ui.new_child(UiBuilder::new().max_rect(queue_rect));
        super::queue::fullscreen_column(app, &mut queue_ui);
    }
    if words {
        if app.show_queue_panel {
            // The cover docks left; the lyrics take the middle that remains.
            let room = lyrics_end - below.left();
            let side = (room * 0.42).min(below.height() - 90.0).clamp(160.0, 520.0);
            let column = Rect::from_min_size(
                pos2(below.left(), below.center().y - (side + 90.0) / 2.0),
                vec2(side, side + 90.0),
            );
            big_cover(app, ui, column, Align::Min);
            let lyrics_avail = lyrics_end - column.right() - gap;
            let lyrics_width = lyrics_avail.clamp(80.0, LYRICS_BESIDE_WIDTH);
            // Centre the block between the cover and the queue when there is
            // room to spare; it hugs the cover on narrow windows.
            let lyrics_x = column.right() + gap + (lyrics_avail - lyrics_width).max(0.0) / 2.0;
            let lyrics = Rect::from_min_max(
                pos2(lyrics_x, below.top()),
                pos2(lyrics_x + lyrics_width, below.bottom()),
            );
            let mut content = ui.new_child(UiBuilder::new().max_rect(lyrics));
            fullscreen_contents(app, &mut content);
        } else {
            // The cover and the lyrics are one group, centred in the window.
            let side = (below.width() * 0.38)
                .min(below.height() - 90.0)
                .clamp(200.0, 520.0);
            let lyrics_width = (below.width() - side - gap).min(LYRICS_BESIDE_WIDTH);
            let left = below.center().x - (side + gap + lyrics_width) / 2.0;
            let column = Rect::from_min_size(
                pos2(left, below.center().y - (side + 90.0) / 2.0),
                vec2(side, side + 90.0),
            );
            big_cover(app, ui, column, Align::Min);
            let lyrics = Rect::from_min_max(
                pos2(column.right() + gap, below.top()),
                pos2(column.right() + gap + lyrics_width, below.bottom()),
            );
            let mut content = ui.new_child(UiBuilder::new().max_rect(lyrics));
            fullscreen_contents(app, &mut content);
        }
    } else {
        let span = Rect::from_min_max(
            pos2(below.left(), below.top()),
            pos2(lyrics_end, below.bottom()),
        );
        // Without the queue the span is the whole room, matching the stock
        // centred cover exactly.
        let min_side = if app.show_queue_panel { 160.0 } else { 200.0 };
        let side = (span.height() - 140.0)
            .min(span.width() * 0.5)
            .clamp(min_side, 560.0);
        let column = Rect::from_center_size(span.center(), vec2(side, side + 90.0));
        big_cover(app, ui, column, Align::Center);
        // Why there are no words, quietly, under the song, or that they
        // are still being fetched.
        let (heading, detail) = match &app.lyrics {
            Loadable::Loaded(Some(_)) => (
                gettext(app.locale, "Instrumental"),
                gettext(app.locale, "No timed lyrics for this track."),
            ),
            Loadable::Loaded(None) => (
                gettext(app.locale, "No lyrics"),
                gettext(app.locale, "No lyrics found for this track."),
            ),
            Loadable::Failed(error) => (
                gettext(
                    app.locale,
                    // Translators: Keep {error} unchanged. It is the original failure detail.
                    "Couldn't fetch the lyrics: {error}",
                )
                .replace("{error}", error)
                .into(),
                Default::default(),
            ),
            Loadable::NotLoaded | Loadable::Loading => {
                (gettext(app.locale, "Loading…"), Default::default())
            }
        };
        let heading = ui.painter().text(
            pos2(column.center().x, column.bottom() + 8.0),
            egui::Align2::CENTER_TOP,
            heading,
            theme::semibold(13.0),
            Color32::from_gray(200),
        );
        if matches!(app.lyrics, Loadable::Failed(_)) {
            let retry = Rect::from_center_size(
                pos2(column.center().x, heading.bottom() + 24.0),
                vec2(column.width(), 32.0),
            );
            let mut retry_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(retry)
                    .layout(Layout::top_down(Align::Center)),
            );
            let label = gettext(app.locale, "Try again");
            if theme::pill_button(&mut retry_ui, &theme::Palette::dark(), &label, false).clicked() {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        ui.painter().text(
            pos2(column.center().x, heading.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            detail,
            theme::regular(13.0),
            Color32::from_gray(170),
        );
    }
}

/// The playing song's cover filling the top of `column`, with its title and
/// artists beneath, aligned to its left edge or centred.
fn big_cover(app: &mut App, ui: &mut egui::Ui, column: Rect, align: Align) {
    let Some(now) = app.now_playing() else {
        return;
    };
    let side = column.width();
    let cover = Rect::from_min_size(column.min, vec2(side, side));
    let radius = 10.0;
    ui.painter().add(
        egui::epaint::Shadow {
            offset: [0, 18],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(140),
        }
        .as_shape(cover, radius),
    );
    widgets::playing_cover(
        ui,
        &theme::Palette::dark(),
        app.backend.art(),
        "fullscreen-lyrics",
        now.art_url.as_deref().or(now.art_small.as_deref()),
        cover,
        radius,
    );
    let art = now.art_url.is_some() || now.art_small.is_some();
    cover_opens_immersive(app, ui, cover, art);
    let words = Rect::from_min_max(pos2(column.left(), cover.bottom() + 18.0), column.max);
    let mut text = ui.new_child(
        UiBuilder::new()
            .max_rect(words)
            .layout(Layout::top_down(align)),
    );
    text.spacing_mut().item_spacing.y = 4.0;
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.title)
                .font(theme::semibold(22.0))
                .color(Color32::WHITE),
        )
        .truncate(),
    );
    text.add(
        egui::Label::new(
            egui::RichText::new(&now.subtitle)
                .font(theme::regular(14.0))
                .color(Color32::from_gray(225)),
        )
        .truncate(),
    );
}

fn fullscreen_content_width(viewport_width: f32) -> f32 {
    let available = (viewport_width - 48.0).max(0.0);
    (viewport_width * 0.72).clamp(400.0, 960.0).min(available)
}

fn preferred_backdrop_art(small: Option<String>, large: Option<String>) -> Option<String> {
    small.or(large)
}

/// How bright the brightest of the picture may stay behind the words, so a
/// white preset reads like the dimmed art does.
const BACKDROP_GLOW: f32 = 0.2;
/// The dimming's limits: even a dark preset's sparks stay this far under the
/// white of the words, and a bright one is never lost altogether.
const BACKDROP_DIM: std::ops::RangeInclusive<f32> = 0.2..=0.32;

/// How much of the picture's brightness to keep, for one this bright.
fn backdrop_dim(glow: f32) -> f32 {
    (BACKDROP_GLOW / glow.max(0.01)).clamp(*BACKDROP_DIM.start(), *BACKDROP_DIM.end())
}

/// MilkDrop filling `rect`, dimmed so the words stay clear whatever it draws.
/// `false` leaves the art: there is no picture yet, or none to be had.
#[cfg(feature = "milkdrop")]
fn milkdrop_backdrop(app: &mut App, ui: &mut egui::Ui, rect: Rect) -> bool {
    use crate::milkdrop::pane::{Picture, UV};
    if !app.milkdrop_behind_lyrics() {
        return false;
    }
    let ctx = ui.ctx().clone();
    let pixels = rect.size() * ctx.pixels_per_point();
    let picture = app.milkdrop_picture(&ctx, [pixels.x, pixels.y]);
    let Picture::Frame(texture) = picture else {
        if picture == Picture::Waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            // No MilkDrop to be had: as if the art was never clicked.
            app.actions.push(Action::LeaveImmersive);
        }
        return false;
    };
    app.immersive_arrived();
    let dim = ctx.animate_value_with_time(
        egui::Id::new("lyrics-backdrop-dim"),
        backdrop_dim(app.milkdrop_glow()),
        crate::motion::seconds(&ctx, 0.5),
    );
    let tint = Color32::from_gray((dim * 255.0).round() as u8);
    ui.painter()
        .with_clip_rect(rect)
        .image(texture, rect, UV, tint);
    if app.now_playing().is_some_and(|now| now.playing) {
        theme::visualiser_frame(&ctx, Some(app.milkdrop_pane_fps()));
    }
    true
}

/// MilkDrop alone, edge to edge and at full brightness, under the root of the
/// window so that the player bar comes and goes over it without resizing it.
/// Returns whether the controls show.
#[cfg(feature = "milkdrop")]
pub fn immersive(app: &mut App, ui: &mut egui::Ui) -> bool {
    use crate::milkdrop::pane::{Picture, UV};
    let ctx = ui.ctx().clone();
    let rect = ctx.viewport_rect();
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    let pixels = rect.size() * ctx.pixels_per_point();
    match app.milkdrop_picture(&ctx, [pixels.x, pixels.y]) {
        Picture::Frame(texture) => {
            ui.painter().image(texture, rect, UV, Color32::WHITE);
        }
        Picture::Waiting => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Picture::None => app.actions.push(Action::LeaveImmersive),
    }
    if app.now_playing().is_some_and(|now| now.playing) {
        theme::visualiser_frame(&ctx, Some(app.milkdrop_pane_fps()));
    }
    let moved = ctx.input(|input| input.pointer.delta() != egui::Vec2::ZERO);
    let now = ctx.input(|input| input.time);
    let left = app.immersive_controls(moved, now);
    match left {
        Some(left) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(left)),
        None => ctx.set_cursor_icon(egui::CursorIcon::None),
    }
    left.is_some()
}

/// Everything in immersive mode that is not a control: any click there brings
/// the view back.
#[cfg(feature = "milkdrop")]
pub fn immersive_click(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new())
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if ui
                .interact(rect, ui.id().with("immersive"), Sense::click())
                .clicked()
            {
                app.actions.push(Action::LeaveImmersive);
            }
        });
}

/// The cover is the way into immersive mode, if there is MilkDrop to go to.
fn cover_opens_immersive(app: &mut App, ui: &egui::Ui, cover: Rect, art: bool) {
    #[cfg(feature = "milkdrop")]
    if art
        && ui
            .interact(cover, ui.id().with("lyrics-cover"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    {
        app.actions.push(Action::EnterImmersive);
    }
    #[cfg(not(feature = "milkdrop"))]
    let _ = (app, ui, cover, art);
}

fn background(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    theme::apply_local(ui, &theme::Palette::dark());
    #[cfg(feature = "milkdrop")]
    if milkdrop_backdrop(app, ui, rect) {
        return;
    }
    let art = app
        .now_playing()
        .and_then(|now| preferred_backdrop_art(now.art_small, now.art_url));
    let painter = ui.painter().with_clip_rect(rect);
    if let Some(texture) = app
        .lyrics_backdrop
        .texture(ui.ctx(), app.backend.art(), art.as_deref())
    {
        painter.image(
            texture.id(),
            rect,
            cover_uv(rect.size(), texture.size_vec2()),
            Color32::from_gray(170),
        );
    }
    painter.rect_filled(
        rect,
        0.0,
        Color32::from_black_alpha(scaled_alpha(128, app.settings.lyrics_background_dim)),
    );
    let colors = playing_colors(app);
    let playing = app.now_playing().is_some_and(|now| now.playing);
    if !app.settings.lyrics_simple_mode {
        paint_ambient(
            ui,
            rect,
            &colors,
            22,
            playing && !app.settings.lyrics_low_performance,
        );
    }
    let dim = app.settings.lyrics_background_dim;
    widgets::paint_vertical_gradient(
        ui,
        rect,
        Color32::from_black_alpha(scaled_alpha(30, dim)),
        Color32::from_black_alpha(scaled_alpha(110, dim)),
    );
}

fn cover_uv(view: egui::Vec2, image: egui::Vec2) -> Rect {
    let ratio = (view.x / view.y.max(1.0)) / (image.x / image.y.max(1.0));
    let size = if ratio > 1.0 {
        vec2(1.0, 1.0 / ratio)
    } else {
        vec2(ratio, 1.0)
    };
    Rect::from_center_size(pos2(0.5, 0.5), size)
}

fn fullscreen_header(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    ui.horizontal(|ui| {
        theme::text(
            ui,
            gettext(app.locale, "Lyrics"),
            theme::bold(18.0),
            palette.text,
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::icon_button(
                ui,
                Icon::Shrink,
                18.0,
                palette.text,
                palette.text,
                &gettext(app.locale, "Leave full screen (Esc)"),
            )
            .clicked()
            {
                app.actions.push(Action::SetLyricsFullscreen(false));
            }
            let settings_btn = theme::icon_button(
                ui,
                Icon::Settings,
                18.0,
                palette.text,
                palette.text,
                &gettext(app.locale, "Lyrics settings"),
            );
            lyrics_settings_popup(app, &settings_btn, &palette, true);
            let loaded = matches!(&app.lyrics, Loadable::Loaded(Some(_)));
            if loaded
                && !app.lyrics_following
                && theme::pill_button(
                    ui,
                    &palette,
                    &pgettext(app.locale, "lyrics", "Follow"),
                    false,
                )
                .clicked()
            {
                app.actions.push(Action::FollowLyrics);
            }
        });
    });
}

fn track_heading(app: &mut App, ui: &mut egui::Ui) {
    if let Some(now) = app.now_playing() {
        ui.horizontal(|ui| {
            let size = 52.0;
            let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
            widgets::playing_cover(
                ui,
                &theme::Palette::dark(),
                app.backend.art(),
                "lyrics-heading",
                app.small_art(&now, size),
                rect,
                4.0,
            );
            let art = now.art_url.is_some() || now.art_small.is_some();
            cover_opens_immersive(app, ui, rect, art);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&now.title)
                            .font(theme::semibold(22.0))
                            .color(Color32::WHITE),
                    )
                    .truncate(),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&now.subtitle)
                            .font(theme::regular(13.0))
                            .color(Color32::from_gray(235)),
                    )
                    .truncate(),
                );
            });
        });
    }
}

fn fullscreen_contents(app: &mut App, ui: &mut egui::Ui) {
    let palette = theme::Palette::dark();
    let Some(now) = app.now_playing() else {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Mic,
            &gettext(app.locale, "Nothing playing"),
            &gettext(app.locale, "Play a song to see its lyrics."),
        );
        return;
    };
    let lyrics = match &app.lyrics {
        Loadable::NotLoaded | Loadable::Loading => {
            widgets::loading_row(ui, &palette, app.locale);
            return;
        }
        Loadable::Failed(error) => {
            let message = gettext(
                app.locale,
                // Translators: Keep {error} unchanged. It is the original failure detail.
                "Couldn't fetch the lyrics: {error}",
            )
            .replace("{error}", error);
            ui.add_space(8.0);
            theme::text(ui, message, theme::regular(13.0), palette.text);
            ui.add_space(8.0);
            if theme::pill_button(ui, &palette, &gettext(app.locale, "Try again"), false).clicked()
            {
                app.actions.push(Action::RetryLyrics);
            }
            return;
        }
        Loadable::Loaded(None) => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Mic,
                &gettext(app.locale, "No lyrics"),
                &gettext(app.locale, "No lyrics found for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) if lyrics.instrumental => {
            widgets::empty_state(
                ui,
                &palette,
                Icon::Music,
                &gettext(app.locale, "Instrumental"),
                &gettext(app.locale, "No timed lyrics for this track."),
            );
            return;
        }
        Loadable::Loaded(Some(lyrics)) => lyrics
            .clone()
            .with_interludes(app.settings.lyrics_minimal_mode),
    };

    let position_ms = (now.position_ms as i64 + app.settings.lyrics_offset_ms as i64).max(0) as u32;
    let active_at = position_ms.saturating_add(app.settings.lyrics_early_scroll_ms);
    let active = lyrics.active_line_with_hold(active_at, app.settings.lyrics_line_hold_ms);
    // A Follow click resets the remembered line after drawing. Other frames
    // record the shown line before any line-click action restores following.
    if !app
        .actions
        .iter()
        .any(|action| matches!(action, Action::FollowLyrics))
    {
        app.actions.push(Action::LyricsLineShown(active));
    }
    let viewport = ui.available_rect_before_wrap();
    let manual_scroll = ui.rect_contains_pointer(viewport)
        && ui.input(|input| {
            input.smooth_scroll_delta.y != 0.0
                || (input.pointer.primary_down() && input.pointer.delta().y != 0.0)
        });
    let following = app.lyrics_following && !manual_scroll;
    let follow = following && app.lyrics_line_shown != Some(active);
    let motion = MotionLook::from_settings(
        &app.settings,
        if app.milkdrop_behind_lyrics() { 48 } else { 36 },
    );
    let hide_passed = app.settings.lyrics_hide_passed_lines || app.settings.lyrics_minimal_mode;
    let animation = if app.settings.lyrics_smooth_scroll {
        crate::motion::scroll(ui.ctx(), app.settings.lyrics_scroll_ms as f32 / 1000.0)
    } else {
        egui::style::ScrollAnimation::none()
    };
    let font_scale = (app.settings.lyrics_font_size as f32 / 100.0).clamp(0.7, 1.8);
    let size = (ui.available_width() * 0.052).clamp(31.0, 52.0) * font_scale;
    let active_at_fraction = (f32::from(app.settings.lyrics_position) / 100.0).clamp(0.15, 0.75);
    let line_gap = 32.0 * (f32::from(app.settings.lyrics_spacing) / 100.0);
    // The line being sung is pure white and grows, as Spicy Lyrics draws it;
    // each line wraps for its full size all along, so growing can't rewrap
    // its words. A moving picture asks more of the quiet lines than a still
    // one.
    let active_color = if app.settings.lyrics_cover_color && !app.settings.lyrics_simple_mode {
        blend(playing_colors(app)[0], Color32::WHITE, 0.45)
    } else {
        Color32::WHITE
    };
    let base_quiet = palette
        .text
        .gamma_multiply(if app.milkdrop_behind_lyrics() {
            0.56
        } else {
            0.34
        });
    let base_sung = palette
        .text
        .gamma_multiply(if app.milkdrop_behind_lyrics() {
            0.66
        } else {
            0.44
        });
    let base_active = palette
        .text
        .gamma_multiply(if app.milkdrop_behind_lyrics() {
            0.60
        } else {
            0.40
        });
    ui.spacing_mut().scroll.fade.strength = 0.0;
    ui.spacing_mut().scroll.fade.size = 0.0;
    egui::ScrollArea::vertical()
        .id_salt(("fullscreen-lyrics-scroll", &now.uri))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Before the first line there is nothing to highlight, so the
            // panel sits at the top rather than wherever it was left.
            if follow && lyrics.synced && active.is_none() {
                let top = ui.cursor().min;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_size(top, egui::vec2(1.0, 1.0)),
                    Some(Align::Min),
                    animation,
                );
            }
            // Room for the first line to sit where a sung line sits, and for
            // the last to rise to it.
            let (padding, below) = if lyrics.synced {
                (
                    (viewport.height() * active_at_fraction - size).max(12.0),
                    viewport.height() * (1.0 - active_at_fraction),
                )
            } else {
                (12.0, 60.0)
            };
            ui.add_space(padding);
            for (index, line) in lyrics.lines.iter().enumerate() {
                let is_active = active == Some(index);
                let lit = ui.ctx().animate_bool_with_time(
                    egui::Id::new("lyric-line").with(("fullscreen", &now.uri, index)),
                    is_active,
                    crate::motion::seconds(ui.ctx(), motion.line_fade),
                );
                let lit_eased = lit * lit * (3.0 - 2.0 * lit);
                let passed = active.is_some_and(|active_idx| index < active_idx);
                // A minimal view lets a passed line fade right out, then
                // stops giving it room, so only the song ahead is left.
                if hide_passed && passed && lit < 0.02 {
                    continue;
                }
                let distance = active.map_or(0, |active_idx| index.abs_diff(active_idx));
                let focus = if app.settings.lyrics_distance_blur {
                    motion.focus_factor(distance)
                } else {
                    1.0
                };
                let dim = focus * motion.dim;
                let quiet = base_quiet.gamma_multiply(dim);
                let sung = base_sung.gamma_multiply(dim);
                let active_base = base_active.gamma_multiply(focus);
                let color = if lyrics.synced {
                    blend(quiet, active_color, lit_eased)
                } else {
                    palette.text
                };
                let grown = if lyrics.synced {
                    1.0 + (motion.active_growth - 1.0) * lit_eased
                } else {
                    1.0
                };
                let base_size = size.round();
                let wrap = ui.available_width() / motion.active_growth.max(1.0);
                let top = ui.cursor().top();
                let center = top + base_size * 0.5;
                let edge = ((center - viewport.top()).min(viewport.bottom() - center)
                    / (base_size * 2.0))
                    .clamp(0.0, 1.0);
                let opacity = edge * edge * (3.0 - 2.0 * edge);
                let rise = if motion.rise && is_active {
                    (1.0 - lit_eased) * base_size * 0.14
                } else {
                    0.0
                };
                let look = motion.line(
                    if is_active {
                        theme::lyrics_sung(base_size)
                    } else {
                        theme::lyrics_quiet(base_size)
                    },
                    if is_active {
                        active_base
                    } else if line.at_ms.is_some_and(|at| at < position_ms) {
                        sung
                    } else {
                        quiet
                    },
                    active_color,
                    wrap,
                    rise,
                );
                let indent = if motion.duet_padding && line.opposite_aligned {
                    (wrap * 0.07).min(56.0)
                } else {
                    0.0
                };
                if line.text.is_empty()
                    && !line.background.is_empty()
                    && background_pop(ui, line, position_ms, is_active, &look, index) <= 0.01
                {
                    continue;
                }
                let draw_line = |ui: &mut egui::Ui| {
                    ui.multiply_opacity(opacity);
                    if line.text.is_empty() && line.background.is_empty() && lyrics.synced {
                        paint_interlude(
                            ui,
                            is_active,
                            color,
                            look.glow,
                            base_size,
                            wrap,
                            now.playing,
                            line.at_ms.unwrap_or(0),
                            line.end_ms.unwrap_or(0),
                            position_ms,
                        )
                    } else if word_line(line) {
                        paint_word_line(ui, line, position_ms, is_active, &look, index).0
                    } else {
                        // Untimed words have nowhere to jump to, but a click on
                        // them is still on a line, so it must not reach the
                        // backdrop.
                        paint_plain_line(
                            ui,
                            &line.text,
                            &look,
                            color,
                            is_active,
                            indent,
                            Sense::click(),
                        )
                    }
                };
                let response = if (grown - 1.0).abs() > 0.005 {
                    let origin = ui.cursor().min;
                    let pivot_x = match look.align {
                        LyricsAlign::Center => origin.x + wrap * 0.5,
                        LyricsAlign::Right => origin.x + wrap,
                        LyricsAlign::Left => origin.x,
                    };
                    let pivot = pos2(pivot_x, origin.y + base_size * 0.5);
                    let transform =
                        egui::emath::TSTransform::new(pivot.to_vec2() * (1.0 - grown), grown);
                    ui.with_visual_transform(transform, draw_line).inner
                } else {
                    ui.scope(draw_line).inner
                };
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                let rect = response.rect;
                if lyrics.synced
                    && response.clicked()
                    && let Some(at_ms) = line.at_ms
                {
                    app.actions.push(Action::Seek(at_ms));
                    app.actions.push(Action::FollowLyrics);
                }
                if is_active && following {
                    show_sung_line(ui, rect, active_at_fraction, Some(animation));
                }
                ui.add_space(line_gap);
            }
            if app.settings.lyrics_show_credits {
                attribution(ui, &lyrics, &palette, app.locale);
            }
            // Words without timing can only be followed by the clock: sit
            // at the part of the text the song is probably at.
            if following && !lyrics.synced && now.duration_ms > 0 {
                let fraction =
                    (f64::from(now.position_ms) / f64::from(now.duration_ms)).clamp(0.0, 1.0);
                let content = ui.min_rect();
                let y = content.top() + content.height() * fraction as f32;
                ui.scroll_to_rect_animation(
                    egui::Rect::from_min_max(
                        egui::pos2(content.left(), y),
                        egui::pos2(content.right(), y + 1.0),
                    ),
                    Some(Align::Center),
                    animation,
                );
            }
            ui.add_space(below.max(60.0));
        });
    // Scrolling by hand means the reader wants to look elsewhere; the
    // Follow button in the header picks the song back up.
    if manual_scroll && app.lyrics_following {
        app.actions.push(Action::PauseLyricsFollow);
    }
    if now.playing
        && lyrics.synced
        && let Some(next) = lyrics
            .lines
            .iter()
            .filter_map(|line| line.at_ms)
            .find(|at| *at > now.position_ms)
    {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(u64::from(
                next - now.position_ms,
            )));
    }
    request_word_repaint(ui.ctx(), &now, &lyrics, app.settings.lyrics_low_performance);
}

#[cfg(test)]
mod tests {
    use super::{
        align_offset, fullscreen_content_width, joined_text, joined_transliteration,
        preferred_backdrop_art, word_bounce, word_bounce_varied,
    };
    use crate::lyrics::Syllable;
    use crate::settings::LyricsAlign;

    #[test]
    fn a_word_springs_up_and_settles() {
        let size = 20.0;
        let (start, low) = word_bounce(1_000, 1_000, size, 1.0);
        assert_eq!((start, low), (1.0, 0.0), "a word starts at rest");
        let (peak, lift) = word_bounce(1_150, 1_000, size, 1.0);
        assert!(peak > 1.0, "a word swells while sounding, got {peak}");
        assert!(lift < 0.0, "the swell lifts, got {lift}");
        assert_eq!(word_bounce(5_000, 1_000, size, 1.0), (1.0, 0.0));
    }

    #[test]
    fn word_bounce_varied_introduces_subtle_variation_per_seed() {
        let (scale1, lift1) = word_bounce_varied(1_150, 1_000, 20.0, 1.0, 1);
        let (scale2, lift2) = word_bounce_varied(1_150, 1_000, 20.0, 1.0, 2);
        assert!(scale1 > 1.0);
        assert!(scale2 > 1.0);
        assert_ne!((scale1, lift1), (scale2, lift2));
        assert_eq!(word_bounce_varied(1_150, 1_000, 20.0, 0.0, 42), (1.0, 0.0));
    }

    #[test]
    fn the_bounce_strength_scales_the_swell_and_zero_rests() {
        let (gentle, gentle_lift) = word_bounce(1_150, 1_000, 20.0, 0.5);
        let (strong, strong_lift) = word_bounce(1_150, 1_000, 20.0, 1.5);
        assert!(gentle > 1.0);
        assert!(strong > gentle);
        assert!(strong_lift < gentle_lift);
        assert_eq!(word_bounce(1_150, 1_000, 20.0, 0.0), (1.0, 0.0));
    }

    #[test]
    fn alignment_centres_and_right_aligns_inside_the_column() {
        assert_eq!(align_offset(LyricsAlign::Left, 200.0, 80.0), 0.0);
        assert_eq!(align_offset(LyricsAlign::Center, 200.0, 80.0), 60.0);
        assert_eq!(align_offset(LyricsAlign::Right, 200.0, 80.0), 120.0);
        assert_eq!(align_offset(LyricsAlign::Right, 50.0, 80.0), 0.0);
    }

    fn syllable(text: &str, part_of_word: bool) -> Syllable {
        Syllable {
            text: text.into(),
            part_of_word,
            ..Syllable::default()
        }
    }

    #[test]
    fn a_word_keeps_together_however_many_syllables_it_has() {
        let syllables = [
            syllable("He", true),
            syllable("ll", true),
            syllable("o", false),
            syllable("there", false),
        ];
        assert_eq!(joined_text(&syllables), "Hello there");
    }

    #[test]
    fn a_transliteration_uses_its_own_word_markers() {
        let mut syllables = [
            syllable("こ", true),
            syllable("ん", true),
            syllable("に", false),
            syllable("ち", false),
            syllable("は", false),
        ];
        for (index, text) in ["ko", "n", "ni", "chi", "wa"].into_iter().enumerate() {
            syllables[index].transliterated = Some(text.into());
            syllables[index].transliterated_part_of_word = Some(index != 2);
        }
        // The text is one word, but this romanisation marks a word break
        // after "n", so the marker decides the space.
        assert_eq!(
            joined_transliteration(&syllables).as_deref(),
            Some("konni chiwa")
        );
    }

    #[test]
    fn fullscreen_backdrop_prefers_small_art_with_large_art_as_fallback() {
        let small = "small".to_string();
        let large = "large".to_string();
        assert_eq!(
            preferred_backdrop_art(Some(small.clone()), Some(large.clone())),
            Some(small)
        );
        assert_eq!(
            preferred_backdrop_art(None, Some(large.clone())),
            Some(large)
        );
        assert_eq!(preferred_backdrop_art(None, None), None);
    }

    #[test]
    fn fullscreen_content_width_never_inverts_a_narrow_viewport() {
        for viewport_width in [0.0, 24.0, 47.0, 48.0, 64.0, 760.0, 2_000.0] {
            let width = fullscreen_content_width(viewport_width);
            assert!(width >= 0.0);
            assert!(width <= (viewport_width - 48.0).max(0.0));
        }
        assert_eq!(fullscreen_content_width(47.0), 0.0);
        assert_eq!(fullscreen_content_width(2_000.0), 960.0);
    }

    /// A white picture ends up as dark as the dimmed art, and full white
    /// words on it above WCAG's 7:1.
    #[test]
    fn white_is_dimmed_until_the_words_read_on_it() {
        let dim = super::backdrop_dim(1.0);
        let linear = |value: f32| ((value + 0.055) / 1.055).powf(2.4);
        let contrast = 1.05 / (linear(dim) + 0.05);
        assert!(contrast >= 7.0, "{contrast}");
    }

    #[test]
    fn a_dark_picture_is_dimmed_less_but_never_left_alone() {
        assert!(super::backdrop_dim(0.1) > super::backdrop_dim(1.0));
        assert!(super::backdrop_dim(0.0) <= 0.32);
    }
}
