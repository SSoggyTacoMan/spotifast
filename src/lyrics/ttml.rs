//! Word-synced lyrics in Apple Music's TTML shape.
//!
//! AMLL, Unison, lrc.red and NetEase all end up here: the first three serve
//! TTML directly, and NetEase's YRC is converted to TTML before parsing. The
//! parser keeps the reference implementation's rules: names are matched as
//! written (prefix included), an attribute-less `<span>` is not a syllable,
//! and a syllable joins the next only when their tags touch.
//!
//! Background vocals (`ttm:role="x-bg"`) become the line's `background`
//! syllables, which the lyrics view already pops out under the lead.

use std::collections::HashMap;

use super::{Line, Lyrics, Syllable};

/// The document had no lyrics this parser can show.
pub fn parse(text: &str) -> Option<Lyrics> {
    let root = parse_document(text)?;
    parse_root(&root)
}

// ---- a minimal XML tree ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn elements(&self, name: &str) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(move |node| match node {
            Node::Element(element) if element.name == name => Some(element),
            _ => None,
        })
    }

    fn element(&self, name: &str) -> Option<&Element> {
        self.elements(name).next()
    }

    /// The element's own text, not its children's.
    fn own_text(&self) -> String {
        self.children
            .iter()
            .filter_map(|node| match node {
                Node::Text(text) => Some(text.as_str()),
                Node::Element(_) => None,
            })
            .collect()
    }

    /// An element without attributes or child elements is plain text to the
    /// web parsers these documents were written for.
    fn is_plain(&self) -> bool {
        self.attrs.is_empty()
            && !self
                .children
                .iter()
                .any(|node| matches!(node, Node::Element(_)))
    }

    fn has_text_child(&self) -> bool {
        self.children
            .iter()
            .any(|node| matches!(node, Node::Text(_)))
    }
}

fn parse_document(text: &str) -> Option<Element> {
    let chars: Vec<char> = text
        .trim_start_matches(|c: char| c == '\u{feff}' || c.is_whitespace())
        .chars()
        .collect();
    let mut index = 0;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    while index < chars.len() {
        if chars[index] != '<' {
            let start = index;
            while index < chars.len() && chars[index] != '<' {
                index += 1;
            }
            let raw: String = chars[start..index].iter().collect();
            if let Some(parent) = stack.last_mut() {
                parent.children.push(Node::Text(decode_entities(&raw)));
            }
            continue;
        }
        if starts_with(&chars, index, "<?") {
            index = skip_past(&chars, index + 2, "?>")?;
        } else if starts_with(&chars, index, "<!--") {
            index = skip_past(&chars, index + 4, "-->")?;
        } else if starts_with(&chars, index, "<![CDATA[") {
            let start = index + 9;
            let end = find(&chars, start, "]]>")?;
            let raw: String = chars[start..end].iter().collect();
            if let Some(parent) = stack.last_mut() {
                parent.children.push(Node::Text(raw));
            }
            index = end + 3;
        } else if starts_with(&chars, index, "<!") {
            index = skip_declaration(&chars, index)?;
        } else if starts_with(&chars, index, "</") {
            let end = find(&chars, index + 2, ">")?;
            let name: String = chars[index + 2..end]
                .iter()
                .take_while(|c| !c.is_whitespace())
                .collect();
            let closed = stack.pop()?;
            if closed.name != name.trim() {
                return None;
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(Node::Element(closed)),
                None => root = Some(closed),
            }
            index = end + 1;
        } else {
            let (element, self_closing, next) = parse_open_tag(&chars, index)?;
            if self_closing {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(element)),
                    None => {
                        if root.is_some() {
                            return None;
                        }
                        root = Some(element);
                    }
                }
            } else {
                stack.push(element);
            }
            index = next;
        }
    }

    if !stack.is_empty() {
        return None;
    }
    root
}

fn starts_with(chars: &[char], at: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(at + offset) == Some(&expected))
}

