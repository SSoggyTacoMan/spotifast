//! Where every word of the lyrics sits, a port of Spicy Lyrics Mobile's
//! `LyricsLayoutCalculator` and `LineWrapper`. Lengths are points.

use std::sync::Arc;

use egui::{Galley, Vec2, vec2};

use super::timeline::{self, Kind, Role, TLine};

/// The lyric size and the column it is set in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// The lead lines' size.
    pub font: f32,
    /// The gap between lines, as a share of Spicy's own.
    pub spacing: f32,
    /// The whole view's width, which duet insets are shares of.
    pub width: f32,
    /// The text column's padding from the view's left and right.
    pub pad_left: f32,
    pub pad_right: f32,
    pub bloom: bool,
    pub wide_duet: bool,
}

impl Metrics {
    pub fn line_height(&self, font: f32) -> f32 {
        font * 1.181_818_2
    }

    pub fn line_gap(&self) -> f32 {
        self.font
            * if self.bloom {
                super::bloom::GAP_EM
            } else {
                super::bloom::SPRING_GAP_EM
            }
            * self.spacing
    }

    /// The slot a line is set in. With duets, a line steps in on the side
    /// it leans away from: to 15% of the width with duet padding, else the
    /// 5% every line has.
    fn slot(&self, has_duet: bool, rtl: bool, opposite: bool) -> (f32, f32) {
        let width = (self.width - self.pad_left - self.pad_right).max(1.0);
        if !has_duet {
            return (self.pad_left, width);
        }
        let extra = if self.wide_duet {
            self.width * 0.10
        } else {
            0.0
        };
        let at_inset = if rtl { !opposite } else { opposite };
        (
            self.pad_left + if at_inset { extra } else { 0.0 },
            (width - extra).max(1.0),
        )
    }
}

pub fn base_font(kind: Kind, width: f32) -> f32 {
    match kind {
        Kind::Static => (width * 0.05).clamp(12.8, 40.0),
        _ => (width * 0.07).clamp(29.6, 56.0),
    }
}

#[derive(Clone)]
pub struct WordLayout {
    pub galley: Arc<Galley>,
    /// From the line's start, top left of the piece.
    pub offset: Vec2,
    pub word: usize,
    /// The letter of a held word, when it is one.
    pub letter: Option<usize>,
    /// The whole word's width, across rows.
    pub full_width: f32,
    /// Where this piece starts inside the whole word.
    pub start_x: f32,
}

impl WordLayout {
    pub fn width(&self) -> f32 {
        self.galley.size().x
    }
    pub fn height(&self) -> f32 {
        self.galley.size().y
    }
}

#[derive(Clone)]
pub struct Note {
    pub galley: Arc<Galley>,
    pub offset: Vec2,
}

#[derive(Clone)]
pub struct LineLayout {
    pub words: Vec<WordLayout>,
    pub y: f32,
    pub height: f32,
    /// The words' own height, without the notes under them.
    pub lyric_height: f32,
    pub total_width: f32,
    pub max_row_width: f32,
    pub interlude: bool,
    pub background: bool,
    pub rtl: bool,
    pub right_aligned: bool,
    pub content_left: f32,
    pub content_width: f32,
    pub notes: Vec<Note>,
    /// Per source word, glued to the one before.
    pub glued: Vec<bool>,
    pub font: f32,
}

impl LineLayout {
    pub fn start_x(&self) -> f32 {
        if self.right_aligned {
            self.content_left + self.content_width - self.total_width
        } else {
            self.content_left
        }
    }
}

pub struct Fonts {
    pub bold: egui::FontId,
    pub semibold: egui::FontId,
    pub medium: egui::FontId,
}

fn fonts(size: f32) -> Fonts {
    Fonts {
        bold: crate::theme::lyrics_face(700, size),
        semibold: crate::theme::lyrics_face(600, size),
        medium: crate::theme::lyrics_face(500, size),
    }
}

fn measure(painter: &egui::Painter, text: &str, font: &egui::FontId) -> Arc<Galley> {
    let mut galley = painter.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE);
    crate::bidi::reorder(&mut galley);
    galley
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3040..=0x30FF | 0x3100..=0x31FF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
        | 0x20000..=0x323AF)
}

