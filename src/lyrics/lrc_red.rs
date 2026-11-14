//! Lyrics from [lrc.red](https://lrc.red).
//!
//! lrc.red serves Apple Music's TTML by ISRC, with Apple's `itunes:` names
//! renamed to `lrc:`. Its site search finds the ISRC from title and artist;
//! some songs carry a word sync from lrc.red's own model instead.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::remote::{
    duration_matches, get_json, number_at, same_track, string_at, ttml_in, with_cache,
};
use super::{Attribution, Lyrics, Query, cache_key, ttml};

const BASE: &str = "https://lrc.red";
const SOURCE: &str = "lrc_red";
/// How many matched recordings to try, best first.
const TRIES: usize = 3;

/// lrc.red's lyrics for `query`, or `None` when it has none or cannot be read.
pub async fn fetch_lrc_red(
    http: &reqwest::Client,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    let key = format!("lrc-red-{}", cache_key(query));
    with_cache(cache_dir, &key, request_from(http, BASE, query)).await
}

async fn request_from(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    let Some(root) = get_json(
        http,
        &format!("{base}/search.json"),
        &[("q", &format!("{} {}", query.title, query.artist))],
        &[],
    )
    .await?
    else {
        return Ok(None);
    };
    let hits = root
        .get("hits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut candidates: Vec<(&Value, f64)> = hits
        .iter()
        .filter_map(|hit| {
            let title = string_at(hit, "title")?;
            let artist = string_at(hit, "artist")?;
            if !same_track(&title, &artist, query) {
                return None;
            }
            let candidate_ms = number_at(hit, "duration")
                .map(|seconds| (seconds * 1000.0) as u64)
                .unwrap_or(0);
            if !duration_matches(candidate_ms, query) {
                return None;
            }
            let drift = if candidate_ms == 0 || query.duration_ms == 0 {
                0.0
            } else {
                (candidate_ms as f64 - f64::from(query.duration_ms)).abs()
            };
            Some((hit, drift))
        })
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));

    for (hit, _) in candidates.into_iter().take(TRIES) {
        let Some(isrc) = string_at(hit, "isrc") else {
            continue;
        };
        let Some(song) = get_json(http, &format!("{base}/s/{isrc}.song.json"), &[], &[]).await?
        else {
            continue;
        };
        let Some(lyrics) = payload(&song) else {
            continue;
        };
        return Ok(Some(lyrics));
    }
    Ok(None)
}

/// A `song.json` as lyrics: its TTML with Apple's names put back.
fn payload(song: &Value) -> Option<Lyrics> {
    let document = ttml_in(song)?;
    let mut lyrics = ttml::parse(&apple_ttml(&document))?;
    lyrics.attribution = Some(Attribution {
        source: SOURCE.to_string(),
        uploader: None,
        maker: None,
    });
    Some(lyrics)
}

/// Puts back the names the TTML parser reads: `xmlns:lrc`, `lrc:timing` and
/// friends, and `<sourceMetadata>`.
fn apple_ttml(document: &str) -> String {
    let renamed = document
        .replace("xmlns:lrc=", "xmlns:itunes=")
        .replace("<sourceMetadata", "<iTunesMetadata")
        .replace("</sourceMetadata>", "</iTunesMetadata>");
    let chars: Vec<char> = renamed.chars().collect();
    let mut out = String::with_capacity(renamed.len());
    let mut index = 0;
    while index < chars.len() {
        let spaced = index == 0 || chars[index - 1].is_whitespace();
        let is_prefix = spaced
            && chars.get(index) == Some(&'l')
            && chars.get(index + 1) == Some(&'r')
            && chars.get(index + 2) == Some(&'c')
            && chars.get(index + 3) == Some(&':');
        if is_prefix {
            let mut name = index + 4;
            while chars.get(name).is_some_and(char::is_ascii_alphabetic) {
                name += 1;
            }
            if name > index + 4 && chars.get(name) == Some(&'=') {
                out.push_str("itunes:");
                index += 4;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
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
    fn lrc_names_become_apples_names() {
        let document = concat!(
            r#"<tt xmlns:lrc="http://music.apple.com/lyric-ttml-internal" "#,
            r#"lrc:timing="word"><head><sourceMetadata><songwriters>"#,
            r#"<songwriter>Ada</songwriter></songwriters></sourceMetadata></head><body/></tt>"#
        );
        let renamed = apple_ttml(document);
        assert!(renamed.contains(r#"xmlns:itunes="http://music.apple.com/lyric-ttml-internal""#));
        assert!(renamed.contains(r#"itunes:timing="word""#));
        assert!(renamed.contains("<iTunesMetadata>"));
        assert!(renamed.contains("</iTunesMetadata>"));
        assert!(!renamed.contains("lrc:"));
        assert!(!renamed.contains("sourceMetadata"));
    }

    #[test]
    fn a_song_json_becomes_lyrics() {
        let song = serde_json::json!({
            "source": "<tt xmlns:lrc=\"http://music.apple.com/lyric-ttml-internal\" lrc:timing=\"word\"><body><div><p begin=\"1\" end=\"2\"><span begin=\"1\" end=\"2\">Hello</span></p></div></body></tt>",
            "byModel": false
        });
        let lyrics = payload(&song).unwrap();
        assert_eq!(lyrics.lines[0].text, "Hello");
        assert_eq!(lyrics.attribution.unwrap().source, "lrc_red");
    }

    #[test]
    fn a_remastered_title_still_matches() {
        assert!(same_track("Song (Remastered)", "Artist", &query()));
        assert!(!same_track("Song", "Other", &query()));
    }
}
