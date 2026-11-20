//! The lyrics as Spicy Lyrics Mobile draws them: lead lines, the background
//! vocals sung under them as lines of their own, and the interludes between,
//! with each held word split into timed letters.

use crate::lyrics::{Line, Lyrics};

/// What kind of lyrics these are; each draws differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Word by word.
    Syllable,
    /// A time per line.
    Line,
    /// No timing at all.
    Static,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Lead,
    Background,
    Interlude,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Letter {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// No space between this word and the one before.
    pub glued: bool,
    /// Split into letters for the held-word emphasis.
    pub letters: Vec<Letter>,
}

impl Word {
    pub fn is_letter_group(&self) -> bool {
        !self.letters.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TLine {
    pub words: Vec<Word>,
    pub start: f64,
    pub end: f64,
    pub role: Role,
    /// The lead line and its background vocals share one.
    pub group: usize,
    pub opposite: bool,
    /// A line-timed line in word-synced lyrics.
    pub line_timed: bool,
    /// The plain text of the line, for line-synced and static lyrics.
    pub text: String,
    /// Romanisation, translation, shown under the line.
    pub notes: Vec<String>,
}

impl TLine {
    pub fn is_background(&self) -> bool {
        self.role == Role::Background
    }
    pub fn is_interlude(&self) -> bool {
        self.role == Role::Interlude
    }
    pub fn duration(&self) -> f64 {
        self.end - self.start
    }
}

pub const NORMAL_INTERLUDE_MS: f64 = 3_000.0;
pub const MINIMAL_INTERLUDE_MS: f64 = 5_000.0;
/// Interlude dots collapse this long before their line ends.
pub const PRE_HIDDEN_DOT_LINE_MS: f64 = 500.0;
/// Spring splits a word held this long into letters.
const LETTER_THRESHOLD_MS: f64 = 1_000.0;

/// How a word sung in syllables is filled.
pub use crate::settings::SyllableMerge;

#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    pub bloom: bool,
    pub minimal: bool,
    pub merge: SyllableMerge,
    pub pace: bool,
    pub notes: bool,
}

pub struct Timeline {
    pub kind: Kind,
    pub lines: Vec<TLine>,
}

impl Timeline {
    pub fn build(lyrics: &Lyrics, options: BuildOptions) -> Self {
        let kind = if !lyrics.synced {
            Kind::Static
        } else if lyrics.word_synced() {
            Kind::Syllable
        } else {
            Kind::Line
        };
        let mut lines = Vec::new();
        for (group, line) in lyrics.lines.iter().enumerate() {
            let next_start = lyrics.lines[group + 1..]
                .iter()
                .find_map(|line| line.at_ms)
                .map(f64::from);
            lines.extend(convert(line, group, kind, next_start, options));
        }
        if kind != Kind::Static {
            lines = display_timeline(
                lines,
                options.minimal,
                options.bloom || (options.minimal && kind == Kind::Syllable),
            );
        }
        if kind == Kind::Syllable {
            for line in &mut lines {
                if line.is_interlude() || line.line_timed {
                    continue;
                }
                line.words = merge_syllables(std::mem::take(&mut line.words), options.merge);
                let background = line.is_background();
                line.words = if options.bloom {
                    bloom_words(std::mem::take(&mut line.words), background)
                } else {
                    std::mem::take(&mut line.words)
                        .into_iter()
                        .map(|word| synthesize(word, false, background))
                        .collect()
                };
            }
        }
        Self { kind, lines }
    }
}

fn convert(
    line: &Line,
    group: usize,
    kind: Kind,
    next_start: Option<f64>,
    options: BuildOptions,
) -> Vec<TLine> {
    let start = line.at_ms.map_or(0.0, f64::from);
    let mut notes = Vec::new();
    if options.notes {
        let transliterated = line
            .transliterated
            .clone()
            .or_else(|| super::joined_transliteration(&line.syllables));
        notes.extend(transliterated.filter(|text| !text.trim().is_empty()));
        notes.extend(
            line.translated
                .clone()
                .filter(|text| !text.trim().is_empty()),
        );
    }
    if kind != Kind::Syllable || line.syllables.is_empty() && line.background.is_empty() {
        // LRC end: the next distinct timestamp, else start + 4 s.
        let end = line
            .end_ms
            .map(f64::from)
            .filter(|end| *end > start)
            .or(next_start.filter(|next| *next > start))
            .unwrap_or(start + 4_000.0);
        if line.text.trim().is_empty() {
            return Vec::new();
        }
        let line_timed = kind == Kind::Syllable;
        return vec![TLine {
            words: vec![Word {
                text: line.text.clone(),
                start,
                end,
                glued: false,
                letters: Vec::new(),
            }],
            start,
            end,
            role: Role::Lead,
            group,
            opposite: line.opposite_aligned,
            line_timed,
            text: line.text.clone(),
            notes,
        }];
    }
    let words = words_of(&line.syllables);
    let background = words_of(&line.background);
    let words_end = words.iter().map(|w| w.end).fold(f64::MIN, f64::max);
    let mut lead_start = words.iter().map(|w| w.start).fold(start, f64::min);
    let mut lead_end = line
        .end_ms
        .map(f64::from)
        .unwrap_or(words_end)
        .max(words_end);
    let bg_start = background.iter().map(|w| w.start).fold(f64::MAX, f64::min);
    let bg_end = background.iter().map(|w| w.end).fold(f64::MIN, f64::max);
    if !background.is_empty() {
        lead_start = lead_start.min(bg_start);
        lead_end = lead_end.max(bg_end);
    }
    if words.is_empty() {
        lead_end = lead_end.max(lead_start);
    }
    let mut out = vec![TLine {
        text: line.text.clone(),
        words,
        start: lead_start,
        end: lead_end.max(lead_start),
        role: Role::Lead,
        group,
        opposite: line.opposite_aligned,
        line_timed: false,
        notes,
    }];
    if !background.is_empty() {
        out.push(TLine {
            text: String::new(),
            words: background,
            start: bg_start,
            end: bg_end.max(bg_start),
            role: Role::Background,
            group,
            opposite: line.opposite_aligned,
            line_timed: false,
            notes: Vec::new(),
        });
    }
    out
}

/// Syllables as words: `part_of_word` on a syllable glues the next one to it.
fn words_of(syllables: &[crate::lyrics::Syllable]) -> Vec<Word> {
    let mut words = Vec::with_capacity(syllables.len());
    let mut glue_next = false;
    for syllable in syllables {
        let text = syllable.text.trim().to_string();
        if text.is_empty() {
            glue_next = syllable.part_of_word && glue_next;
            continue;
        }
        let start = f64::from(syllable.at_ms);
        let end = f64::from(syllable.end_ms.max(syllable.at_ms));
        words.push(Word {
            text,
            start,
            end,
            glued: glue_next,
            letters: Vec::new(),
        });
        glue_next = syllable.part_of_word;
    }
    words
}

/// Interludes in the gaps, and short gaps held through when asked.
fn display_timeline(lines: Vec<TLine>, minimal: bool, hold: bool) -> Vec<TLine> {
    let threshold = if minimal {
        MINIMAL_INTERLUDE_MS
    } else {
        NORMAL_INTERLUDE_MS
    };
    if !lines.iter().any(|l| l.role == Role::Lead) {
        return lines;
    }
    let leads: Vec<(f64, f64)> = lines
        .iter()
        .filter(|l| l.role == Role::Lead)
        .map(|l| (l.start, l.end))
        .collect();
    let mut timeline = Vec::with_capacity(lines.len() + 8);
    let mut previous: Option<f64> = None;
    let mut lead_index = 0;
    for line in lines {
        let mut shown = line;
        if shown.role == Role::Lead {
            let gap_start = previous.unwrap_or(0.0);
            let gap_end = if previous.is_none() {
                shown
                    .words
                    .iter()
                    .map(|w| w.start)
                    .fold(f64::MAX, f64::min)
                    .min(f64::MAX)
                    .min(if shown.words.is_empty() {
                        shown.start
                    } else {
                        f64::MAX
                    })
            } else {
                shown.start
            };
            if gap_end - gap_start >= threshold {
                timeline.push(TLine {
                    words: Vec::new(),
                    start: gap_start,
                    end: gap_end,
                    role: Role::Interlude,
                    group: usize::MAX,
                    opposite: false,
                    line_timed: false,
                    text: String::new(),
                    notes: Vec::new(),
                });
            }
            previous = Some(shown.end);
            lead_index += 1;
            if hold
                && let Some(&(next_start, _)) = leads.get(lead_index)
                && next_start > shown.end
                && next_start - shown.end < threshold
            {
                shown.end = next_start;
            }
        }
        timeline.push(shown);
    }
    timeline
}

const WORD_JOINERS: [char; 8] = ['/', '-', '‐', '‑', '‒', '–', '—', '―'];
const DASHES: [char; 7] = ['-', '‐', '‑', '‒', '–', '—', '―'];

fn is_unspaced(c: char) -> bool {
    let c = c as u32;
    [
        0x0E00..=0x0EFF,
        0x1000..=0x109F,
        0x1780..=0x17FF,
        0x2E80..=0x2FDF,
        0x3005..=0x3007,
        0x3021..=0x3029,
        0x3038..=0x303B,
        0x3040..=0x30FF,
        0x31F0..=0x31FF,
        0x3400..=0x4DBF,
        0x4E00..=0x9FFF,
        0xF900..=0xFAFF,
        0xFF66..=0xFF9F,
        0x20000..=0x323AF,
    ]
    .iter()
    .any(|range| range.contains(&c))
}

fn starts_new_word(prev: &Word, next: &Word) -> bool {
    let (Some(last), Some(first)) = (
        prev.text.trim_end().chars().last(),
        next.text.trim_start().chars().next(),
    ) else {
        return true;
    };
    WORD_JOINERS.contains(&last)
        || WORD_JOINERS.contains(&first)
        || is_unspaced(last)
        || is_unspaced(first)
}

/// Words in runs: a word, then the syllables glued to it.
fn runs(words: Vec<Word>) -> Vec<Vec<Word>> {
    let mut runs: Vec<Vec<Word>> = Vec::new();
    for word in words {
        if word.glued
            && let Some(run) = runs.last_mut()
            && !starts_new_word(run.last().expect("a run has a word"), &word)
        {
            run.push(word);
        } else {
            runs.push(vec![word]);
        }
    }
    runs
}

/// For each word, the index of the first word of its run.
pub fn run_starts(words: &[Word]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(words.len());
    let mut first = 0;
    for (i, word) in words.iter().enumerate() {
        if i == 0 || !word.glued || starts_new_word(&words[i - 1], word) {
            first = i;
        }
        starts.push(first);
    }
    starts
}

fn joins_evenly(run: &[Word]) -> bool {
    const MAX_GAP: f64 = 100.0;
    const MAX_PACE: f64 = 3.0;
    let last = run.len() - 1;
    if run.iter().enumerate().any(|(i, part)| {
        let text = if i == last {
            part.text.trim_end()
        } else {
            &part.text
        };
        text.chars().any(char::is_whitespace)
    }) {
        return false;
    }
    if run[..last].iter().any(|part| {
        part.text
            .chars()
            .last()
            .is_some_and(|c| DASHES.contains(&c))
    }) {
        return false;
    }
    if run
        .windows(2)
        .any(|pair| pair[1].start - pair[0].end > MAX_GAP)
    {
        return false;
    }
    let total = run.iter().map(|w| w.end).fold(f64::MIN, f64::max) - run[0].start;
    let chars: usize = run.iter().map(|w| w.text.trim_end().chars().count()).sum();
    if total <= 0.0 || chars == 0 {
        return false;
    }
    run.iter().all(|part| {
        let time = (part.end - part.start) / total;
        let share = part.text.trim_end().chars().count() as f64 / chars as f64;
        time >= share / MAX_PACE && time <= share * MAX_PACE
    })
}

fn merge_syllables(words: Vec<Word>, merge: SyllableMerge) -> Vec<Word> {
    if merge == SyllableMerge::Off {
        return words;
    }
    runs(words)
        .into_iter()
        .flat_map(|run| {
            if run.len() == 1 || merge == SyllableMerge::Steady && !joins_evenly(&run) {
                return run;
            }
            let merged = Word {
                text: run.iter().map(|w| w.text.as_str()).collect(),
                start: run[0].start,
                end: run.iter().map(|w| w.end).fold(f64::MIN, f64::max),
                glued: run[0].glued,
                letters: Vec::new(),
            };
            if merge == SyllableMerge::Held && !letter_capable(&merged, false, false) {
                run
            } else {
                vec![merged]
            }
        })
        .collect()
}

pub fn graphemes(text: &str) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation as _;
    let mut out: Vec<String> = Vec::new();
    for grapheme in text.graphemes(true) {
        // A virama or joiner ends a cluster that belongs with the next.
        if let Some(last) = out.last_mut()
            && last.ends_with(['\u{200D}', '\u{094D}', '\u{09CD}', '\u{0BCD}', '\u{0CCD}'])
        {
            last.push_str(grapheme);
        } else {
            out.push(grapheme.to_string());
        }
    }
    out
}

