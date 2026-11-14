//! Word-synced lyrics from [Unison](https://unison.boidu.dev).
//!
//! Unison's search finds the song and who submitted it. Its detail answer
//! carries TTML, LRC or plain text; a title lookup is the fallback when the
//! search finds nothing.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::remote::{duration_matches, get_json, same_track, string_at, with_cache};
use super::{Attribution, Credit, Lyrics, Query, cache_key, from_text, parse_lrc, ttml};

const BASE: &str = "https://unison.boidu.dev";
const SOURCE: &str = "unison";
/// At most this many search hits are tried, best first.
const TRIES: usize = 3;

/// Unison's lyrics for `query`, or `None` when it has none or cannot be read.
pub async fn fetch_unison(
    http: &reqwest::Client,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    let key = format!("unison-{}", cache_key(query));
    with_cache(cache_dir, &key, request_from(http, BASE, query)).await
}

async fn request_from(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    if let Some(found) = search(http, base, query).await? {
        return Ok(Some(found));
    }
    direct(http, base, query).await
}

async fn direct(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    let Some(root) = get_json(
        http,
        &format!("{base}/lyrics"),
        &[
            ("song", query.title.as_str()),
            ("artist", query.artist.as_str()),
        ],
        &[],
    )
    .await?
    else {
        return Ok(None);
    };
    let data = root.get("data").unwrap_or(&root);
    if !matches(data, query) {
        return Ok(None);
    }
    Ok(payload(data))
}

async fn search(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    let Some(root) = get_json(
        http,
        &format!("{base}/lyrics/search"),
        &[("q", &format!("{} {}", query.artist, query.title))],
        &[],
    )
    .await?
    else {
        return Ok(None);
    };
    let hits = root
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Some(hit) = hits
        .iter()
        .filter(|hit| matches(hit, query))
        .take(TRIES)
        .next()
    else {
        return Ok(None);
    };
    let Some(id) = string_at(hit, "id") else {
        return Ok(None);
    };
    let Some(root) = get_json(http, &format!("{base}/lyrics/{id}"), &[], &[]).await? else {
        return Ok(None);
    };
    let data = root.get("data").unwrap_or(&root);
    let Some(mut lyrics) = payload(data) else {
        return Ok(None);
    };
    if lyrics.attribution.is_none() {
        lyrics.attribution = attribution(hit);
    }
    Ok(Some(lyrics))
}

/// The lyrics in an answer, by the format it names and the shape it has.
fn payload(data: &Value) -> Option<Lyrics> {
    let text = string_at(data, "lyrics")?;
    let format = string_at(data, "format").unwrap_or_default().to_lowercase();
    let mut lyrics = if format == "ttml" || text.contains("<tt") {
        ttml::parse(&text)?
    } else if format == "lrc" || looks_like_lrc(&text) {
        let lines = parse_lrc(&text);
        if lines.is_empty() {
            return None;
        }
        Lyrics {
            lines,
            synced: true,
            instrumental: false,
            ..Lyrics::default()
        }
    } else if matches!(format.as_str(), "plain" | "txt" | "") {
        from_text(&text)?
    } else {
        return None;
    };
    lyrics.attribution = attribution(data).or(lyrics.attribution);
    Some(lyrics)
}

fn looks_like_lrc(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with('[')
            && line
                .chars()
                .skip(1)
                .take_while(|c| *c != ']')
                .any(|c| c == ':')
    })
}

fn attribution(entry: &Value) -> Option<Attribution> {
    let submitter = entry.get("submitter")?;
    let name = string_at(submitter, "displayName")?;
    let url = string_at(submitter, "keyId")
        .filter(|key| {
            !key.is_empty() && key.len() >= 16 && key.chars().all(|c| c.is_ascii_hexdigit())
        })
        .map(|key| format!("{BASE}/curator/{key}"));
    Some(Attribution {
        source: SOURCE.to_string(),
        uploader: Some(Credit { name, url }),
        maker: None,
    })
}

fn matches(entry: &Value, query: &Query) -> bool {
    let title = string_at(entry, "song").or_else(|| string_at(entry, "title"));
    let artist = string_at(entry, "artist");
    let (Some(title), Some(artist)) = (title, artist) else {
        return false;
    };
    same_track(&title, &artist, query) && duration_matches(0, query)
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
    fn a_ttml_answer_becomes_word_synced_lyrics() {
        let data = serde_json::json!({
            "lyrics": "<tt xmlns=\"http://www.w3.org/ns/ttml\" itunes:timing=\"word\"><body><div><p begin=\"1\" end=\"2\"><span begin=\"1\" end=\"2\">Hi</span></p></div></body></tt>",
            "format": "ttml",
            "submitter": {"displayName": "Ada", "keyId": "0123456789abcdef"}
        });
        let lyrics = payload(&data).unwrap();
        assert!(lyrics.word_synced());
        let attribution = lyrics.attribution.unwrap();
        assert_eq!(attribution.source, "unison");
        assert_eq!(attribution.uploader.unwrap().name, "Ada");
    }

    #[test]
    fn an_lrc_answer_stays_line_synced() {
        let data = serde_json::json!({
            "lyrics": "[00:01.00]One\n[00:02.00]Two",
            "format": "lrc"
        });
        let lyrics = payload(&data).unwrap();
        assert!(lyrics.synced);
        assert!(!lyrics.word_synced());
    }

    #[test]
    fn a_plain_answer_has_no_timing() {
        let data = serde_json::json!({"lyrics": "One\nTwo", "format": "plain"});
        let lyrics = payload(&data).unwrap();
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
    }

    #[test]
    fn matching_is_tolerant_of_a_remaster_suffix() {
        let entry = serde_json::json!({"song": "Song (Remastered)", "artist": "Artist"});
        assert!(matches(&entry, &query()));
        let other = serde_json::json!({"song": "Song", "artist": "Someone"});
        assert!(!matches(&other, &query()));
    }
}
