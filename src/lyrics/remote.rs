//! Shared plumbing for the remote lyric providers.
//!
//! AMLL, Unison, lrc.red and NetEase answer JSON over HTTP and can miss,
//! fail, or serve words without timing. Each provider uses these helpers so
//! that a miss is quiet, an outage keeps the last good answer, and a track
//! that is merely a different recording is not mistaken for the one playing.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Lyrics, cached, previously_found, store};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(8);

/// A JSON GET. Missing tracks and bad questions are a plain miss; anything
/// else is an error the cascade may skip.
pub(crate) async fn get_json(
    http: &reqwest::Client,
    url: &str,
    params: &[(&str, &str)],
    headers: &[(&str, &str)],
) -> Result<Option<Value>> {
    let mut request = http.get(url).query(params).timeout(TIMEOUT);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("cannot reach {url}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::BAD_REQUEST {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(anyhow::anyhow!("{url} answered {status}"));
    }
    Ok(Some(response.json().await.with_context(|| {
        format!("unexpected answer from {url}")
    })?))
}

/// The provider's answer, from its 30-day cache when fresh. An outage serves
/// the last good answer rather than erasing words the listener could read.
pub(crate) async fn with_cache(
    cache_dir: &Path,
    key: &str,
    call: impl std::future::Future<Output = Result<Option<Lyrics>>>,
) -> Result<Option<Lyrics>> {
    let path = cache_dir.join(format!("{key}.json"));
    if let Some(cached) = cached(&path) {
        return Ok(cached);
    }
    let result = tokio::time::timeout(TIMEOUT, call)
        .await
        .context("lyrics service did not respond in time")
        .and_then(|result| result);
    match result {
        Ok(found) => {
            store(&path, &found);
            Ok(found)
        }
        Err(error) => previously_found(&path).map(Some).ok_or(error),
    }
}

/// Whether a provider's title and artist describe the track that is playing.
pub(crate) fn same_track(title: &str, artist: &str, query: &super::Query) -> bool {
    super::loose_match(title, &super::clean_title(&query.title))
        && (super::loose_match(artist, &super::clean_artist(&query.artist))
            || super::loose_match(artist, &query.artist))
}

/// A candidate this far from the playing track's length is another recording.
const MAX_DRIFT_SECS: f64 = 30.0;

/// `candidate_ms` of zero means the provider did not say, which is not a
/// reason to refuse it.
pub(crate) fn duration_matches(candidate_ms: u64, query: &super::Query) -> bool {
    if candidate_ms == 0 || query.duration_ms == 0 {
        return true;
    }
    let drift = (candidate_ms as f64 - f64::from(query.duration_ms)).abs() / 1000.0;
    drift <= MAX_DRIFT_SECS
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

pub(crate) fn string_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(text).map(ToString::to_string)
}

pub(crate) fn number_at(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

pub(crate) fn strings_at(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim))
                .filter(|item| !item.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A TTML document in one of the fields the providers use for it.
pub(crate) fn ttml_in(data: &Value) -> Option<String> {
    ["lyrics", "ttml", "content", "source"]
        .into_iter()
        .filter_map(|key| data.get(key))
        .filter_map(Value::as_str)
        .find(|text| text.contains("<tt"))
        .map(ToString::to_string)
}

/// The credit lines these catalogues write into the lyrics themselves:
/// "作词 : ...", "Produced by: ...", "Lyrics: ...".
pub(crate) fn is_credit(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with('~') && lower.trim_matches('~').trim() == "end" {
        return true;
    }
    let Some(colon) = trimmed.find([':', '：']) else {
        return false;
    };
    let label = trimmed[..colon].trim().to_lowercase();
    if label.is_empty() {
        return false;
    }
    const LABELS: &[&str] = &[
        "lyric",
        "lyrics",
        "lyricist",
        "composed",
        "composer",
        "composition",
        "written",
        "writer",
        "music",
        "arranged",
        "arranger",
        "arrangement",
        "produced",
        "producer",
        "production",
        "mixed",
        "mixing",
        "mastered",
        "mastering",
        "recorded",
        "recording",
        "vocal",
        "vocals",
        "performed",
        "engineer",
        "engineered",
        "backing vocal",
        "backing vocals",
        "guitar",
        "guitars",
        "bass",
        "drum",
        "drums",
        "keyboard",
        "keyboards",
        "string",
        "strings",
        "piano",
        "talkbox",
        "rap",
        "作词",
        "作曲",
        "编曲",
        "制作",
        "出品",
        "监制",
        "录音",
        "混音",
        "母带",
        "吉他",
        "贝斯",
        "鼓",
        "键盘",
        "和声",
        "弦乐",
        "人声",
        "策划",
        "统筹",
        "发行",
        "工程师",
        "演奏",
    ];
    LABELS.iter().any(|name| label.ends_with(name))
        || (label.ends_with(" by") && LABELS.iter().any(|name| label.contains(name)))
}

/// Whether the lines only say the track has no words, as NetEase's
/// "纯音乐，请欣赏" does.
pub(crate) fn is_no_words_note(lines: &[String]) -> bool {
    let words: Vec<&str> = lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !is_credit(line))
        .collect();
    if !(1..=3).contains(&words.len()) {
        return false;
    }
    let joined = words.join(" ").to_lowercase();
    [
        "纯音乐",
        "純音樂",
        "请欣赏",
        "請欣賞",
        "无歌词",
        "无填词",
        "沒有填詞",
        "暂无歌词",
        "暫無歌詞",
        "instrumental",
        "no lyrics",
    ]
    .iter()
    .any(|note| joined.contains(note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::Query;

    fn query() -> Query {
        Query {
            artist: "Artist".into(),
            title: "Song".into(),
            album: String::new(),
            duration_ms: 200_000,
        }
    }

    #[test]
    fn a_track_matches_loosely_but_a_different_length_does_not() {
        assert!(same_track("Song (Live)", "Artist", &query()));
        assert!(same_track("Song", "Artist;Guest", &query()));
        assert!(!same_track("Other", "Artist", &query()));
        assert!(duration_matches(0, &query()));
        assert!(duration_matches(210_000, &query()));
        assert!(!duration_matches(260_000, &query()));
    }

    #[test]
    fn credits_and_instrumental_notes_are_recognised() {
        assert!(is_credit("Lyrics: Someone"));
        assert!(is_credit("Produced by: Someone"));
        assert!(is_credit("作词 : Someone"));
        assert!(is_credit("~~~END~~~"));
        assert!(!is_credit("I wrote a letter"));
        assert!(is_no_words_note(&["纯音乐，请欣赏".into()]));
        assert!(is_no_words_note(&["Instrumental".into()]));
        assert!(!is_no_words_note(&[
            "A line".into(),
            "Another".into(),
            "Third".into(),
            "Fourth".into()
        ]));
    }
}
