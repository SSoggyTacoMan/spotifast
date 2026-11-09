//! Word-synced lyrics from the [Spicy Lyrics API](https://developers.spicylyrics.org).
//!
//! The API returns the best sync it has for a Spotify track: a community
//! syllable sync, an Apple Music word sync, or Spotify's line sync. Spotifast
//! prefers it when it answers because it is the only source with timing inside
//! a line.
//!
//! The key is read from `SPICY_LYRICS_KEY`; without one the request is skipped
//! and the existing Spotify and LRCLIB paths answer instead. Responses are
//! cached for 30 days, as the API's terms require. Community syncs carry an
//! uploader and maker, which the lyrics view credits next to the provider.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    Attribution, CACHE_LIFETIME, Credit, Line, Lyrics, Syllable, cached, older_than,
    previously_found, store,
};

/// The public lyrics endpoint. A track id, ISRC, or Apple Music id goes last.
const API: &str = "https://api.spicylyrics.org/v1/lyrics";
const PROVIDER: &str = "Spicy Lyrics";
const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);
/// The environment variable that holds the developer key.
pub const KEY_ENV: &str = "SPICY_LYRICS_KEY";
/// The name the API's own docs use. Either variable works.
const DOCS_KEY_ENV: &str = "SPICY_LYRICS_SECRET_KEY";

/// The heading shown above the credits at the end of the lyrics.
pub fn attribution_title(attribution: &Attribution, locale: crate::i18n::Locale) -> String {
    use crate::i18n::gettext;
    if attribution.source.is_empty() || attribution.source == "spicy_lyrics" {
        gettext(locale, "Lyrics from {provider}").replace("{provider}", PROVIDER)
    } else {
        gettext(locale, "Lyrics from {provider} via {source}")
            .replace("{provider}", PROVIDER)
            .replace("{source}", &source_label(&attribution.source))
    }
}

/// A source id as something a person reads.
fn source_label(source: &str) -> String {
    match source {
        "apple_music" => "Apple Music".to_string(),
        "spotify" => "Spotify".to_string(),
        "spicy_lyrics" => PROVIDER.to_string(),
        other => {
            let mut label = other.replace('_', " ");
            if let Some(first) = label.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            label
        }
    }
}

fn api_key() -> Option<String> {
    [KEY_ENV, DOCS_KEY_ENV].into_iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    })
}

/// Fetches the track's word-synced lyrics, or `None` when no key is set, the
/// track has none, or the key is not accepted. Those are all reasons to let
/// the next source answer.
pub async fn fetch_spicy(
    http: &reqwest::Client,
    cache_dir: &Path,
    track_id: &str,
) -> Result<Option<Lyrics>> {
    let Some(key) = api_key() else {
        return Ok(None);
    };
    fetch_from(http, API, &key, cache_dir, track_id).await
}

async fn fetch_from(
    http: &reqwest::Client,
    api: &str,
    key: &str,
    cache_dir: &Path,
    track_id: &str,
) -> Result<Option<Lyrics>> {
    let id: String = track_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if id.is_empty() {
        return Ok(None);
    }
    let path = cache_dir.join(format!("spicy-{id}.json"));
    if let Some(cached) = cached(&path) {
        return Ok(cached);
    }
    let result = tokio::time::timeout(TIMEOUT, request(http, api, key, &id))
        .await
        .context("Spicy Lyrics did not respond in time")
        .and_then(|result| result);
    match result {
        Ok(Answer::Found(found)) => {
            store(&path, &Some(found.clone()));
            Ok(Some(found))
        }
        // The track truly has none; remember that for a while, as the LRCLIB
        // path does.
        Ok(Answer::Missing) => {
            store(&path, &None);
            Ok(None)
        }
        // Static words have no timing; Spotify or LRCLIB may still have a
        // synced copy, so leave them their turn and cache nothing.
        Ok(Answer::Unsynced) => Ok(None),
        // A missing, wrong, or forbidden key is not the listener's problem,
        // and must not be cached as "this track has no lyrics".
        Ok(Answer::NotAllowed) => Ok(None),
        // An outage must not erase words the person could already read, but
        // the API's 30-day storage cap is hard: a stale copy stops being
        // served even when the API cannot replace it.
        Err(error) => {
            if older_than(&path, CACHE_LIFETIME) {
                return Err(error);
            }
            previously_found(&path).map(Some).ok_or(error)
        }
    }
}

