//! Lyrics from [NetEase Cloud Music](https://music.163.com).
//!
//! NetEase has human word timing (YRC), line timing (LRC) and plain words.
//! YRC is converted to a TTML document first, so the same parser reads every
//! word-synced source. Credit lines these catalogues stamp into the lyrics are
//! dropped rather than sung.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::remote::{
    duration_matches, get_json, is_credit, is_no_words_note, same_track, string_at, with_cache,
};
use super::{Attribution, Line, Lyrics, Query, cache_key, parse_lrc, ttml};

const BASE: &str = "https://music.163.com/api";
const SOURCE: &str = "netease";
const REFERER: &str = "https://music.163.com";
/// How many equally good search hits are worth a lyric request.
const TRIES: usize = 3;

/// NetEase's lyrics for `query`, or `None` when it has none or cannot be read.
pub async fn fetch_netease(
    http: &reqwest::Client,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    let key = format!("netease-{}", cache_key(query));
    with_cache(cache_dir, &key, request_from(http, BASE, query)).await
}

async fn request_from(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    let Some(root) = get_json(
        http,
        &format!("{base}/cloudsearch/pc"),
        &[
            ("s", &format!("{} {}", query.artist, query.title)),
            ("type", "1"),
            ("offset", "0"),
            ("limit", "8"),
        ],
        &[("Referer", REFERER)],
    )
    .await?
    else {
        return Ok(None);
    };
    let songs: Vec<&Value> = root
        .get("result")
        .and_then(|result| result.get("songs"))
        .and_then(Value::as_array)
        .map(|songs| {
            songs
                .iter()
                .filter(|song| matches(song, query))
                .take(TRIES)
                .collect()
        })
        .unwrap_or_default();
    let ids: Vec<String> = songs
        .iter()
        .filter_map(|song| string_at(song, "id"))
        .collect();
    let requests = ids.iter().map(|id| {
        let url = format!("{base}/song/lyric");
        async move {
            get_json(
                http,
                &url,
                &[
                    ("os", "pc"),
                    ("id", id.as_str()),
                    ("lv", "-1"),
                    ("kv", "-1"),
                    ("tv", "-1"),
                    ("yv", "-1"),
                    ("rv", "-1"),
                ],
                &[("Referer", REFERER)],
            )
            .await
            .ok()
            .flatten()
        }
    });
    let answers: Vec<Value> = futures_util::future::join_all(requests)
        .await
        .into_iter()
        .flatten()
        .collect();
    // Word timing belongs to a release, not a song, and the search's favourite
    // is often an untimed copy: take the best of the candidates, preferring
    // word timing over line timing over plain words.
    if let Some(mut lyrics) = best_lyrics(&answers) {
        lyrics.attribution = Some(Attribution {
            source: SOURCE.to_string(),
            uploader: None,
            maker: None,
        });
        return Ok(Some(lyrics));
    }
    Ok(None)
}

/// The best of several candidate answers: word timing first, then line timing,
/// then plain words.
fn best_lyrics(answers: &[Value]) -> Option<Lyrics> {
    answers
        .iter()
        .filter_map(answer_lyrics)
        .max_by_key(|lyrics| (lyrics.word_synced(), lyrics.synced))
}