struct Piece {
    word: usize,
    letter: Option<usize>,
    galley: Arc<Galley>,
    text: String,
    full_width: f32,
    start_x: f32,
    glued: bool,
}

/// Where a word wider than a row may be cut: between its spaces (which
/// fall out), between CJK characters, and between letters of a part still
/// too wide.
fn wrap_fragments(text: &str, fits: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut prev: Option<char> = None;
    for (i, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push((s, i));
            }
        } else if let Some(s) = start {
            if is_cjk(c) || prev.is_some_and(is_cjk) {
                out.push((s, i));
                start = Some(i);
            }
        } else {
            start = Some(i);
        }
        prev = Some(c);
    }
    out.into_iter()
        .flat_map(|(s, e)| {
            if fits(&text[s..e]) {
                return vec![(s, e)];
            }
            let mut at = s;
            timeline::graphemes(&text[s..e])
                .into_iter()
                .map(|g| {
                    let range = (at, at + g.len());
                    at += g.len();
                    range
                })
                .collect()
        })
        .collect()
}

const NO_BREAK_BEFORE: &str =
    "、。，．・：；？！゛゜ヽヾゝゞ々）］｝」』】〕〉》〙〗〟’”｠»,.!?)]}…‥";
const NO_BREAK_AFTER: &str = "（［｛「『【〔〈《〘〖〝‘“｟«([{";

/// Row starts, then the piece count.
fn breaks(
    pieces: &[(f32, &str, bool)],
    max_width: f32,
    gap: f32,
    trailing: f32,
    text_mode: bool,
) -> Vec<usize> {
    let n = pieces.len();
    let mut rows = vec![0];
    if n == 0 {
        rows.push(0);
        return rows;
    }
    let mut run_fits = vec![false; n];
    let mut run_start = 0;
    let mut run_width = 0.0;
    for i in 0..n {
        if i > 0 && !pieces[i].2 {
            let fits = run_width + trailing <= max_width;
            run_fits[run_start..i].fill(fits);
            run_start = i;
            run_width = 0.0;
        }
        run_width += pieces[i].0;
    }
    run_fits[run_start..n].fill(run_width <= max_width);
    let ends_run = |i: usize| i == n - 1 || !pieces[i + 1].2;
    let punctuation = |i: usize| {
        let prev = pieces[i - 1].1.chars().last();
        let cur = pieces[i].1.chars().next();
        match (prev, cur) {
            (Some(p), Some(c)) => !NO_BREAK_BEFORE.contains(c) && !NO_BREAK_AFTER.contains(p),
            _ => true,
        }
    };
    let can_break = |i: usize| -> bool {
        if i == 0 {
            return true;
        }
        if text_mode {
            if !pieces[i].2 {
                return true;
            }
            if !punctuation(i) {
                return false;
            }
            let (Some(p), Some(c)) = (pieces[i - 1].1.chars().last(), pieces[i].1.chars().next())
            else {
                return true;
            };
            return is_cjk(p) || is_cjk(c) || !run_fits[i];
        }
        !pieces[i].2 || !run_fits[i] && punctuation(i)
    };
    let mut row_width = 0.0;
    let mut candidate = 0;
    let mut i = 0;
    while i < n {
        let (width, _, glued) = pieces[i];
        let space = if row_width > 0.0 && !glued { gap } else { 0.0 };
        if can_break(i) {
            candidate = i;
        }
        let tail = if ends_run(i) && i < n - 1 {
            trailing
        } else {
            0.0
        };
        if row_width + space + width + tail > max_width && row_width > 0.0 {
            let at = if candidate > *rows.last().expect("rows") {
                candidate
            } else {
                i
            };
            rows.push(at);
            i = at;
            row_width = 0.0;
        } else {
            row_width += space + width;
            i += 1;
        }
    }
    if rows.len() >= 2 {
        let last = *rows.last().expect("rows");
        let prev = rows[rows.len() - 2];
        if n - last == 1 && pieces[last].2 && last >= 1 {
            let moved = last - 1;
            if moved >= prev + 2 && can_break(moved) {
                let mut width = 0.0;
                for (k, piece) in pieces.iter().enumerate().skip(moved) {
                    width += piece.0 + if k > moved && !piece.2 { gap } else { 0.0 };
                }
                if width <= max_width {
                    let at = rows.len() - 1;
                    rows[at] = moved;
                }
            }
        }
    }
    rows.push(n);
    rows
}