fn letter_capable(word: &Word, bloom: bool, background: bool) -> bool {
    let count = graphemes(&word.text).len();
    let (min, max) = if bloom { (2, 7) } else { (1, usize::MAX) };
    !(bloom && background)
        && word.end - word.start >= LETTER_THRESHOLD_MS
        && (min..=max).contains(&count)
        && !crate::bidi::is_rtl(&word.text)
}

fn synthesize(word: Word, bloom: bool, background: bool) -> Word {
    if !letter_capable(&word, bloom, background) {
        return Word {
            letters: Vec::new(),
            ..word
        };
    }
    split(word, bloom)
}

/// The held word's letters, over its emphasis window: Spring trims 250 ms
/// off its tail; Bloom starts 21 ms and ends 40 ms later.
fn split(word: Word, bloom: bool) -> Word {
    let parts = graphemes(&word.text);
    let (shift_start, shift_end) = if bloom { (21.0, 40.0) } else { (0.0, -250.0) };
    let from = word.start + shift_start;
    let to = (word.end + shift_end).max(from + 1.0);
    let span = (to - from) / parts.len() as f64;
    let letters = parts
        .into_iter()
        .enumerate()
        .map(|(i, text)| Letter {
            text,
            start: from + (i as f64 * span).floor(),
            end: from + ((i + 1) as f64 * span).floor(),
        })
        .collect();
    Word {
        start: from,
        end: to,
        letters,
        ..word
    }
}