/// What one request settled.
enum Answer {
    Found(Lyrics),
    /// The API has words for the track, but no timing.
    Unsynced,
    /// The track has no lyrics in any of the API's sources.
    Missing,
    /// The key was rejected; later tracks may fare better.
    NotAllowed,
}

async fn request(http: &reqwest::Client, api: &str, key: &str, track_id: &str) -> Result<Answer> {
    let url = format!("{api}/{track_id}");
    let mut attempt = 0;
    let response = loop {
        let response = http
            .get(&url)
            .bearer_auth(key)
            .header("Accept", "application/json")
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("cannot reach Spicy Lyrics")?;
        let retry = matches!(response.status().as_u16(), 429 | 502..=504);
        let wait = retry_after(response.headers());
        if !retry || attempt >= 1 || wait > MAX_RETRY_AFTER {
            break response;
        }
        attempt += 1;
        tokio::time::sleep(wait).await;
    };
    match response.status().as_u16() {
        200 => {
            let response: Response = response
                .json()
                .await
                .context("unexpected answer from Spicy Lyrics")?;
            Ok(match response.into_lyrics() {
                Some(lyrics) if lyrics.synced => Answer::Found(lyrics),
                Some(_) => Answer::Unsynced,
                None => Answer::Missing,
            })
        }
        // Nobody transcribed the track.
        404 => Ok(Answer::Missing),
        // Spotify and LRCLIB still get their turn.
        401 | 403 => Ok(Answer::NotAllowed),
        status => Err(anyhow::anyhow!("Spicy Lyrics answered {status}")),
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_secs)
}