fn right_aligned(has_duet: bool, rtl: bool, opposite: bool) -> bool {
    if !has_duet {
        rtl
    } else if rtl {
        !opposite
    } else {
        opposite
    }
}

pub fn layout(
    painter: &egui::Painter,
    lines: &[TLine],
    kind: Kind,
    metrics: &Metrics,
) -> Vec<LineLayout> {
    let mut layouts: Vec<LineLayout> = Vec::with_capacity(lines.len());
    let mut y = 0.0;
    let spacing = metrics.line_gap();
    let has_duet = lines.iter().any(|l| l.opposite);
    for (index, line) in lines.iter().enumerate() {
        let background = line.role == Role::Background;
        let size = if background {
            metrics.font * 0.75
        } else {
            metrics.font
        };
        let faces = fonts(size);
        let face = if kind == Kind::Static {
            &faces.medium
        } else if background {
            &faces.semibold
        } else {
            &faces.bold
        };
        if line.is_interlude() {
            let dot_font = fonts(metrics.font * 1.3).bold;
            let width = metrics.width;
            let gap = if metrics.bloom {
                (width * 0.0176).clamp(0.0067 * 16.0, 0.32 * 16.0)
            } else {
                (width * 0.017).clamp(0.005 * 16.0, 0.18 * 16.0)
            };
            let dot = measure(painter, "•", &dot_font);
            let dot_w = dot.size().x;
            let words = (0..3)
                .map(|i| WordLayout {
                    galley: dot.clone(),
                    offset: vec2(i as f32 * (dot_w + gap), 0.0),
                    word: i,
                    letter: None,
                    full_width: dot_w,
                    start_x: 0.0,
                })
                .collect();
            let total = 3.0 * dot_w + 2.0 * gap;
            let next = lines[index + 1..]
                .iter()
                .find(|l| l.start > line.start && l.role == Role::Lead);
            let next_rtl = next.is_some_and(|l| crate::bidi::is_rtl(&joined(l)));
            let next_opposite = next.is_some_and(|l| l.opposite);
            let (left, slot) = metrics.slot(has_duet, next_rtl, next_opposite);
            layouts.push(LineLayout {
                words,
                y,
                height: metrics.font * 1.3 * 0.65,
                lyric_height: metrics.font * 1.3 * 0.65,
                total_width: total,
                max_row_width: total,
                interlude: true,
                background: false,
                rtl: next_rtl,
                right_aligned: right_aligned(has_duet, next_rtl, next_opposite),
                content_left: left,
                content_width: slot,
                notes: Vec::new(),
                glued: Vec::new(),
                font: metrics.font,
            });
            continue;
        }
        let text = joined(line);
        let rtl = crate::bidi::is_rtl(&text);
        let (left, slot) = metrics.slot(has_duet, rtl, line.opposite);
        let line_kind = if line.line_timed && kind == Kind::Syllable {
            Kind::Line
        } else {
            kind
        };
        let max_width = if line_kind == Kind::Line {
            slot / super::animator::ACTIVE_LINE_SCALE
        } else {
            slot
        };
        let text_mode = line_kind != Kind::Syllable;
        let gap = if text_mode {
            measure(painter, "a a", face).size().x - measure(painter, "aa", face).size().x
        } else {
            measure(painter, "0", face).size().x * 0.32
        };
        let trailing = if !text_mode && !line.opposite && !rtl {
            gap
        } else {
            0.0
        };

        let mut pieces: Vec<Piece> = Vec::new();
        let mut glued = Vec::with_capacity(line.words.len());
        for (w, word) in line.words.iter().enumerate() {
            let full = measure(painter, &word.text, face);
            let full_width = full.size().x;
            glued.push(word.glued);
            if word.is_letter_group() {
                let mut x = 0.0;
                for (k, letter) in word.letters.iter().enumerate() {
                    let galley = measure(painter, &letter.text, face);
                    let width = galley.size().x;
                    pieces.push(Piece {
                        word: w,
                        letter: Some(k),
                        galley,
                        text: letter.text.clone(),
                        full_width,
                        start_x: x,
                        glued: k > 0 || word.glued,
                    });
                    x += width;
                }
            } else if full_width > max_width {
                for (s, e) in wrap_fragments(&word.text, |part| {
                    measure(painter, part, face).size().x <= max_width
                }) {
                    let part = &word.text[s..e];
                    let start_x = if s == 0 {
                        0.0
                    } else {
                        measure(painter, &word.text[..s], face).size().x
                    };
                    pieces.push(Piece {
                        word: w,
                        letter: None,
                        galley: measure(painter, part, face),
                        text: part.to_owned(),
                        full_width,
                        start_x,
                        glued: if s == 0 {
                            word.glued
                        } else {
                            !word.text[..s].ends_with(char::is_whitespace)
                        },
                    });
                }
            } else {
                pieces.push(Piece {
                    word: w,
                    letter: None,
                    galley: full,
                    text: word.text.clone(),
                    full_width,
                    start_x: 0.0,
                    glued: word.glued,
                });
            }
        }
        let rows = breaks(
            &pieces
                .iter()
                .map(|p| (p.galley.size().x, p.text.as_str(), p.glued))
                .collect::<Vec<_>>(),
            max_width,
            gap,
            trailing,
            text_mode,
        );
        let row_height = metrics.line_height(size);
        let mut placed: Vec<(f32, Vec<WordLayout>)> = Vec::new();
        let mut row_y = 0.0;
        let mut max_row = 0.0_f32;
        for r in 0..rows.len() - 1 {
            let mut row = Vec::new();
            let mut x = 0.0;
            for piece in &pieces[rows[r]..rows[r + 1]] {
                let space = if x > 0.0 && !piece.glued { gap } else { 0.0 };
                // Centre the glyph box in the row like CSS half-leading.
                let lead = (row_height - piece.galley.size().y) / 2.0;
                row.push(WordLayout {
                    galley: piece.galley.clone(),
                    offset: vec2(x + space, row_y + lead),
                    word: piece.word,
                    letter: piece.letter,
                    full_width: piece.full_width,
                    start_x: piece.start_x,
                });
                x += piece.galley.size().x + space;
            }
            max_row = max_row.max(x);
            placed.push((x, row));
            if r < rows.len() - 2 {
                row_y += row_height;
            }
        }
        let height = if pieces.is_empty() && !background {
            0.0
        } else {
            row_y + row_height
        };
        let aligned_right = right_aligned(has_duet, rtl, line.opposite);
        let mut words = Vec::new();
        for (width, row) in placed {
            let shift = if aligned_right { max_row - width } else { 0.0 };
            for mut piece in row {
                if rtl {
                    piece.offset.x = width - piece.offset.x - piece.galley.size().x;
                }
                piece.offset.x += shift;
                words.push(piece);
            }
        }
        let prev_interlude = layouts.last().is_some_and(|l| l.interlude);
        let draw_y = if background && !prev_interlude {
            y - spacing
        } else {
            y
        };
        let mut layout = LineLayout {
            words,
            y: draw_y,
            height,
            lyric_height: height,
            total_width: max_row,
            max_row_width: max_row,
            interlude: false,
            background,
            rtl,
            right_aligned: aligned_right,
            content_left: left,
            content_width: slot,
            notes: Vec::new(),
            glued,
            font: size,
        };
        let mut extra = 0.0;
        if !line.notes.is_empty() {
            let note_face = if background {
                fonts(size * 0.6).semibold
            } else {
                fonts(size * 0.6).bold
            };
            let mut ny = height;
            for note in &line.notes {
                ny += size * 0.06;
                let mut galley = painter.layout(
                    note.clone(),
                    note_face.clone(),
                    egui::Color32::WHITE,
                    (slot / super::animator::ACTIVE_LINE_SCALE).max(1.0),
                );
                crate::bidi::reorder(&mut galley);
                let x = if aligned_right {
                    max_row - galley.size().x
                } else {
                    0.0
                };
                let h = galley.size().y;
                layout.notes.push(Note {
                    galley,
                    offset: vec2(x, ny),
                });
                ny += h;
            }
            layout.height = ny;
            let next_background =
                !background && lines.get(index + 1).is_some_and(TLine::is_background);
            extra = size * if next_background { 0.0 } else { 0.18 };
        }
        let bottom = draw_y + layout.height;
        y = y.max(bottom + spacing) + extra;
        layouts.push(layout);
    }
    layouts
}

