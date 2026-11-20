//! Spotify Canvas lookup and caching via the Spicy Lyrics Canvas service.
//!
//! Canvas is a short looping portrait video for a track. Looking up a canvas
//! is best-effort and failures fall back cleanly to static album artwork.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use image::RgbaImage;
use mp4::{Mp4Reader, TrackType};
use openh264::decoder::{DecodeOptions, Decoder, Flush};
use openh264::formats::YUVSource as _;
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

/// A decoded Canvas loop: small RGBA frames in display order and how many
/// play in a second.
pub struct Video {
    pub frames: Vec<RgbaImage>,
    pub fps: f32,
}

/// A decoded Canvas loop, already blurred into the square 128×128 images the
/// Kawarp shader samples. Decoding and blurring happen off the UI thread.
pub struct BlurredVideo {
    pub frames: Vec<egui::ColorImage>,
    pub fps: f32,
}

/// The 128×128 blurred cover Kawarp warps, from an image: a bilinear
/// downsample, then `passes` Kawase passes (taps at ±(i + 0.5) texels), the
/// separable form of `KawarpBlurCore.processFast`.
pub fn blur_frame(image: &image::RgbaImage, passes: usize) -> egui::ColorImage {
    const N: usize = 128;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let raw = image.as_raw();
    let sample = |x: f32, y: f32, c: usize| -> f32 {
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let clampx = |v: f32| (v as isize).clamp(0, w as isize - 1) as usize;
        let clampy = |v: f32| (v as isize).clamp(0, h as isize - 1) as usize;
        let (xa, xb, ya, yb) = (clampx(x0), clampx(x0 + 1.0), clampy(y0), clampy(y0 + 1.0));
        let at = |x: usize, y: usize| f32::from(raw[(y * w + x) * 4 + c]) / 255.0;
        let top = at(xa, ya) + (at(xb, ya) - at(xa, ya)) * fx;
        let bottom = at(xa, yb) + (at(xb, yb) - at(xa, yb)) * fx;
        top + (bottom - top) * fy
    };
    let mut read = vec![0.0_f32; N * N * 4];
    for y in 0..N {
        let v = (y as f32 + 0.5) / N as f32;
        for x in 0..N {
            let u = (x as f32 + 0.5) / N as f32;
            for c in 0..4 {
                read[(y * N + x) * 4 + c] = sample(u * w as f32 - 0.5, v * h as f32 - 0.5, c);
            }
        }
    }
    let mut temp = vec![0.0_f32; N * N * 4];
    let last = N as isize - 1;
    let at = |v: isize| v.clamp(0, last) as usize;
    for i in 0..passes.clamp(1, 40) as isize {
        for y in 0..N {
            for x in 0..N as isize {
                let taps = [at(x - i - 1), at(x - i), at(x + i), at(x + i + 1)];
                for c in 0..4 {
                    temp[(y * N + x as usize) * 4 + c] =
                        taps.iter().map(|t| read[(y * N + t) * 4 + c]).sum::<f32>() * 0.25;
                }
            }
        }
        for y in 0..N as isize {
            let taps = [at(y - i - 1), at(y - i), at(y + i), at(y + i + 1)];
            for x in 0..N {
                for c in 0..4 {
                    read[(y as usize * N + x) * 4 + c] =
                        taps.iter().map(|t| temp[(t * N + x) * 4 + c]).sum::<f32>() * 0.25;
                }
            }
        }
    }
    let bytes: Vec<u8> = read
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([N, N], &bytes)
}

/// Canvas frames kept this wide and tall (a 9:16 loop; the blur hides the
/// resolution) at about this many a second.
const VIDEO_WIDTH: u32 = 96;
const VIDEO_HEIGHT: u32 = 171;
const VIDEO_FPS: f32 = 12.5;

/// Decodes a Canvas MP4 into small RGBA frames, or `None` when it cannot be
/// read. Runs off the UI thread; best effort, like the lookup.
pub fn decode_video(bytes: &[u8]) -> Option<Video> {
    let size = bytes.len() as u64;
    let mut mp4 = Mp4Reader::read_header(Cursor::new(bytes), size).ok()?;
    let track = mp4
        .tracks()
        .values()
        .find(|track| track.track_type().ok() == Some(TrackType::Video))?;
    let id = track.track_id();
    let rate = track.frame_rate() as f32;
    let skip = (rate / VIDEO_FPS).round().max(1.0) as usize;
    let sps = track.sequence_parameter_set().ok()?.to_vec();
    let pps = track.picture_parameter_set().ok()?.to_vec();
    let samples = mp4.sample_count(id).ok()?;
    let mut decoder = Decoder::new().ok()?;
    let mut frames = Vec::new();
    let mut seen = 0usize;
    for index in 1..=samples {
        let Some(sample) = mp4.read_sample(id, index).ok().flatten() else {
            continue;
        };
        let packet = annex_b(&sps, &pps, &sample.bytes);
        if let Ok(Some(yuv)) = decoder.decode_with_options(
            &packet,
            DecodeOptions::new().flush_after_decode(Flush::NoFlush),
        ) {
            push_frame(&mut frames, &mut seen, skip, &yuv);
        }
    }
    if let Ok(rest) = decoder.flush_remaining() {
        for yuv in rest {
            push_frame(&mut frames, &mut seen, skip, &yuv);
        }
    }
    if frames.is_empty() {
        return None;
    }
    Some(Video {
        frames,
        fps: rate / skip as f32,
    })
}