fn find(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        if starts_with(chars, index, needle) {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn skip_past(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    find(chars, from, needle).map(|at| at + needle.chars().count())
}

/// A doctype or other `<!...>` declaration, which may carry an internal
/// subset with `>` of its own.
fn skip_declaration(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = from + 2;
    while index < chars.len() {
        match chars[index] {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '>' if depth == 0 => return Some(index + 1),
            _ => {}
        }
        index += 1;
    }
    None
}

fn parse_open_tag(chars: &[char], from: usize) -> Option<(Element, bool, usize)> {
    let mut index = from + 1;
    let name_start = index;
    while index < chars.len()
        && !chars[index].is_whitespace()
        && chars[index] != '>'
        && chars[index] != '/'
    {
        index += 1;
    }
    let name: String = chars[name_start..index].iter().collect();
    if name.is_empty() {
        return None;
    }
    let mut attrs = Vec::new();
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        match chars.get(index)? {
            '>' => {
                return Some((
                    Element {
                        name,
                        attrs,
                        children: Vec::new(),
                    },
                    false,
                    index + 1,
                ));
            }
            '/' if chars.get(index + 1) == Some(&'>') => {
                return Some((
                    Element {
                        name,
                        attrs,
                        children: Vec::new(),
                    },
                    true,
                    index + 2,
                ));
            }
            '/' => index += 1,
            _ => {
                let key_start = index;
                while index < chars.len()
                    && !chars[index].is_whitespace()
                    && chars[index] != '='
                    && chars[index] != '>'
                {
                    index += 1;
                }
                let key: String = chars[key_start..index].iter().collect();
                while index < chars.len() && chars[index].is_whitespace() {
                    index += 1;
                }
                if chars.get(index) != Some(&'=') {
                    continue;
                }
                index += 1;
                while index < chars.len() && chars[index].is_whitespace() {
                    index += 1;
                }
                let quote = *chars.get(index)?;
                if quote != '"' && quote != '\'' {
                    return None;
                }
                index += 1;
                let value_start = index;
                while index < chars.len() && chars[index] != quote {
                    index += 1;
                }
                let raw: String = chars[value_start..index].iter().collect();
                attrs.push((key, decode_entities(&raw)));
                index += 1;
            }
        }
    }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(end) = after.find(';') else {
            out.push('&');
            rest = after;
            continue;
        };
        let name = &after[..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ if name.starts_with("#x") || name.starts_with("#X") => {
                u32::from_str_radix(&name[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- the TTML itself ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LyricsType {
    Static,
    Line,
    Syllable,
}

struct TimedSpan {
    begin: String,
    end: String,
    begin_s: Option<f64>,
    end_s: Option<f64>,
    text: String,
}

#[derive(Default)]
struct TransliterationLine {
    spans: Vec<TimedSpan>,
    background: Vec<TimedSpan>,
    text: String,
}

struct SyllableDraft {
    text: String,
    roman: Option<String>,
    part_of_word: bool,
    start: Option<f64>,
    end: Option<f64>,
}

struct RoleTexts {
    roman: Option<String>,
    translation: Option<String>,
}

struct TranslatedLine {
    lead: String,
    backgrounds: Vec<String>,
}

struct Translations {
    by_key: HashMap<String, TranslatedLine>,
}

struct TranslationLookup {
    head: Option<Translations>,
}

fn parse_root(root: &Element) -> Option<Lyrics> {
    if root.name != "tt" {
        return None;
    }
    let body = root.element("body")?;
    let divs = divs(body);
    if divs.is_empty() {
        return None;
    }

    let kind = match root.attr("itunes:timing") {
        None => infer_type(&divs),
        Some(timing) => match timing.to_lowercase().as_str() {
            "none" | "static" => LyricsType::Static,
            "word" | "syllable" => LyricsType::Syllable,
            "line" => LyricsType::Line,
            _ => return None,
        },
    };

    let metadata = root
        .element("head")
        .and_then(|head| head.element("metadata"));
    let songwriters = songwriters(metadata);
    let transliterations = transliterations(metadata);
    let mut translated = TranslationLookup {
        head: translations(metadata),
    };
    let opposite: Vec<String> = metadata
        .map(|metadata| {
            metadata
                .elements("ttm:agent")
                .filter(|agent| !agent.is_plain())
                .filter_map(|agent| agent.attr("xml:id"))
                .filter(|id| *id == "v2" || *id == "v2000")
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    let timed = match kind {
        LyricsType::Static => static_lines(&divs, &mut translated),
        LyricsType::Line => line_lines(&divs, &transliterations, &mut translated),
        LyricsType::Syllable => {
            syllable_lines(&divs, body, &transliterations, &mut translated, &opposite)
        }
    };
    let lines = place_untimed(timed);
    if lines.is_empty() {
        return None;
    }
    let synced = lines.iter().all(|line| line.at_ms.is_some());
    Some(Lyrics {
        lines,
        synced,
        instrumental: false,
        attribution: None,
        song_writers: songwriters,
    })
}

fn divs(body: &Element) -> Vec<&Element> {
    let found: Vec<&Element> = body.elements("div").filter(|div| !div.is_plain()).collect();
    if !found.is_empty() {
        return found;
    }
    body.elements("p").collect()
}

fn paragraphs(div: &Element) -> Vec<&Element> {
    div.elements("p").collect()
}

fn infer_type(divs: &[&Element]) -> LyricsType {
    let mut saw_timing = false;
    for p in divs.iter().flat_map(|div| paragraphs(div)) {
        if p.is_plain() {
            continue;
        }
        let timed_vocal_spans = p
            .elements("span")
            .filter(|span| is_vocal_span(span))
            .filter(|span| span.attr("begin").is_some_and(|begin| !begin.is_empty()))
            .count();
        if timed_vocal_spans > 1 {
            return LyricsType::Syllable;
        }
        if timed_vocal_spans == 1 {
            saw_timing = true;
        }
        if p.attr("begin").is_some_and(|begin| !begin.is_empty()) {
            saw_timing = true;
        }
    }
    if saw_timing {
        LyricsType::Line
    } else {
        LyricsType::Static
    }
}

fn is_span_object(node: &Element) -> bool {
    node.name == "span" && !node.is_plain()
}

fn is_vocal_span(node: &Element) -> bool {
    is_span_object(node) && node.attr("ttm:role").is_none()
}

fn is_role(node: &Element, role: &str) -> bool {
    is_span_object(node) && node.attr("ttm:role") == Some(role)
}

fn strip_zero_width(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{2060}' | '\u{feff}'
            )
        })
        .collect()
}

fn has_lyrics_text(text: &str) -> bool {
    !strip_zero_width(text).trim().is_empty()
}

fn collapse(text: &str) -> String {
    strip_zero_width(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn static_lines(divs: &[&Element], translated: &mut TranslationLookup) -> Vec<(Line, bool)> {
    divs.iter()
        .filter(|div| {
            !matches!(
                div.attr("itunes:songPart"),
                Some("Instrumental") | Some("Outro")
            )
        })
        .flat_map(|div| paragraphs(div))
        .filter_map(|p| {
            let text = line_text(p).unwrap_or_default();
            let roman = inline_role_texts(p)
                .roman
                .map(|roman| roman.trim().to_string())
                .filter(|roman| !roman.is_empty());
            if !has_lyrics_text(&text)
                && roman.as_deref().is_none_or(|roman| !has_lyrics_text(roman))
            {
                return None;
            }
            Some((
                Line {
                    transliterated: roman,
                    translated: translated.lead(p),
                    ..Line::new(None, text.trim())
                },
                false,
            ))
        })
        .collect()
}

fn line_lines(
    divs: &[&Element],
    transliterations: &HashMap<String, TransliterationLine>,
    translated: &mut TranslationLookup,
) -> Vec<(Line, bool)> {
    divs.iter()
        .filter(|div| div.attr("itunes:songPart") != Some("Instrumental"))
        .flat_map(|div| paragraphs(div))
        .filter_map(|p| {
            let vocal_spans: Vec<&Element> = p
                .elements("span")
                .filter(|span| is_vocal_span(span))
                .collect();
            let start = convert_time_to_seconds(
                p.attr("begin")
                    .or_else(|| vocal_spans.first().and_then(|span| span.attr("begin"))),
            );
            let end = convert_time_to_seconds(
                p.attr("end")
                    .or_else(|| vocal_spans.last().and_then(|span| span.attr("end"))),
            );
            let text = line_text(p);
            let entry = p
                .attr("itunes:key")
                .and_then(|key| transliterations.get(key));
            let roman = entry
                .filter(|entry| !entry.text.trim().is_empty())
                .map(|entry| entry.text.clone())
                .or_else(|| {
                    entry
                        .filter(|entry| !entry.spans.is_empty())
                        .map(|entry| entry.spans.iter().map(|span| span.text.as_str()).collect())
                })
                .or_else(|| inline_role_texts(p).roman)
                .map(|roman| roman.trim().to_string())
                .filter(|roman| !roman.is_empty());
            let text = text.unwrap_or_default();
            if !has_lyrics_text(&text)
                && roman.as_deref().is_none_or(|roman| !has_lyrics_text(roman))
            {
                return None;
            }
            let start_ms = start.map(ms);
            let end_ms = end.map(ms).unwrap_or(start_ms.unwrap_or(0));
            let opposite = p
                .attr("spicy:oppositeAligned")
                .and_then(|value| value.parse::<bool>().ok())
                .unwrap_or(false);
            Some((
                Line {
                    at_ms: start_ms,
                    end_ms: Some(end_ms.max(start_ms.unwrap_or(0))),
                    text: text.trim().to_string(),
                    transliterated: roman,
                    opposite_aligned: opposite,
                    translated: translated.lead(p),
                    ..Line::default()
                },
                start_ms.is_none(),
            ))
        })
        .collect()
}

fn syllable_lines(
    divs: &[&Element],
    body: &Element,
    transliterations: &HashMap<String, TransliterationLine>,
    translated: &mut TranslationLookup,
    opposite_agents: &[String],
) -> Vec<(Line, bool)> {
    let mut lines = Vec::new();
    for div in divs {
        if div.attr("itunes:songPart") == Some("Instrumental") {
            continue;
        }
        for p in paragraphs(div) {
            if p.is_plain() && !has_lyrics_text(&p.own_text()) {
                continue;
            }
            let agent = p
                .attr("ttm:agent")
                .filter(|agent| !agent.is_empty())
                .or_else(|| div.attr("ttm:agent").filter(|agent| !agent.is_empty()))
                .or_else(|| body.attr("ttm:agent"));
            let opposite = p
                .attr("spicy:oppositeAligned")
                .and_then(|value| value.parse::<bool>().ok())
                .unwrap_or_else(|| {
                    agent.is_some_and(|agent| opposite_agents.iter().any(|id| id == agent))
                });
            let line_entry = p
                .attr("itunes:key")
                .and_then(|key| transliterations.get(key));
            let spans: Vec<&Element> = p.elements("span").collect();

            let mut lead = build_syllables(
                p,
                &spans,
                line_entry.map(|entry| entry.spans.as_slice()),
                false,
            );
            let lead_roman = inline_role_texts(p)
                .roman
                .map(|roman| roman.trim().to_string())
                .filter(|roman| !roman.is_empty());
            if lead.is_empty() {
                let fallback = line_text(p).unwrap_or_default().trim().to_string();
                if !fallback.is_empty() || lead_roman.is_some() {
                    let start = convert_time_to_seconds(p.attr("begin"));
                    lead.push(SyllableDraft {
                        text: fallback,
                        roman: lead_roman.clone(),
                        part_of_word: false,
                        start,
                        end: end_after(start, convert_time_to_seconds(p.attr("end"))),
                    });
                }
            }

            let mut lead_start = min_time(
                convert_time_to_seconds(p.attr("begin")),
                lead.iter()
                    .filter_map(|syllable| syllable.start)
                    .reduce(f64::min),
            );
            let mut lead_end = max_time(
                convert_time_to_seconds(p.attr("end")),
                lead.iter()
                    .filter_map(|syllable| syllable.end)
                    .reduce(f64::max),
            );
            let words_end = lead_end;

            let mut background = Vec::new();
            for bg_span in p.elements("span").filter(|span| is_role(span, "x-bg")) {
                let bg_children: Vec<&Element> = bg_span.elements("span").collect();
                let bg_vocal: Vec<&Element> = bg_children
                    .iter()
                    .copied()
                    .filter(|span| is_vocal_span(span))
                    .collect();
                let bg_entry = bg_span
                    .attr("itunes:key")
                    .and_then(|key| transliterations.get(key));
                let syllables = build_syllables(
                    bg_span,
                    &bg_children,
                    pick_background_transliterations(bg_entry, line_entry),
                    true,
                );
                if !syllables.iter().any(|syllable| {
                    has_lyrics_text(&syllable.text)
                        || syllable.roman.as_deref().is_some_and(has_lyrics_text)
                }) {
                    continue;
                }
                let start =
                    convert_time_to_seconds(bg_vocal.first().and_then(|span| span.attr("begin")))
                        .or_else(|| convert_time_to_seconds(bg_span.attr("begin")))
                        .or_else(|| syllables.first().and_then(|syllable| syllable.start));
                let end =
                    convert_time_to_seconds(bg_vocal.last().and_then(|span| span.attr("end")))
                        .or_else(|| convert_time_to_seconds(bg_span.attr("end")))
                        .or_else(|| syllables.last().and_then(|syllable| syllable.end));
                lead_end = max_time(lead_end, end);
                lead_start = min_time(lead_start, start);
                let start_ms = start.map(ms).unwrap_or(0);
                let end_ms = end.map(ms).unwrap_or(start_ms).max(start_ms);
                background.extend(to_syllables(&syllables, start_ms, end_ms));
            }

            let empty = lead.iter().all(|syllable| {
                !has_lyrics_text(&syllable.text)
                    && syllable
                        .roman
                        .as_deref()
                        .is_none_or(|roman| !has_lyrics_text(roman))
            }) && background.is_empty();
            if empty {
                continue;
            }

            let lead_start_ms = lead_start.map(ms).unwrap_or(0);
            let words_end_ms = words_end
                .map(ms)
                .unwrap_or(lead_start_ms)
                .max(lead_start_ms);
            let words = to_syllables(&lead, lead_start_ms, words_end_ms);
            let text = join_syllables(&words);
            let end_ms = lead_end.map(ms).unwrap_or(lead_start_ms).max(lead_start_ms);
            let untimed = lead_start.is_none();
            lines.push((
                Line {
                    at_ms: (!untimed).then_some(lead_start_ms),
                    end_ms: Some(end_ms),
                    text,
                    syllables: words,
                    background,
                    opposite_aligned: opposite,
                    transliterated: lead_roman,
                    translated: translated.lead(p),
                },
                untimed,
            ));
        }
    }
    lines
}

fn min_time(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn max_time(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn end_after(start: Option<f64>, end: Option<f64>) -> Option<f64> {
    end.filter(|end| start.is_none_or(|start| *end >= start))
}

fn build_syllables(
    parent: &Element,
    siblings: &[&Element],
    transliterations: Option<&[TimedSpan]>,
    strip_parentheses: bool,
) -> Vec<SyllableDraft> {
    let clean = |value: &str| {
        if strip_parentheses {
            value.trim().replace(['(', ')'], "").trim().to_string()
        } else {
            value.trim().to_string()
        }
    };
    let mut out = Vec::new();
    for (index, node) in siblings.iter().enumerate() {
        if !is_vocal_span(node) {
            continue;
        }
        let raw = node.own_text();
        let text = clean(&raw);
        if text.is_empty() {
            continue;
        }
        let next = siblings.get(index + 1);
        let touching = node.attr("begin").is_some_and(|begin| !begin.is_empty())
            && node.attr("end").is_some_and(|end| !end.is_empty())
            && parent
                .children
                .iter()
                .position(
                    |child| matches!(child, Node::Element(element) if std::ptr::eq(element, *node)),
                )
                .is_some_and(|at| matches!(parent.children.get(at + 1), Some(Node::Element(_))));
        let part_of_word = touching
            && next.is_some_and(|next| is_vocal_span(next))
            && next
                .and_then(|next| next.own_text().chars().next())
                .is_some_and(|first| !first.is_whitespace())
            && !raw.trim_end().ends_with(',')
            && !raw.chars().last().is_some_and(char::is_whitespace);
        let roman =
            find_transliterated_text(transliterations, node.attr("begin"), node.attr("end"))
                .map(|span| clean(&span.text))
                .filter(|roman| !roman.is_empty());
        let start = convert_time_to_seconds(node.attr("begin"));
        out.push(SyllableDraft {
            text,
            roman,
            part_of_word,
            start,
            end: end_after(start, convert_time_to_seconds(node.attr("end"))),
        });
    }
    out
}

/// Turns drafts into timed syllables. A syllable missing its end runs until
/// the next one starts (or the line ends); a run with no timing at all shares
/// that gap evenly.
fn to_syllables(
    syllables: &[SyllableDraft],
    line_start_ms: u32,
    line_end_ms: u32,
) -> Vec<Syllable> {
    let starts: Vec<Option<u32>> = syllables
        .iter()
        .map(|syllable| syllable.start.map(ms))
        .collect();
    let next_start = |after: usize| -> u32 {
        starts
            .iter()
            .skip(after + 1)
            .flatten()
            .copied()
            .next()
            .unwrap_or(line_end_ms)
    };
    let mut times: Vec<(u32, u32)> = Vec::with_capacity(syllables.len());
    let mut previous_end = line_start_ms;
    let mut index = 0;
    while index < syllables.len() {
        let syllable = &syllables[index];
        if starts[index].is_none() && syllable.end.is_none() {
            let mut end_index = index;
            while end_index < syllables.len()
                && starts[end_index].is_none()
                && syllables[end_index].end.is_none()
            {
                end_index += 1;
            }
            let until = next_start(end_index - 1).max(previous_end);
            let count = (end_index - index) as u32;
            let span = until.saturating_sub(previous_end);
            let step = span / count.max(1);
            for offset in index..end_index {
                let from = previous_end + step * (offset - index) as u32;
                let to = if offset == end_index - 1 {
                    until
                } else {
                    previous_end + step * (offset - index + 1) as u32
                };
                times.push((from, to));
            }
            previous_end = until;
            index = end_index;
            continue;
        }
        let start = starts[index].unwrap_or(previous_end);
        let end = syllable
            .end
            .map(ms)
            .unwrap_or_else(|| next_start(index))
            .max(start);
        times.push((start, end));
        previous_end = end;
        index += 1;
    }
    syllables
        .iter()
        .enumerate()
        .map(|(index, syllable)| {
            let (start, end) = times[index];
            Syllable {
                at_ms: start,
                end_ms: end,
                text: syllable.text.clone(),
                transliterated: syllable.roman.clone(),
                part_of_word: syllable.part_of_word,
                transliterated_part_of_word: None,
            }
        })
        .collect()
}

fn join_syllables(syllables: &[Syllable]) -> String {
    let mut text = String::new();
    for (index, syllable) in syllables.iter().enumerate() {
        if index > 0 && !syllables[index - 1].part_of_word {
            text.push(' ');
        }
        text.push_str(syllable.text.trim());
    }
    text.trim().to_string()
}

/// A line's text: its spans' text (space-joined where they carry none), else
/// its own.
fn line_text(p: &Element) -> Option<String> {
    if p.is_plain() {
        return Some(p.own_text());
    }
    let texts: Vec<String> = p
        .elements("span")
        .filter(|span| span.is_plain() || is_vocal_span(span))
        .map(Element::own_text)
        .filter(|text| !text.is_empty())
        .collect();
    if !texts.is_empty() {
        let mut joined = String::new();
        for text in texts {
            let needs_space = joined
                .chars()
                .last()
                .is_some_and(|last| !last.is_whitespace())
                && text
                    .chars()
                    .next()
                    .is_some_and(|first| !first.is_whitespace());
            if needs_space {
                joined.push(' ');
            }
            joined.push_str(&text);
        }
        return Some(joined);
    }
    p.has_text_child().then(|| p.own_text())
}

fn inline_role_texts(p: &Element) -> RoleTexts {
    let role = |name: &str| p.elements("span").find(|span| is_role(span, name));
    let translation = role("x-translation");
    RoleTexts {
        roman: role("x-roman").map(Element::own_text),
        translation: translation
            .map(lead_text)
            .map(|text| collapse(&text))
            .filter(|text| has_lyrics_text(text)),
    }
}

impl TranslationLookup {
    fn lead(&mut self, p: &Element) -> Option<String> {
        self.inline(p).or_else(|| {
            p.attr("itunes:key")
                .and_then(|key| self.head.as_ref().and_then(|head| head.by_key.get(key)))
                .map(|entry| entry.lead.clone())
                .filter(|text| has_lyrics_text(text))
        })
    }

    #[allow(dead_code)]
    fn background(&mut self, p: &Element, bg_span: &Element, index: usize) -> Option<String> {
        self.inline(bg_span)
            .or_else(|| {
                bg_span
                    .attr("itunes:key")
                    .and_then(|key| self.head.as_ref().and_then(|head| head.by_key.get(key)))
                    .map(|entry| entry.lead.clone())
                    .filter(|text| has_lyrics_text(text))
            })
            .or_else(|| {
                p.attr("itunes:key")
                    .and_then(|key| self.head.as_ref().and_then(|head| head.by_key.get(key)))
                    .and_then(|entry| entry.backgrounds.get(index).cloned())
                    .filter(|text| has_lyrics_text(text))
            })
    }

    fn inline(&mut self, node: &Element) -> Option<String> {
        inline_role_texts(node).translation
    }
}

fn translations(metadata: Option<&Element>) -> Option<Translations> {
    let metadata = metadata?;
    for entry in metadata.elements("iTunesMetadata") {
        let Some(block) = entry
            .element("translations")
            .and_then(|translations| translations.elements("translation").next())
        else {
            continue;
        };
        let by_key: HashMap<String, TranslatedLine> = block
            .elements("text")
            .filter_map(|text| {
                let key = text.attr("for")?;
                Some((
                    key.to_string(),
                    TranslatedLine {
                        lead: collapse(&lead_text(text)),
                        backgrounds: background_texts(text)
                            .iter()
                            .map(|text| collapse(text))
                            .collect(),
                    },
                ))
            })
            .collect();
        if !by_key.is_empty() {
            return Some(Translations { by_key });
        }
    }
    None
}

fn background_texts(node: &Element) -> Vec<String> {
    let mut out = Vec::new();
    for child in &node.children {
        let Node::Element(element) = child else {
            continue;
        };
        if element.attr("ttm:role") == Some("x-bg") {
            let text: String = element
                .children
                .iter()
                .map(|child| match child {
                    Node::Text(text) => text.clone(),
                    Node::Element(element) => lead_text(element),
                })
                .collect();
            out.push(text);
        } else {
            out.extend(background_texts(element));
        }
    }
    out
}

fn songwriters(metadata: Option<&Element>) -> Vec<String> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let entries: Vec<&Element> = metadata.elements("iTunesMetadata").collect();
    let entry = entries
        .iter()
        .copied()
        .find(|entry| entry.element("songwriters").is_some())
        .or_else(|| entries.first().copied());
    entry
        .and_then(|entry| entry.element("songwriters"))
        .map(|writers| {
            writers
                .elements("songwriter")
                .map(|writer| writer.own_text().trim().to_string())
                .filter(|writer| !writer.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn transliterations(metadata: Option<&Element>) -> HashMap<String, TransliterationLine> {
    let mut map: HashMap<String, TransliterationLine> = HashMap::new();
    let Some(metadata) = metadata else {
        return map;
    };
    for entry in metadata.elements("iTunesMetadata") {
        let Some(translations) = entry.element("transliterations") else {
            continue;
        };
        for block in translations.elements("transliteration") {
            for text in block.elements("text") {
                let Some(key) = text.attr("for") else {
                    continue;
                };
                let mut line = TransliterationLine::default();
                collect_transliteration_spans(
                    text.elements("span").collect::<Vec<_>>().as_slice(),
                    &mut line.spans,
                    &mut line.background,
                );
                line.text = collapse(&lead_text(text));
                if line.spans.is_empty() && line.background.is_empty() && line.text.is_empty() {
                    continue;
                }
                match map.get_mut(key) {
                    Some(existing) => {
                        existing.spans.extend(line.spans);
                        existing.background.extend(line.background);
                        if !line.text.is_empty() {
                            if !existing.text.is_empty() {
                                existing.text.push(' ');
                            }
                            existing.text.push_str(&line.text);
                        }
                    }
                    None => {
                        map.insert(key.to_string(), line);
                    }
                }
            }
        }
    }
    map
}

fn collect_transliteration_spans(
    nodes: &[&Element],
    out: &mut Vec<TimedSpan>,
    background: &mut Vec<TimedSpan>,
) {
    for node in nodes {
        if !is_span_object(node) {
            continue;
        }
        let children: Vec<&Element> = node.elements("span").collect();
        if !children.is_empty() {
            if node.attr("ttm:role") == Some("x-bg") {
                let mut own = Vec::new();
                collect_transliteration_spans(&children, &mut own, background);
                background.extend(own);
            } else {
                collect_transliteration_spans(&children, out, background);
            }
            continue;
        }
        let (Some(begin), Some(end)) = (node.attr("begin"), node.attr("end")) else {
            continue;
        };
        out.push(TimedSpan {
            begin: begin.to_string(),
            end: end.to_string(),
            begin_s: convert_time_to_seconds(Some(begin)),
            end_s: convert_time_to_seconds(Some(end)),
            text: node.own_text(),
        });
    }
}

fn pick_background_transliterations<'a>(
    background_entry: Option<&'a TransliterationLine>,
    line_entry: Option<&'a TransliterationLine>,
) -> Option<&'a [TimedSpan]> {
    for entry in [background_entry, line_entry].into_iter().flatten() {
        if !entry.background.is_empty() {
            return Some(&entry.background);
        }
        if !entry.spans.is_empty() {
            return Some(&entry.spans);
        }
    }
    None
}

fn find_transliterated_text<'a>(
    spans: Option<&'a [TimedSpan]>,
    begin: Option<&str>,
    end: Option<&str>,
) -> Option<&'a TimedSpan> {
    let spans = spans?;
    let (begin, end) = (begin?, end?);
    if spans.is_empty() {
        return None;
    }
    if let Some(exact) = spans
        .iter()
        .find(|span| span.begin == begin && span.end == end)
    {
        return Some(exact);
    }
    let begin_s = convert_time_to_seconds(Some(begin))?;
    let end_s = convert_time_to_seconds(Some(end))?;
    spans.iter().find(|span| {
        span.begin_s.zip(span.end_s).is_some_and(|(at, until)| {
            (at - begin_s).abs() <= 0.002 && (until - end_s).abs() <= 0.002
        })
    })
}

/// All the text under `node`, the spaces between spans included, leaving out
/// `x-bg` wrappers.
fn lead_text(node: &Element) -> String {
    let mut out = String::new();
    for child in &node.children {
        match child {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) if element.attr("ttm:role") == Some("x-bg") => {}
            Node::Element(element) => out.push_str(&lead_text(element)),
        }
    }
    out
}

/// Synced lines with no timing at all get a window in the gap around them, so
/// the view still scrolls through them in order.
fn place_untimed(lines: Vec<(Line, bool)>) -> Vec<Line> {
    if !lines.iter().any(|(_, untimed)| *untimed) {
        return lines.into_iter().map(|(line, _)| line).collect();
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].1 {
            out.push(lines[index].0.clone());
            index += 1;
            continue;
        }
        let mut end = index;
        while end < lines.len() && lines[end].1 {
            end += 1;
        }
        let from = lines[..index]
            .iter()
            .filter(|(_, untimed)| !*untimed)
            .filter_map(|(line, _)| line.end_ms.or(line.at_ms))
            .max()
            .unwrap_or(0);
        let next = lines[end..]
            .iter()
            .find(|(_, untimed)| !*untimed)
            .and_then(|(line, _)| line.at_ms);
        const UNTIMED_MS: u32 = 4_000;
        let count = (end - index) as u32;
        let until =
            from.max(next.unwrap_or_else(|| from.saturating_add(UNTIMED_MS.saturating_mul(count))));
        for (line, _) in &lines[index..end] {
            let mut line = line.clone();
            line.at_ms = Some(from);
            line.end_ms = Some(until.max(from));
            for syllable in line.syllables.iter_mut().chain(line.background.iter_mut()) {
                syllable.at_ms = from;
                syllable.end_ms = until.max(from);
            }
            out.push(line);
        }
        index = end;
    }
    out
}

// ---- times -------------------------------------------------------------------

fn ms(seconds: f64) -> u32 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * 1000.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// Clock time `[hh:]mm:ss[.fraction]` or an offset such as `12.5s`, `300ms`,
/// `2m` or `1.5h`. SMPTE frames and ticks need a rate, so they are refused.
fn convert_time_to_seconds(value: Option<&str>) -> Option<f64> {
    let time = value?.trim();
    if time.is_empty() {
        return None;
    }
    if time.contains(':') {
        let parts: Vec<&str> = time.split(':').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let reversed: Vec<&str> = parts.iter().rev().copied().collect();
        let offset = if parts.len() == 4 { 1 } else { 0 };
        let mut seconds = 0.0;
        for (index, part) in reversed.iter().enumerate().skip(offset) {
            let part = leading_float(part)?;
            seconds += part * 60f64.powi((index - offset) as i32);
        }
        return finite(seconds);
    }
    let (number, rest) = split_number(time)?;
    let number = number.parse::<f64>().ok()?;
    match rest.as_str() {
        "ms" => finite(number / 1000.0),
        "h" => finite(number * 3600.0),
        "m" => finite(number * 60.0),
        "s" | "" => finite(number),
        _ => None,
    }
}

fn finite(seconds: f64) -> Option<f64> {
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
}

/// The longest leading number, as JavaScript's `parseFloat` takes it.
fn leading_float(text: &str) -> Option<f64> {
    split_number(text.trim()).and_then(|(number, _)| number.parse().ok())
}

fn split_number(text: &str) -> Option<(String, String)> {
    let bytes: Vec<char> = text.chars().collect();
    let mut index = 0;
    while bytes.get(index).is_some_and(|c| c.is_whitespace()) {
        index += 1;
    }
    let start = index;
    if bytes.get(index).is_some_and(|c| *c == '+' || *c == '-') {
        index += 1;
    }
    let mut digits = false;
    while bytes.get(index).is_some_and(char::is_ascii_digit) {
        index += 1;
        digits = true;
    }
    if bytes.get(index) == Some(&'.') {
        index += 1;
        while bytes.get(index).is_some_and(char::is_ascii_digit) {
            index += 1;
            digits = true;
        }
    }
    if !digits {
        return None;
    }
    if bytes.get(index).is_some_and(|c| *c == 'e' || *c == 'E') {
        let mut probe = index + 1;
        if bytes.get(probe).is_some_and(|c| *c == '+' || *c == '-') {
            probe += 1;
        }
        let mut exponent = false;
        while bytes.get(probe).is_some_and(char::is_ascii_digit) {
            probe += 1;
            exponent = true;
        }
        if exponent {
            index = probe;
        }
    }
    let number: String = bytes[start..index].iter().collect();
    let rest: String = bytes[index..].iter().collect();
    Some((number, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str, timing: Option<&str>, head: &str) -> String {
        let timing = timing
            .map(|timing| format!(" itunes:timing=\"{timing}\""))
            .unwrap_or_default();
        format!(
            "<tt xmlns=\"http://www.w3.org/ns/ttml\" xmlns:ttm=\"http://www.w3.org/ns/ttml#metadata\" xmlns:itunes=\"http://music.apple.com/lyric-ttml-internal\"{timing}><head><metadata>{head}</metadata></head><body>{body}</body></tt>"
        )
    }

    fn parse_xml(xml: &str) -> Lyrics {
        parse(xml).expect("lyrics")
    }

    fn word(xml: &str) -> Line {
        parse_xml(xml).lines.into_iter().next().expect("a line")
    }

    #[test]
    fn a_span_is_one_syllable_with_its_own_timing() {
        let line = word(&doc(
            r#"<div><p begin="1s" end="3s"><span begin="1s" end="2s">two words</span> <span begin="2s" end="3s">well-known</span></p></div>"#,
            Some("Word"),
            "",
        ));
        assert_eq!(
            line.syllables
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["two words", "well-known"]
        );
        assert_eq!(
            line.syllables.iter().map(|s| s.at_ms).collect::<Vec<_>>(),
            [1_000, 2_000]
        );
        assert_eq!(line.text, "two words well-known");
    }

    #[test]
    fn a_line_covers_its_own_words_whatever_its_tag_says() {
        let line = word(&doc(
            r#"<div><p begin="195.973s" end="195.973s"><span begin="195.973s" end="196.777s">Galileo</span></p></div>"#,
            Some("Word"),
            "",
        ));
        assert_eq!((line.at_ms, line.end_ms), (Some(195_973), Some(196_777)));

        let late = word(&doc(
            r#"<div><p begin="12s" end="13s"><span begin="11.5s" end="12.5s">Early</span> <span begin="12.5s" end="14s">late</span></p></div>"#,
            Some("Word"),
            "",
        ));
        assert_eq!((late.at_ms, late.end_ms), (Some(11_500), Some(14_000)));
    }

    #[test]
    fn syllables_join_only_when_their_tags_touch() {
        let p = r#"<p begin="1" end="5"><span begin="1" end="2">Ma</span><span begin="2" end="3">ma,</span><span begin="3" end="4">mi</span> <span begin="4" end="5">a</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(
            line.syllables
                .iter()
                .map(|s| s.part_of_word)
                .collect::<Vec<_>>(),
            [true, false, false, false]
        );
        assert_eq!(line.text, "Mama, mi a");
    }

    #[test]
    fn a_background_span_in_between_ends_the_word() {
        let p = r#"<p begin="1" end="3"><span begin="1" end="2">Ma</span><span ttm:role="x-bg"><span begin="1" end="2">(oh)</span></span><span begin="2" end="3">ma</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert!(
            line.syllables.iter().all(|syllable| !syllable.part_of_word),
            "a background span between two parts ends the word"
        );
        assert_eq!(line.background.len(), 1);
        assert_eq!(line.background[0].text, "oh");
    }

    #[test]
    fn background_vocals_lose_every_parenthesis() {
        let p = r#"<p begin="1" end="3"><span begin="1" end="2">A</span><span ttm:role="x-bg"><span begin="1" end="2">(ooh (yeah))</span></span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(line.background[0].text, "ooh yeah");
    }

    #[test]
    fn only_declared_v2_and_v2000_sing_from_the_other_side() {
        let head = r#"<ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><ttm:agent type="person" xml:id="v3"/>"#;
        let body: String = ["v1", "v2", "v3"]
            .into_iter()
            .map(|agent| {
                format!(
                    r#"<p begin="1" end="2" ttm:agent="{agent}"><span begin="1" end="2">x</span></p>"#
                )
            })
            .collect();
        let lyrics = parse_xml(&doc(&format!("<div>{body}</div>"), Some("Word"), head));
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|l| l.opposite_aligned)
                .collect::<Vec<_>>(),
            [false, true, false]
        );
    }

    #[test]
    fn instrumental_sections_and_empty_lines_are_skipped() {
        let body = concat!(
            r#"<div itunes:songPart="Instrumental"><p begin="0" end="1"><span begin="0" end="1">skip</span></p></div>"#,
            r#"<div><p begin="1" end="2"><span begin="1" end="2"> </span></p><p begin="2" end="3"><span begin="2" end="3">keep</span></p></div>"#
        );
        let lyrics = parse_xml(&doc(body, Some("Word"), ""));
        assert_eq!(
            lyrics
                .lines
                .iter()
                .flat_map(|line| line.syllables.iter())
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["keep"]
        );
    }

    #[test]
    fn a_byte_order_mark_or_whitespace_before_the_declaration_is_ignored() {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>{}"#,
            doc(
                r#"<div><p begin="1s" end="2s"><span begin="1s" end="2s">Hello</span></p></div>"#,
                Some("Word"),
                ""
            )
        );
        for prefix in ["\u{feff}", "\n  ", "\u{feff}\r\n"] {
            let lyrics = parse_xml(&format!("{prefix}{xml}"));
            assert_eq!(lyrics.lines[0].syllables[0].text, "Hello");
        }
    }

    #[test]
    fn nbsp_reads_as_a_no_break_space() {
        let p = r#"<p begin="1s" end="2s"><span begin="1s" end="2s">Hello&nbsp;there</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(line.syllables[0].text, "Hello\u{a0}there");
    }

    #[test]
    fn a_span_ending_before_it_begins_runs_to_the_next_span() {
        let p = r#"<p begin="1s" end="4s"><span begin="1s" end="2s">One</span> <span begin="3s" end="2s">two</span> <span begin="3.5s" end="4s">three</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(
            line.syllables
                .iter()
                .map(|s| (s.at_ms, s.end_ms))
                .collect::<Vec<_>>(),
            [(1_000, 2_000), (3_000, 3_500), (3_500, 4_000)]
        );
    }

    #[test]
    fn a_line_ending_before_it_begins_ends_where_it_begins() {
        let lyrics = parse_xml(&doc(
            r#"<div><p begin="5s" end="3s">Words</p><p begin="6s" end="7s"><span begin="6s" end="7s">x</span></p></div>"#,
            Some("Word"),
            "",
        ));
        assert_eq!(
            (lyrics.lines[0].at_ms, lyrics.lines[0].end_ms),
            (Some(5_000), Some(5_000))
        );
        let line_synced = word(&doc(
            r#"<div><p begin="5s" end="3s">Words</p></div>"#,
            Some("Line"),
            "",
        ));
        assert_eq!(
            (line_synced.at_ms, line_synced.end_ms),
            (Some(5_000), Some(5_000))
        );
    }

    #[test]
    fn untimed_spans_share_the_gap_until_the_next_timed_one() {
        let p = concat!(
            r#"<p begin="1s" end="6s"><span begin="1s" end="2s">A</span> <span xml:lang="en">b</span> <span xml:lang="en">c</span> "#,
            r#"<span begin="4s" end="5s">D</span> <span xml:lang="en">e</span></p>"#
        );
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(
            line.syllables
                .iter()
                .map(|s| (s.at_ms, s.end_ms))
                .collect::<Vec<_>>(),
            [
                (1_000, 2_000),
                (2_000, 3_000),
                (3_000, 4_000),
                (4_000, 5_000),
                (5_000, 6_000),
            ]
        );
    }

    #[test]
    fn reads_every_time_format() {
        assert_eq!(convert_time_to_seconds(Some("300ms")), Some(0.3));
        assert_eq!(convert_time_to_seconds(Some("1.5m")), Some(90.0));
        assert_eq!(convert_time_to_seconds(Some("01:02:03.5")), Some(3723.5));
        assert_eq!(convert_time_to_seconds(Some("1:03.25")), Some(63.25));
        assert_eq!(convert_time_to_seconds(Some("12.5s")), Some(12.5));
        assert_eq!(convert_time_to_seconds(Some("10f")), None);
    }

    #[test]
    fn transliterations_are_matched_by_timing() {
        let head = concat!(
            r#"<iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal"><transliterations><transliteration><text for="L1">"#,
            r#"<span begin="00:00:02.000" end="00:00:03.000">de</span><span begin="1s" end="2s">nan</span></text></transliteration></transliterations></iTunesMetadata>"#
        );
        let p = r#"<p begin="1s" end="3s" itunes:key="L1"><span begin="1s" end="2s">なん</span><span begin="2s" end="3s">で</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), head));
        assert_eq!(
            line.syllables
                .iter()
                .filter_map(|s| s.transliterated.as_deref())
                .collect::<Vec<_>>(),
            ["nan", "de"]
        );
    }

    #[test]
    fn one_timed_span_per_line_without_a_declared_timing_is_line_synced() {
        let lyrics = parse_xml(&doc(
            r#"<div><p begin="1s" end="2s"><span begin="1s" end="2s">hello there</span></p></div>"#,
            None,
            "",
        ));
        assert!(lyrics.synced);
        assert!(!lyrics.word_synced());
        assert_eq!(lyrics.lines[0].text, "hello there");
    }

    #[test]
    fn a_line_without_timing_starts_with_its_first_syllable() {
        let lyrics = parse_xml(&doc(
            r#"<div begin="0s"><p><span begin="4s" end="5s">late</span><span begin="5s" end="6s">r</span></p></div>"#,
            Some("Word"),
            "",
        ));
        let line = &lyrics.lines[0];
        assert_eq!(line.at_ms, Some(4_000));
        assert_eq!(line.end_ms, Some(6_000));
        assert_eq!(line.text, "later");
    }

    #[test]
    fn every_background_vocal_on_a_line_is_kept() {
        let p = concat!(
            r#"<p begin="1" end="4"><span begin="1" end="2">A</span><span ttm:role="x-bg"><span begin="1" end="2">(oh)</span></span>"#,
            r#"<span ttm:role="x-bg"><span begin="3" end="4">(yeah)</span></span></p>"#
        );
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Word"), ""));
        assert_eq!(
            line.background
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["oh", "yeah"]
        );
        assert_eq!(line.end_ms, Some(4_000));
    }

    #[test]
    fn line_timed_transliterations_keep_the_spaces_between_spans() {
        let head = concat!(
            r#"<iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal"><transliterations><transliteration>"#,
            r#"<text for="L1"><span begin="1s" end="2s">konnichiwa</span> <span begin="2s" end="3s">sekai</span><span ttm:role="x-bg"><span begin="2s" end="3s">oh</span></span></text>"#,
            r#"<text for="L2">plain  text</text></transliteration></transliterations></iTunesMetadata>"#
        );
        let body = r#"<div><p begin="1s" end="3s" itunes:key="L1">こんにちは世界</p><p begin="3s" end="4s" itunes:key="L2">平文</p></div>"#;
        let lyrics = parse_xml(&doc(body, Some("Line"), head));
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.transliterated.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["konnichiwa sekai", "plain text"]
        );
    }

    #[test]
    fn a_line_with_only_its_romanization_is_kept() {
        let p = r#"<p begin="1" end="2"><span ttm:role="x-roman">romaji</span></p>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Line"), ""));
        assert_eq!(line.text, "");
        assert_eq!(line.transliterated.as_deref(), Some("romaji"));
    }

    #[test]
    fn inline_translation_and_songwriters_are_read() {
        let p = r#"<p begin="1" end="2"><span begin="1" end="2">Hallo</span><span ttm:role="x-translation"><span xml:lang="en">Hello</span></span></p>"#;
        let head = r#"<iTunesMetadata><songwriters><songwriter>Ada Writer</songwriter></songwriters></iTunesMetadata>"#;
        let line = word(&doc(&format!("<div>{p}</div>"), Some("Line"), head));
        assert_eq!(line.translated.as_deref(), Some("Hello"));
        assert_eq!(
            parse_xml(&doc(&format!("<div>{p}</div>"), Some("Line"), head)).song_writers,
            ["Ada Writer"]
        );
    }

    #[test]
    fn head_translations_fill_untranslated_lines() {
        let head = concat!(
            r#"<iTunesMetadata><translations><translation xml:lang="fr"><text for="L1">Bonjour</text>"#,
            r#"</translation></translations></iTunesMetadata>"#
        );
        let body = r#"<div><p begin="1" end="2" itunes:key="L1"><span begin="1" end="2">Hello</span></p></div>"#;
        let line = word(&doc(body, Some("Line"), head));
        assert_eq!(line.translated.as_deref(), Some("Bonjour"));
    }

    #[test]
    fn a_static_document_carries_no_timing() {
        let lyrics = parse_xml(&doc("<div><p>One</p><p>Two</p></div>", Some("None"), ""));
        assert!(!lyrics.synced);
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["One", "Two"]
        );
        assert!(lyrics.lines.iter().all(|line| line.at_ms.is_none()));
    }

    #[test]
    fn untimed_lines_are_placed_in_the_gap_around_them() {
        let body = concat!(
            r#"<div><p begin="1" end="2"><span begin="1" end="2">a</span></p>"#,
            r#"<p>middle</p><p>another</p>"#,
            r#"<p begin="10" end="11"><span begin="10" end="11">b</span></p></div>"#
        );
        let lyrics = parse_xml(&doc(body, Some("Word"), ""));
        assert_eq!(lyrics.lines.len(), 4);
        assert_eq!(lyrics.lines[1].at_ms, Some(2_000));
        assert_eq!(lyrics.lines[2].at_ms, Some(2_000));
        assert_eq!(lyrics.lines[1].end_ms, Some(10_000));
        assert_eq!(lyrics.lines[3].at_ms, Some(10_000));
    }

    #[test]
    fn malformed_documents_produce_nothing_instead_of_panicking() {
        assert!(parse("").is_none());
        assert!(parse("<tt><body><div><p>").is_none());
        assert!(parse("<html><body><p>Words</p></body></html>").is_none());
        assert!(parse("<tt><body></body></tt>").is_none());
    }

    #[test]
    fn comments_cdata_and_doctypes_do_not_confuse_the_reader() {
        let xml = concat!(
            "<!DOCTYPE tt [<!ELEMENT tt ANY>]>",
            "<!-- a comment with <tags> -->",
            r#"<tt><body><div><p begin="1" end="2"><span begin="1" end="2"><![CDATA[Hi & bye]]></span></p></div></body></tt>"#
        );
        let lyrics = parse(xml).unwrap();
        assert_eq!(lyrics.lines[0].text, "Hi & bye");
    }
}