/// Bloom judges a word sung in syllables as a whole.
fn bloom_words(words: Vec<Word>, background: bool) -> Vec<Word> {
    runs(words)
        .into_iter()
        .flat_map(|run| {
            if run.len() == 1 {
                return vec![synthesize(
                    run.into_iter().next().expect("one"),
                    true,
                    background,
                )];
            }
            let whole = Word {
                text: run.iter().map(|w| w.text.as_str()).collect(),
                start: run[0].start,
                end: run.iter().map(|w| w.end).fold(f64::MIN, f64::max),
                glued: run[0].glued,
                letters: Vec::new(),
            };
            let splits = letter_capable(&whole, true, background)
                && !run.iter().any(|w| crate::bidi::is_rtl(&w.text));
            run.into_iter()
                .map(|part| if splits { split(part, true) } else { part })
                .collect()
        })
        .collect()
}

#[cfg(test)]
/// The three interlude dots' (start, end): each a third of the gap, shifted
/// so the last ends 550 ms early.
pub fn interlude_dot_times(start: f64, end: f64) -> [(f64, f64); 3] {
    let total = end - start;
    let base = total / 3.0;
    let padding = -550.0 / 3.0;
    let d1 = start.max(start + base + padding).round();
    let d2 = d1.max(start + base * 2.0 + padding * 2.0).round();
    let d3 = d2.max(start + total - 550.0).round();
    [(start, d1), (d1, d2), (d2, d3)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Syllable;

    fn syl(text: &str, at: u32, end: u32, part: bool) -> Syllable {
        Syllable {
            text: text.into(),
            at_ms: at,
            end_ms: end,
            part_of_word: part,
            ..Syllable::default()
        }
    }

    fn options() -> BuildOptions {
        BuildOptions {
            bloom: false,
            minimal: false,
            merge: SyllableMerge::Off,
            pace: false,
            notes: true,
        }
    }

    fn lyrics(lines: Vec<Line>) -> Lyrics {
        Lyrics {
            lines,
            synced: true,
            ..Lyrics::default()
        }
    }

    #[test]
    fn background_vocals_become_their_own_line_under_the_lead() {
        let line = Line {
            at_ms: Some(5_000),
            end_ms: Some(6_000),
            syllables: vec![syl("Hey", 5_000, 5_900, false)],
            background: vec![syl("(oh)", 5_500, 6_500, false)],
            ..Line::default()
        };
        let timeline = Timeline::build(&lyrics(vec![line]), options());
        let roles: Vec<Role> = timeline.lines.iter().map(|l| l.role).collect();
        assert_eq!(roles, [Role::Interlude, Role::Lead, Role::Background]);
        assert_eq!(
            timeline.lines[1].end, 6_500.0,
            "the lead covers its background"
        );
        assert_eq!(timeline.lines[1].words[0].end, 5_900.0);
    }

    #[test]
    fn a_held_word_splits_into_letters_over_its_trimmed_window() {
        let line = Line {
            at_ms: Some(0),
            syllables: vec![syl("Oh", 0, 1_250, false)],
            ..Line::default()
        };
        let timeline = Timeline::build(&lyrics(vec![line]), options());
        let word = &timeline.lines[0].words[0];
        assert_eq!(word.letters.len(), 2);
        assert_eq!(word.end, 1_000.0);
        assert_eq!(word.letters[1].start, 500.0);
    }

    #[test]
    fn glued_syllables_stay_glued_and_merge_when_asked() {
        let line = Line {
            at_ms: Some(0),
            syllables: vec![
                syl("Hel", 0, 300, true),
                syl("lo", 300, 600, false),
                syl("you", 600, 900, false),
            ],
            ..Line::default()
        };
        let plain = Timeline::build(&lyrics(vec![line.clone()]), options());
        let glued: Vec<bool> = plain.lines[0].words.iter().map(|w| w.glued).collect();
        assert_eq!(glued, [false, true, false]);
        let merged = Timeline::build(
            &lyrics(vec![line]),
            BuildOptions {
                merge: SyllableMerge::Full,
                ..options()
            },
        );
        assert_eq!(merged.lines[0].words[0].text, "Hello");
        assert_eq!(merged.lines[0].words.len(), 2);
    }

    #[test]
    fn dots_end_early() {
        let [a, b, c] = interlude_dot_times(0.0, 6_000.0);
        assert_eq!(a, (0.0, 1817.0));
        assert_eq!(b.1, 3633.0);
        assert_eq!(c.1, 5450.0);
    }
}