fn push_frame(
    frames: &mut Vec<RgbaImage>,
    seen: &mut usize,
    skip: usize,
    yuv: &openh264::decoder::DecodedYUV<'_>,
) {
    if (*seen).is_multiple_of(skip) {
        frames.push(small_frame(yuv, VIDEO_WIDTH, VIDEO_HEIGHT));
    }
    *seen += 1;
}

/// Samples a decoded YUV frame straight down to `width` by `height`, without
/// making a full-size RGBA copy first. A blurred backdrop cannot use more,
/// and the per-pixel conversion is cheap even in a development build.
fn small_frame(yuv: &openh264::decoder::DecodedYUV<'_>, width: u32, height: u32) -> RgbaImage {
    let (source_w, source_h) = yuv.dimensions();
    let (y_stride, u_stride, v_stride) = yuv.strides();
    let (y_plane, u_plane, v_plane) = (yuv.y(), yuv.u(), yuv.v());
    let mut out = vec![0u8; (width * height * 4) as usize];
    let chroma_w = (source_w / 2).max(1);
    let chroma_h = (source_h / 2).max(1);
    for y in 0..height {
        let source_y = (y as usize * source_h / height as usize).min(source_h - 1);
        for x in 0..width {
            let source_x = (x as usize * source_w / width as usize).min(source_w - 1);
            let luma = (y_plane[source_y * y_stride + source_x] as f32 - 16.0) * 1.164;
            let (cx, cy) = (
                (source_x / 2).min(chroma_w - 1),
                (source_y / 2).min(chroma_h - 1),
            );
            let u = u_plane[cy * u_stride + cx] as f32 - 128.0;
            let v = v_plane[cy * v_stride + cx] as f32 - 128.0;
            let at = ((y * width + x) * 4) as usize;
            out[at] = (luma + 1.596 * v).clamp(0.0, 255.0) as u8;
            out[at + 1] = (luma - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8;
            out[at + 2] = (luma + 2.017 * u).clamp(0.0, 255.0) as u8;
            out[at + 3] = 255;
        }
    }
    RgbaImage::from_raw(width, height, out).expect("the frame is the right size")
}

/// A sample's length-prefixed AVCC NAL units, with SPS and PPS, as the Annex B
/// stream OpenH264 decodes. An already-Annex-B sample is passed through.
fn annex_b(sps: &[u8], pps: &[u8], sample: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sample.len() + sps.len() + pps.len() + 32);
    for nal in [sps, pps] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    if sample.starts_with(&[0, 0, 0, 1]) {
        out.extend_from_slice(sample);
        return out;
    }
    let mut at = 0;
    while at + 4 <= sample.len() {
        let len = u32::from_be_bytes([sample[at], sample[at + 1], sample[at + 2], sample[at + 3]])
            as usize;
        at += 4;
        if len == 0 || at + len > sample.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[at..at + len]);
        at += len;
    }
    out
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

    #[test]
    fn junk_is_not_a_canvas_video() {
        assert!(decode_video(b"not an mp4").is_none());
    }

    #[test]
    fn the_kawarp_blur_keeps_a_flat_colour_flat() {
        let image = RgbaImage::from_pixel(64, 64, image::Rgba([200, 40, 10, 255]));
        let blurred = blur_frame(&image, 8);
        assert_eq!(blurred.size, [128, 128]);
        let pixel = blurred.pixels[128 * 64 + 64];
        assert_eq!((pixel.r(), pixel.g(), pixel.b()), (200, 40, 10));
    }

    #[test]
    fn avcc_samples_become_annex_b_with_their_headers() {
        let sample = [0, 0, 0, 2, 0xAA, 0xBB, 0, 0, 0, 1, 0xCC];
        let out = annex_b(&[1, 2], &[3], &sample);
        assert_eq!(
            out,
            [
                0, 0, 0, 1, 1, 2, 0, 0, 0, 1, 3, 0, 0, 0, 1, 0xAA, 0xBB, 0, 0, 0, 1, 0xCC
            ]
        );
    }

    #[test]
    fn decodes_a_real_canvas_when_asked() {
        let Ok(path) = std::env::var("SPOTIFAST_CANVAS_MP4") else {
            return;
        };
        let bytes = std::fs::read(path).expect("the canvas file");
        let video = decode_video(&bytes).expect("the canvas decodes");
        assert!(video.frames.len() > 10, "{}", video.frames.len());
        assert!(video.fps > 5.0, "{}", video.fps);
        assert_eq!(
            (video.frames[0].width(), video.frames[0].height()),
            (VIDEO_WIDTH, VIDEO_HEIGHT)
        );
    }
}