// ---- response ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(rename = "Body")]
    body: Body,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Body {
    #[serde(rename = "Type", default)]
    kind: String,
    #[serde(rename = "Content", default)]
    content: Vec<serde_json::Value>,
    #[serde(rename = "Lines", default)]
    lines: Vec<RawStaticLine>,
    #[serde(rename = "SongWriters", default)]
    song_writers: Vec<String>,
    #[serde(rename = "UploadAttribution", default)]
    upload_attribution: Option<RawAttribution>,
    /// The API spells this one in lowercase, unlike its other fields.
    #[serde(rename = "source", default)]
    source: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawAttribution {
    uploader: Option<RawCredit>,
    maker: Option<RawCredit>,
}

/// A contributor. Unlike the rest of the response, these fields are
/// lowercase in the API, `username` and `url` included.
#[derive(Clone, Debug, Deserialize)]
struct RawCredit {
    #[serde(default)]
    username: String,
    #[serde(default)]
    url: Option<String>,
}

impl RawCredit {
    fn into_credit(self) -> Option<Credit> {
        let name = self.username.trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Credit {
            name,
            url: self.url.filter(|url| !url.trim().is_empty()),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawSyllableLine {
    #[serde(default)]
    lead: RawGroup,
    #[serde(default)]
    background: Vec<RawGroup>,
    #[serde(default)]
    opposite_aligned: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawGroup {
    #[serde(default)]
    start_time: f64,
    #[serde(default)]
    end_time: f64,
    #[serde(default)]
    syllables: Vec<RawSyllable>,
    /// A translation of the whole group. Syllable-level translations do not
    /// exist, so this is the one the line shows.
    #[serde(default)]
    translated_text: Option<String>,
    /// A romanisation of the whole group, used when no syllable carries one.
    #[serde(default)]
    transliterated_text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawSyllable {
    #[serde(default)]
    text: String,
    #[serde(default)]
    start_time: f64,
    #[serde(default)]
    end_time: f64,
    /// The next syllable continues this word.
    #[serde(default)]
    is_part_of_word: bool,
    #[serde(default)]
    transliterated_text: Option<String>,
    /// The same join marker for the romanisation, which can split words where
    /// the text does not. Falls back to `is_part_of_word`.
    #[serde(default)]
    transliterated_is_part_of_word: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawLine {
    #[serde(default)]
    text: String,
    #[serde(default)]
    start_time: f64,
    #[serde(default)]
    end_time: f64,
    #[serde(default)]
    transliterated_text: Option<String>,
    #[serde(default)]
    translated_text: Option<String>,
    #[serde(default)]
    opposite_aligned: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawStaticLine {
    #[serde(default)]
    text: String,
    #[serde(default)]
    transliterated_text: Option<String>,
    #[serde(default)]
    translated_text: Option<String>,
}

impl Response {
    fn into_lyrics(self) -> Option<Lyrics> {
        let body = self.body;
        let attribution = body.attribution();
        let writers = body
            .song_writers
            .iter()
            .map(|writer| writer.trim().to_string())
            .filter(|writer| !writer.is_empty())
            .collect();
        let mut lyrics = match body.kind.as_str() {
            "Syllable" => body.syllable_lyrics(),
            "Line" => body.line_lyrics(),
            "Static" => body.static_lyrics(),
            _ => None,
        }?;
        lyrics.attribution = attribution;
        lyrics.song_writers = writers;
        Some(lyrics)
    }
}

impl Body {
    fn attribution(&self) -> Option<Attribution> {
        let source = self.source.trim().to_ascii_lowercase();
        let uploader = self
            .upload_attribution
            .as_ref()
            .and_then(|attribution| attribution.uploader.as_ref())
            .and_then(|credit| credit.clone().into_credit());
        let maker = self
            .upload_attribution
            .as_ref()
            .and_then(|attribution| attribution.maker.as_ref())
            .and_then(|credit| credit.clone().into_credit());
        if source.is_empty() && uploader.is_none() && maker.is_none() {
            return None;
        }
        Some(Attribution {
            source: if source.is_empty() {
                "spicy_lyrics".to_string()
            } else {
                source
            },
            uploader,
            maker,
        })
    }

    fn syllable_lyrics(&self) -> Option<Lyrics> {
        let mut lines = Vec::new();
        for value in &self.content {
            let Ok(raw) = serde_json::from_value::<RawSyllableLine>(value.clone()) else {
                continue;
            };
            let Some(line) = raw.into_line() else {
                continue;
            };
            lines.push(line);
        }
        if lines.is_empty() {
            return None;
        }
        lines.sort_by_key(|line| line.at_ms);
        Some(Lyrics {
            lines,
            synced: true,
            instrumental: false,
            ..Lyrics::default()
        })
    }

    fn line_lyrics(&self) -> Option<Lyrics> {
        let mut lines = Vec::new();
        for value in &self.content {
            let Ok(raw) = serde_json::from_value::<RawLine>(value.clone()) else {
                continue;
            };
            let text = raw.text.trim().to_string();
            let at = seconds_to_ms(raw.start_time);
            let end = seconds_to_ms(raw.end_time);
            lines.push(Line {
                at_ms: Some(at),
                text,
                end_ms: Some(end),
                transliterated: clean(raw.transliterated_text),
                translated: clean(raw.translated_text),
                opposite_aligned: raw.opposite_aligned,
                ..Line::default()
            });
        }
        if lines.is_empty() {
            return None;
        }
        lines.sort_by_key(|line| line.at_ms);
        Some(Lyrics {
            lines,
            synced: true,
            instrumental: false,
            ..Lyrics::default()
        })
    }

    fn static_lyrics(&self) -> Option<Lyrics> {
        let lines: Vec<Line> = self
            .lines
            .iter()
            .map(|raw| Line {
                transliterated: clean(raw.transliterated_text.clone()),
                translated: clean(raw.translated_text.clone()),
                ..Line::new(None, raw.text.trim())
            })
            .filter(|line| !line.text.is_empty() || line.translated.is_some())
            .collect();
        if lines.is_empty() {
            return None;
        }
        Some(Lyrics {
            lines,
            synced: false,
            instrumental: false,
            ..Lyrics::default()
        })
    }
}

impl RawSyllableLine {
    fn into_line(self) -> Option<Line> {
        let lead_start = self.lead.start_time;
        let lead_end = self.lead.end_time;
        let lead_transliterated = clean(self.lead.transliterated_text.clone());
        let lead_translated = clean(self.lead.translated_text.clone());
        let syllables = self.lead.syllables_into();
        let mut background = Vec::new();
        for group in self.background {
            background.extend(group.syllables_into());
        }
        if syllables.is_empty() && background.is_empty() {
            // A timed gap between sung lines; keep the time, not the words.
            let at = seconds_to_ms(lead_start);
            let end = seconds_to_ms(lead_end);
            return Some(Line {
                at_ms: Some(at),
                end_ms: Some(end.max(at)),
                opposite_aligned: self.opposite_aligned,
                ..Line::default()
            });
        }
        // A line of backing vocals alone still reads as words.
        let text = if syllables.is_empty() {
            join_words(&background)
        } else {
            join_words(&syllables)
        };
        // Prefer the syllable-level romanisation, as the API asks, and fall
        // back to the whole group's when no syllable carries one.
        let transliterated = join_optional(
            &syllables,
            |syllable| syllable.transliterated.as_deref(),
            |syllable| {
                syllable
                    .transliterated_part_of_word
                    .unwrap_or(syllable.part_of_word)
            },
        )
        .or(lead_transliterated);
        let at = if lead_start > 0.0 {
            seconds_to_ms(lead_start)
        } else {
            syllables.first().map_or(0, |syllable| syllable.at_ms)
        };
        let end = if lead_end > 0.0 {
            seconds_to_ms(lead_end)
        } else {
            syllables
                .iter()
                .map(|syllable| syllable.end_ms)
                .max()
                .unwrap_or(at)
        };
        Some(Line {
            at_ms: Some(at),
            text,
            end_ms: Some(end.max(at)),
            syllables,
            background,
            opposite_aligned: self.opposite_aligned,
            transliterated,
            translated: lead_translated,
        })
    }
}

impl RawGroup {
    fn syllables_into(self) -> Vec<Syllable> {
        let mut out = Vec::new();
        for raw in self.syllables {
            let text = raw.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let at = seconds_to_ms(raw.start_time);
            let mut end = seconds_to_ms(raw.end_time);
            if end <= at {
                end = at.saturating_add(80);
            }
            out.push(Syllable {
                at_ms: at,
                end_ms: end,
                text,
                transliterated: clean(raw.transliterated_text),
                part_of_word: raw.is_part_of_word,
                transliterated_part_of_word: raw.transliterated_is_part_of_word,
            });
        }
        out
    }
}

fn clean(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The line as a person reads it: a space between words, none inside one.
///
/// `IsPartOfWord` marks the join to the next syllable, so the space goes
/// before a syllable when the one before it did not join.
fn join_words(syllables: &[Syllable]) -> String {
    let mut text = String::new();
    for (index, syllable) in syllables.iter().enumerate() {
        if index > 0 && !syllables[index - 1].part_of_word {
            text.push(' ');
        }
        text.push_str(&syllable.text);
    }
    text
}

fn join_optional(
    syllables: &[Syllable],
    field: impl Fn(&Syllable) -> Option<&str>,
    joins_next: impl Fn(&Syllable) -> bool,
) -> Option<String> {
    let mut text = String::new();
    let mut found = false;
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
        found = true;
    }
    found.then_some(text).filter(|text| !text.trim().is_empty())
}

fn seconds_to_ms(seconds: f64) -> u32 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * 1000.0).round().min(f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: &str) -> Response {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn syllable_responses_become_words_with_credit() {
        let response = body(
            r#"{
              "Body": {
                "Type": "Syllable",
                "SongWriters": ["Ada Writer"],
                "StartTime": 1.0,
                "EndTime": 9.0,
                "source": "spicy_lyrics",
                "UploadAttribution": {
                  "Uploader": {"username": "up", "url": "https://example.com/up"},
                  "Maker": {"username": "maker", "url": "https://example.com/maker"}
                },
                "Content": [
                  {"Lead": {"StartTime": 1.0, "EndTime": 2.0,
                    "TranslatedText": "Bonjour le monde",
                    "Syllables": [
                    {"Text": "Hel", "StartTime": 1.0, "EndTime": 1.4, "IsPartOfWord": true},
                    {"Text": "lo", "StartTime": 1.4, "EndTime": 1.8, "IsPartOfWord": false},
                    {"Text": "world", "StartTime": 1.8, "EndTime": 2.0, "IsPartOfWord": false}
                  ]}},
                  {"Lead": {"StartTime": 2.5, "EndTime": 3.0, "Syllables": []},
                   "OppositeAligned": true}
                ]
              },
              "Status": 200,
              "Type": "object"
            }"#,
        );
        let lyrics = response.into_lyrics().unwrap();
        assert!(lyrics.word_synced());
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "Hello world");
        assert_eq!(lyrics.lines[0].at_ms, Some(1_000));
        assert_eq!(lyrics.lines[0].end_ms, Some(2_000));
        assert_eq!(lyrics.lines[0].syllables.len(), 3);
        assert!(lyrics.lines[0].syllables[0].part_of_word);
        assert!(!lyrics.lines[0].syllables[1].part_of_word);
        assert_eq!(lyrics.lines[0].syllables[2].at_ms, 1_800);
        assert_eq!(
            lyrics.lines[0].translated.as_deref(),
            Some("Bonjour le monde")
        );
        assert_eq!(lyrics.lines[1].text, "");
        assert!(lyrics.lines[1].opposite_aligned);
        assert_eq!(lyrics.song_writers, ["Ada Writer"]);
        let attribution = lyrics.attribution.unwrap();
        assert_eq!(attribution.source, "spicy_lyrics");
        assert_eq!(attribution.uploader.unwrap().name, "up");
        assert_eq!(attribution.maker.unwrap().name, "maker");
    }

    #[test]
    fn a_word_split_into_three_keeps_no_spaces_inside_it() {
        let response = body(
            r#"{
              "Body": {
                "Type": "Syllable",
                "source": "apple_music",
                "Content": [
                  {"Lead": {"StartTime": 1.0, "EndTime": 2.0, "Syllables": [
                    {"Text": "He", "StartTime": 1.0, "EndTime": 1.2, "IsPartOfWord": true,
                     "TransliteratedText": "He", "TransliteratedIsPartOfWord": true},
                    {"Text": "ll", "StartTime": 1.2, "EndTime": 1.4, "IsPartOfWord": true,
                     "TransliteratedText": "ll", "TransliteratedIsPartOfWord": true},
                    {"Text": "o", "StartTime": 1.4, "EndTime": 1.6, "IsPartOfWord": false,
                     "TransliteratedText": "o", "TransliteratedIsPartOfWord": false},
                    {"Text": "there", "StartTime": 1.6, "EndTime": 2.0, "IsPartOfWord": false,
                     "TransliteratedText": "there", "TransliteratedIsPartOfWord": false}
                  ]}}
                ]
              }
            }"#,
        );
        let lyrics = response.into_lyrics().unwrap();
        let line = &lyrics.lines[0];
        assert_eq!(line.text, "Hello there");
        assert_eq!(line.transliterated.as_deref(), Some("Hello there"));
    }

    #[test]
    fn line_responses_stay_whole_and_have_no_community_credit() {
        let response = body(
            r#"{
              "Body": {
                "Type": "Line",
                "source": "apple_music",
                "Content": [
                  {"Text": "One line", "StartTime": 1.25, "EndTime": 3.5},
                  {"Text": "Two line", "StartTime": 3.5, "EndTime": 5.0,
                   "TranslatedText": "Deux lignes"}
                ]
              }
            }"#,
        );
        let lyrics = response.into_lyrics().unwrap();
        assert!(lyrics.synced);
        assert!(!lyrics.word_synced());
        assert_eq!(lyrics.lines[0].at_ms, Some(1_250));
        assert_eq!(lyrics.lines[1].translated.as_deref(), Some("Deux lignes"));
        let attribution = lyrics.attribution.unwrap();
        assert_eq!(attribution.source, "apple_music");
        assert!(attribution.uploader.is_none());
    }

    #[test]
    fn static_responses_have_no_timing() {
        let response = body(
            r#"{
              "Body": {
                "Type": "Static",
                "Lines": [{"Text": "A"}, {"Text": ""}, {"Text": "B"}]
              }
            }"#,
        );
        let lyrics = response.into_lyrics().unwrap();
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text, "B");
    }

    #[test]
    fn a_syllable_is_bright_only_while_it_sounds() {
        let syllable = Syllable {
            at_ms: 1_000,
            end_ms: 2_000,
            text: "a".into(),
            ..Syllable::default()
        };
        assert_eq!(syllable.progress(500), 0.0);
        assert_eq!(syllable.progress(1_500), 0.5);
        assert_eq!(syllable.progress(2_500), 1.0);
        let instant = Syllable {
            at_ms: 1_000,
            end_ms: 1_000,
            ..syllable
        };
        assert_eq!(instant.progress(1_100), 1.0);
    }

    #[test]
    fn times_convert_from_seconds_without_losing_milliseconds() {
        assert_eq!(seconds_to_ms(0.0), 0);
        assert_eq!(seconds_to_ms(7.357), 7_357);
        assert_eq!(seconds_to_ms(-1.0), 0);
        assert_eq!(seconds_to_ms(f64::NAN), 0);
    }

    /// A local server that answers one request and records its headers.
    async fn spicy(answer: String) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let len = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..len]).to_string();
            socket.write_all(answer.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn the_request_bears_the_key_and_answers_become_lyrics() {
        let body = r#"{"Body":{"Type":"Line","source":"spotify","Content":[{"Text":"A","StartTime":1.0,"EndTime":2.0}]}}"#;
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (api, server) = spicy(answer).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root = std::env::temp_dir().join(format!("spotifast-spicy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lyrics = fetch_from(&http, &api, "sl_sk_test", &root, "track1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lyrics.lines[0].text, "A");
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /track1 "), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer sl_sk_test"),
            "{request}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_missing_track_is_none_and_is_cached_as_such() {
        let answer = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (api, server) = spicy(answer.to_string()).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root = std::env::temp_dir().join(format!("spotifast-spicy-404-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            fetch_from(&http, &api, "key", &root, "missing")
                .await
                .unwrap()
                .is_none()
        );
        assert!(root.join("spicy-missing.json").exists(), "miss is cached");
        server.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn static_words_leave_room_for_a_timed_source() {
        let body = r#"{"Body":{"Type":"Static","Lines":[{"Text":"A"}]}}"#;
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (api, server) = spicy(answer).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root =
            std::env::temp_dir().join(format!("spotifast-spicy-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            fetch_from(&http, &api, "key", &root, "track1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !root.join("spicy-track1.json").exists(),
            "untimed words are not a Spicy answer to cache"
        );
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_rejected_key_is_not_cached_as_a_missing_track() {
        let answer = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (api, server) = spicy(answer.to_string()).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let root = std::env::temp_dir().join(format!("spotifast-spicy-401-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            fetch_from(&http, &api, "wrong", &root, "track1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !root.join("spicy-track1.json").exists(),
            "a rejected key must not poison the cache"
        );
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_cached_answer_stops_at_the_thirty_day_storage_cap() {
        let root =
            std::env::temp_dir().join(format!("spotifast-spicy-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("spicy-stale.json");
        let fresh = body(
            r#"{"Body":{"Type":"Line","source":"spotify","Content":[{"Text":"A","StartTime":1.0,"EndTime":2.0}]}}"#,
        )
        .into_lyrics()
        .unwrap();
        store(&path, &Some(fresh));
        let http = reqwest::Client::builder().no_proxy().build().unwrap();

        // A fresh copy bridges an outage without a request.
        let cached = fetch_from(&http, "http://127.0.0.1:1", "key", &root, "stale")
            .await
            .unwrap();
        assert_eq!(cached.unwrap().lines[0].text, "A");

        let file = std::fs::File::options().write(true).open(&path).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(31 * 24 * 60 * 60);
        file.set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();

        let answer =
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (api, server) = spicy(answer.to_string()).await;
        assert!(
            fetch_from(&http, &api, "key", &root, "stale")
                .await
                .is_err(),
            "a copy past the cap must not be served"
        );
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_api_spells_source_in_lowercase() {
        let response = body(r#"{"Body":{"Type":"Line","source":"spotify","Content":[]}}"#);
        assert_eq!(response.body.source, "spotify");
    }

    #[test]
    fn attribution_names_the_source_it_came_from() {
        let community = Attribution {
            source: "spicy_lyrics".into(),
            ..Attribution::default()
        };
        assert_eq!(
            attribution_title(&community, crate::i18n::Locale::English),
            "Lyrics from Spicy Lyrics"
        );
        let apple = Attribution {
            source: "apple_music".into(),
            ..Attribution::default()
        };
        assert_eq!(
            attribution_title(&apple, crate::i18n::Locale::English),
            "Lyrics from Spicy Lyrics via Apple Music"
        );
    }
}