fn matches(song: &Value, query: &Query) -> bool {
    let Some(title) = string_at(song, "name") else {
        return false;
    };
    let artists = song
        .get("ar")
        .or_else(|| song.get("artists"))
        .and_then(Value::as_array)
        .map(|artists| {
            artists
                .iter()
                .filter_map(|artist| string_at(artist, "name"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let duration_ms = song
        .get("dt")
        .or_else(|| song.get("duration"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as u64;
    same_track(&title, &artists, query) && duration_matches(duration_ms, query)
}

fn answer_lyrics(answer: &Value) -> Option<Lyrics> {
    if let Some(yrc) = answer
        .get("yrc")
        .and_then(|yrc| yrc.get("lyric"))
        .and_then(Value::as_str)
    {
        let lines: Vec<String> = yrc
            .lines()
            .map(|line| line.replace(['[', ']'], ""))
            .collect();
        if !is_no_words_note(&lines)
            && let Some(document) = yrc_to_ttml(yrc)
            && let Some(lyrics) = ttml::parse(&document)
        {
            return Some(lyrics);
        }
    }
    let lrcs: Vec<&str> = ["lrc"]
        .into_iter()
        .filter_map(|part| answer.get(part))
        .filter_map(|part| part.get("lyric"))
        .filter_map(Value::as_str)
        .filter(|lrc| !lrc.trim().is_empty())
        .filter(|lrc| !is_no_words_note(&lrc.lines().map(strip_stamps).collect::<Vec<_>>()))
        .collect();
    if let Some(lyrics) = lrcs.iter().find_map(|lrc| timed(lrc)) {
        return Some(lyrics);
    }
    lrcs.first().and_then(|lrc| plain(lrc))
}

fn strip_stamps(line: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for character in line.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn timed(lrc: &str) -> Option<Lyrics> {
    let lines: Vec<Line> = parse_lrc(lrc)
        .into_iter()
        .filter(|line| !line.text.is_empty() && !is_credit(&line.text))
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(Lyrics {
        lines,
        synced: true,
        instrumental: false,
        ..Lyrics::default()
    })
}

fn plain(lrc: &str) -> Option<Lyrics> {
    let lines: Vec<Line> = lrc
        .lines()
        .map(strip_stamps)
        .filter(|line| !line.is_empty() && !is_credit(line))
        .map(|line| Line::new(None, line))
        .collect();
    if lines.is_empty()
        || is_no_words_note(&lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>())
    {
        return None;
    }
    Some(Lyrics {
        lines,
        synced: false,
        instrumental: false,
        ..Lyrics::default()
    })
}

/// NetEase's word-timed lyrics (`[lineStart,lineLength](wordStart,wordLength,0)word...`,
/// in milliseconds) as a TTML document.
fn yrc_to_ttml(yrc: &str) -> Option<String> {
    let mut paragraphs = Vec::new();
    for raw in yrc.lines() {
        let Some((start, duration, rest)) = parse_line_stamp(raw.trim()) else {
            continue;
        };
        let mut words = Vec::new();
        let mut rest = rest;
        while let Some(open) = rest.find('(') {
            let Some(close) = rest[open..].find(')') else {
                break;
            };
            let inside = &rest[open + 1..open + close];
            let mut parts = inside.split(',');
            let at = parts
                .next()
                .and_then(|value| value.trim().parse::<u64>().ok());
            let length = parts
                .next()
                .and_then(|value| value.trim().parse::<u64>().ok());
            let after = &rest[open + close + 1..];
            let text_end = after.find('(').unwrap_or(after.len());
            let text = &after[..text_end];
            rest = &after[text_end..];
            let (Some(at), Some(length)) = (at, length) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            words.push((at, length, text));
        }
        if words.is_empty() {
            continue;
        }
        let body = words.iter().map(|(_, _, text)| *text).collect::<String>();
        let body = body.trim();
        if body.is_empty() || is_credit(body) || body.contains("版权") || body.contains("版權")
        {
            continue;
        }
        // Some catalogues time a line's words from the line's start; if they
        // sit well before it, add the start back.
        let base = if words[0].0 + 1_000 < start { start } else { 0 };
        let spans: String = words
            .iter()
            .map(|(at, length, text)| {
                format!(
                    "<span begin=\"{}\" end=\"{}\">{}</span>",
                    seconds(base + at),
                    seconds(base + at + length),
                    escape(text)
                )
            })
            .collect();
        paragraphs.push(format!(
            "<p begin=\"{}\" end=\"{}\">{spans}</p>",
            seconds(start),
            seconds(start + duration)
        ));
    }
    if paragraphs.is_empty() {
        return None;
    }
    Some(format!(
        "<tt xmlns=\"http://www.w3.org/ns/ttml\" xmlns:itunes=\"http://music.apple.com/lyric-ttml-internal\" itunes:timing=\"word\"><body><div>{}</div></body></tt>",
        paragraphs.concat()
    ))
}

/// `[start,duration]` and the rest of the line.
fn parse_line_stamp(line: &str) -> Option<(u64, u64, &str)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let stamp = &rest[..close];
    let (start, duration) = stamp.split_once(',')?;
    Some((
        start.trim().parse().ok()?,
        duration.trim().parse().ok()?,
        &rest[close + 1..],
    ))
}

fn seconds(ms: u64) -> String {
    format!("{:.3}s", ms as f64 / 1000.0)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> Query {
        Query {
            artist: "Artist".into(),
            title: "Song".into(),
            album: String::new(),
            duration_ms: 120_000,
        }
    }

    #[test]
    fn yrc_becomes_a_word_timed_ttml() {
        let yrc = "[28480,1500](28480,500,0)Hel(28980,500,0)lo (29480,500,0)world";
        let document = yrc_to_ttml(yrc).unwrap();
        let lyrics = ttml::parse(&document).unwrap();
        let line = &lyrics.lines[0];
        assert_eq!(line.text, "Hello world");
        assert_eq!(line.at_ms, Some(28_480));
        assert_eq!(
            line.syllables
                .iter()
                .map(|syllable| (syllable.at_ms, syllable.text.as_str()))
                .collect::<Vec<_>>(),
            [(28_480, "Hel"), (28_980, "lo"), (29_480, "world")]
        );
    }

    #[test]
    fn yrc_credits_and_rights_notices_are_dropped() {
        let yrc = concat!(
            "[0,1000](0,400,0)作词(400,600,0) : Someone\n",
            "[1000,1000](1000,500,0)词版权管理方(1500,500,0)：Label\n",
            "[2000,1000](2000,1000,0)A line"
        );
        let document = yrc_to_ttml(yrc).unwrap();
        let lyrics = ttml::parse(&document).unwrap();
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "A line");
    }

    #[test]
    fn an_instrumental_note_is_not_lyrics() {
        let answer = serde_json::json!({
            "lrc": {"lyric": "[00:00.00]纯音乐，请欣赏\n[00:10.00]纯音乐，请欣赏"}
        });
        assert!(answer_lyrics(&answer).is_none());
    }

    #[test]
    fn a_timed_lrc_falls_back_to_line_sync() {
        let answer = serde_json::json!({
            "lrc": {"lyric": "[00:01.00]One\n[00:02.00]Two"}
        });
        let lyrics = answer_lyrics(&answer).unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.attribution, None);
    }

    #[test]
    fn plain_words_keep_no_timing() {
        let answer = serde_json::json!({
            "lrc": {"lyric": "[00:00.00]Lyrics: Someone\nOne\nTwo"}
        });
        let lyrics = answer_lyrics(&answer).unwrap();
        assert!(!lyrics.synced);
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["One", "Two"]
        );
    }

    #[test]
    fn word_timing_wins_over_a_better_ranked_line_sync() {
        let line_timed = serde_json::json!({
            "lrc": {"lyric": "[00:01.00]One\n[00:02.00]Two"}
        });
        let word_timed = serde_json::json!({
            "yrc": {"lyric": "[1000,1000](1000,500,0)One"}
        });
        let best = best_lyrics(&[line_timed, word_timed]).unwrap();
        assert!(best.word_synced());
    }

    #[test]
    fn a_different_artist_is_not_the_same_song() {
        let song = serde_json::json!({"name": "Song", "ar": [{"name": "Other"}], "dt": 120000});
        assert!(!matches(&song, &query()));
        let song =
            serde_json::json!({"name": "Song (Live)", "ar": [{"name": "Artist"}], "dt": 120000});
        assert!(matches(&song, &query()));
    }
}