fn joined(line: &TLine) -> String {
    let mut text = String::new();
    for word in &line.words {
        if !text.is_empty() && !word.glued {
            text.push(' ');
        }
        text.push_str(&word.text);
    }
    text
}

/// What Bloom needs for one fill edge along a line.
pub fn sweep(layout: &LineLayout, line: &TLine) -> Option<super::animator::Sweep> {
    if layout.rtl || layout.interlude {
        return None;
    }
    let count = line.words.len();
    if count == 0 {
        return None;
    }
    let mut widths = vec![0.0_f32; count];
    let mut letters: Vec<Option<Vec<f32>>> = vec![None; count];
    let mut tops = vec![f32::NAN; count];
    let font = layout.font;
    for piece in &layout.words {
        let i = piece.word;
        if i >= count {
            continue;
        }
        if tops[i].is_nan() {
            tops[i] = piece.offset.y;
        }
        let width = piece.width();
        if let Some(k) = piece.letter {
            let list = letters[i].get_or_insert_with(|| vec![0.0; line.words[i].letters.len()]);
            if k < list.len() {
                list[k] = width;
            }
            widths[i] += width;
        } else {
            widths[i] = widths[i].max(piece.full_width).max(width);
        }
    }
    let mut gap = super::bloom::DEFAULT_GAP_EM * font;
    for pair in layout.words.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        if after.word == before.word || (after.offset.y - before.offset.y).abs() > font / 2.0 {
            continue;
        }
        let between = after.offset.x - (before.offset.x + before.width());
        if between > 0.0 {
            gap = between;
            break;
        }
    }
    let mut rows = vec![0; count];
    let mut row = 0;
    let mut last = f32::NAN;
    for i in 0..count {
        if !tops[i].is_nan() {
            if !last.is_nan() && tops[i] > last + font / 2.0 {
                row += 1;
            }
            last = tops[i];
        }
        rows[i] = row;
    }
    Some(super::animator::Sweep {
        widths,
        gap,
        letter_widths: letters,
        font_px: font,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lyric_size_follows_the_desktop_clamp() {
        assert_eq!(base_font(Kind::Syllable, 200.0), 29.6);
        assert_eq!(base_font(Kind::Syllable, 600.0), 42.0);
        assert_eq!(base_font(Kind::Syllable, 2_000.0), 56.0);
        assert_eq!(base_font(Kind::Static, 600.0), 30.0);
    }

    #[test]
    fn words_wrap_between_words_and_glued_runs_stay_together() {
        let pieces = [
            (30.0, "aa", false),
            (30.0, "bb", false),
            (20.0, "c", false),
            (20.0, "d", true),
        ];
        assert_eq!(breaks(&pieces, 70.0, 5.0, 0.0, false), vec![0, 2, 4]);
    }

    #[test]
    fn duet_lines_step_in_on_their_side() {
        let metrics = Metrics {
            font: 42.0,
            spacing: 1.0,
            width: 1000.0,
            pad_left: 50.0,
            pad_right: 50.0,
            bloom: false,
            wide_duet: true,
        };
        assert_eq!(metrics.slot(false, false, false), (50.0, 900.0));
        let (left, width) = metrics.slot(true, false, true);
        assert!((left - 150.0).abs() < 1e-3 && (width - 800.0).abs() < 1e-3);
        let (left, _) = metrics.slot(true, false, false);
        assert!((left - 50.0).abs() < 1e-3);
    }

    #[test]
    fn line_spacing_scales_only_the_gap() {
        let mut metrics = Metrics {
            font: 40.0,
            spacing: 1.0,
            width: 1000.0,
            pad_left: 0.0,
            pad_right: 0.0,
            bloom: false,
            wide_duet: false,
        };
        let gap = metrics.line_gap();
        let height = metrics.line_height(metrics.font);
        metrics.spacing = 1.5;
        assert!((metrics.line_gap() - gap * 1.5).abs() < 1e-4);
        assert_eq!(metrics.line_height(metrics.font), height);
    }
}
