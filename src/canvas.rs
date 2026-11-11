//! Spotify Canvas lookup and caching via the Spicy Lyrics Canvas service.
//!
//! Canvas is a short looping portrait video for a track. Looking up a canvas
//! is best-effort and failures fall back cleanly to static album artwork.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const BASE_URL: &str = "https://regional.canvasstore.slnode.spicylyrics.org";
/// How long a negative result (track has no canvas) is remembered before retrying.
pub const MISS_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// Initial pause after a network or service error.
pub const FAILURE_PAUSE_MS: u64 = 60 * 1000;
/// Maximum backoff pause after repeated failures.
pub const MAX_FAILURE_PAUSE_MS: u64 = 30 * 60 * 1000;
/// Delay before retrying a failed attempt.
pub const RETRY_DELAY_MS: u64 = 1500;

/// Spotify track IDs are 22 base-62 ASCII characters.
pub fn is_track_id(id: &str) -> bool {
    id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_video_url(url: &str) -> bool {
    url.starts_with("https://")
        && url
            .split('?')
            .next()
            .is_some_and(|clean| clean.to_ascii_lowercase().ends_with(".mp4"))
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Found(String),
    None,
}

/// Parses the service's JSON response for a track ID.
pub fn parse_canvas_response(json: &str, track_id: &str) -> Option<Answer> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = root.get("data")?.as_object()?;
    let entry = data.get(track_id)?;
    if entry.is_null() {
        return Some(Answer::None);
    }
    let obj = entry.as_object()?;
    if obj.contains_key("error") {
        return None;
    }
    if let Some(url) = obj.get("url").and_then(|v| v.as_str())
        && is_video_url(url)
    {
        return Some(Answer::Found(url.to_string()));
    }
    Some(Answer::None)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MemoEntry {
    url: String,
    at: u64,
}

pub struct SpotifyCanvasFinder {
    client: reqwest::Client,
    memo_file: Option<PathBuf>,
    memo: Mutex<Option<HashMap<String, MemoEntry>>>,
    failed_at: Mutex<u64>,
    pause_ms: Mutex<u64>,
}

impl SpotifyCanvasFinder {
    pub fn new(client: reqwest::Client, memo_file: Option<PathBuf>) -> Self {
        Self {
            client,
            memo_file,
            memo: Mutex::new(None),
            failed_at: Mutex::new(0),
            pause_ms: Mutex::new(0),
        }
    }

    fn load_memo(&self) -> HashMap<String, MemoEntry> {
        let mut guard = self.memo.lock().unwrap();
        if let Some(ref map) = *guard {
            return map.clone();
        }
        let mut loaded = HashMap::new();
        if let Some(ref path) = self.memo_file
            && let Ok(text) = std::fs::read_to_string(path)
            && let Ok(map) = serde_json::from_str::<HashMap<String, MemoEntry>>(&text)
        {
            loaded = map;
        }
        *guard = Some(loaded.clone());
        loaded
    }

    fn save_memo(&self, map: &HashMap<String, MemoEntry>) {
        if let Some(ref path) = self.memo_file {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(map) {
                let tmp = path.with_extension("tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(tmp, path);
                }
            }
        }
    }

    /// Finds a track's Spotify Canvas MP4 URL, or `None` if absent or failed.
    pub async fn find(&self, track_id: &str) -> Option<String> {
        if !is_track_id(track_id) {
            return None;
        }

        let now = current_time_ms();
        {
            let memo = self.load_memo();
            if let Some(entry) = memo.get(track_id) {
                if !entry.url.is_empty() {
                    return Some(entry.url.clone());
                }
                if now.saturating_sub(entry.at) < MISS_TTL_MS {
                    return None;
                }
            }
        }

        let failed_at = *self.failed_at.lock().unwrap();
        let pause_ms = *self.pause_ms.lock().unwrap();
        if now.saturating_sub(failed_at) < pause_ms {
            return None;
        }

        let answer = self.ask_twice(track_id).await;
        match answer {
            Some(Answer::Found(url)) => {
                *self.pause_ms.lock().unwrap() = 0;
                let mut guard = self.memo.lock().unwrap();
                let map = guard.get_or_insert_with(HashMap::new);
                map.insert(
                    track_id.to_string(),
                    MemoEntry {
                        url: url.clone(),
                        at: now,
                    },
                );
                self.save_memo(map);
                Some(url)
            }
            Some(Answer::None) => {
                *self.pause_ms.lock().unwrap() = 0;
                let mut guard = self.memo.lock().unwrap();
                let map = guard.get_or_insert_with(HashMap::new);
                map.insert(
                    track_id.to_string(),
                    MemoEntry {
                        url: String::new(),
                        at: now,
                    },
                );
                self.save_memo(map);
                None
            }
            None => {
                let mut pause = self.pause_ms.lock().unwrap();
                *pause = if *pause == 0 {
                    FAILURE_PAUSE_MS
                } else {
                    (*pause * 2).clamp(FAILURE_PAUSE_MS, MAX_FAILURE_PAUSE_MS)
                };
                *self.failed_at.lock().unwrap() = now;
                None
            }
        }
    }

    /// Drops a cached canvas URL so it is re-queried next time.
    pub fn forget(&self, track_id: &str) {
        let mut guard = self.memo.lock().unwrap();
        if let Some(ref mut map) = *guard
            && map.remove(track_id).is_some()
        {
            self.save_memo(map);
        }
    }

    async fn ask_twice(&self, track_id: &str) -> Option<Answer> {
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
            }
            if let Ok(Some(answer)) = self.lookup(track_id).await {
                return Some(answer);
            }
        }
        None
    }

    async fn lookup(&self, track_id: &str) -> Result<Option<Answer>> {
        let url = format!("{BASE_URL}/provider/canvaslist/{track_id}");
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Ok(Some(Answer::None));
        }
        if !response.status().is_success() {
            return Ok(None);
        }

        let body = response.text().await?;
        Ok(parse_canvas_response(&body, track_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_id_validation() {
        assert!(is_track_id("4cOdK2wGLETKBW3PvgPWqT"));
        assert!(!is_track_id("short"));
        assert!(!is_track_id("4cOdK2wGLETKBW3PvgPWqT_too_long"));
        assert!(!is_track_id("4cOdK2wGLETKBW3PvgPWq-"));
    }

    #[test]
    fn parses_canvas_found() {
        let json = r#"{
            "data": {
                "4cOdK2wGLETKBW3PvgPWqT": {
                    "url": "https://canvas.scdn.co/video.mp4?sig=123",
                    "type": "video"
                }
            }
        }"#;
        assert_eq!(
            parse_canvas_response(json, "4cOdK2wGLETKBW3PvgPWqT"),
            Some(Answer::Found(
                "https://canvas.scdn.co/video.mp4?sig=123".into()
            ))
        );
    }

    #[test]
    fn parses_canvas_null_miss() {
        let json = r#"{
            "data": {
                "4cOdK2wGLETKBW3PvgPWqT": null
            }
        }"#;
        assert_eq!(
            parse_canvas_response(json, "4cOdK2wGLETKBW3PvgPWqT"),
            Some(Answer::None)
        );
    }

    #[test]
    fn parses_canvas_error_as_failure() {
        let json = r#"{
            "data": {
                "4cOdK2wGLETKBW3PvgPWqT": {
                    "error": "Not available"
                }
            }
        }"#;
        assert_eq!(parse_canvas_response(json, "4cOdK2wGLETKBW3PvgPWqT"), None);
    }

    #[test]
    fn parses_non_video_as_none() {
        let json = r#"{
            "data": {
                "4cOdK2wGLETKBW3PvgPWqT": {
                    "url": "https://canvas.scdn.co/image.jpeg"
                }
            }
        }"#;
        assert_eq!(
            parse_canvas_response(json, "4cOdK2wGLETKBW3PvgPWqT"),
            Some(Answer::None)
        );
    }
}
