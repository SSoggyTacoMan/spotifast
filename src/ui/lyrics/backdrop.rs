//! Spicy Lyrics' dynamic background: the cover blurred small, warped by
//! moving noise and stretched over the page (a port of @kawarp/core as
//! Spicy Lyrics Mobile runs it). The blur runs off the UI thread once per
//! cover; each frame is one shader pass on the graphics card.

use std::sync::{Arc, Mutex};

use egui::{Color32, Rect};

use crate::app::App;

/// Spicy's options: 8 Kawase passes, a 500 ms first crossfade that becomes
/// 1000 ms after the first second, a tenth of the speed when paused.
const BLUR_PASSES: usize = 8;
const FIRST_CROSSFADE_S: f64 = 0.5;
const CROSSFADE_S: f64 = 1.0;
const PAUSED_RATE: f32 = 0.1;
/// Flow at the quietest and loudest music, before the song's energy joins it.
const BASE_RATE: f32 = 1.2;
const ENERGY_RATE: f32 = 1.4;
/// Energy without an audio analysis, as Spicy's own neutral value.
const NEUTRAL_ENERGY: f32 = 0.35;
/// Easing times for play/pause and for the music's energy, in seconds.
const PLAY_EASE_S: f32 = 0.6;
const ENERGY_ATTACK_S: f32 = 1.1;
const ENERGY_RELEASE_S: f32 = 2.8;
/// Samples read from the visualizer's post-EQ, pre-volume tap for the energy.
const ENERGY_SAMPLES: usize = 2_048;

/// Maps an RMS level to 0..1 over a 34 dB window, quiet at -40 dB and loud at
/// -6 dB, so the background breathes with the music instead of following an
/// absolute level that every modern master pins near the top.
fn energy_from_rms(rms: f32) -> f32 {
    let db = 20.0 * rms.max(1e-5).log10();
    ((db + 40.0) / 34.0).clamp(0.0, 1.0)
}

fn ease(dt: f32, seconds: f32) -> f32 {
    1.0 - (-dt / seconds.max(0.001)).exp()
}

fn audio_energy(app: &App, now: Option<&crate::app::NowPlaying>) -> Option<f32> {
    if !app.visualizer_sounding(now) {
        return None;
    }
    let samples = app.winamp.tap.window(ENERGY_SAMPLES, app.visualizer_lag());
    if samples.is_empty() {
        return None;
    }
    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    Some(energy_from_rms(rms))
}

fn blur(image: &image::RgbaImage) -> egui::ColorImage {
    crate::canvas::blur_frame(image, BLUR_PASSES)
}

#[derive(Default)]
pub struct Backdrop {
    uri: Option<String>,
    pending: Option<std::sync::mpsc::Receiver<Option<egui::ColorImage>>>,
    current: Option<egui::TextureHandle>,
    next: Option<egui::TextureHandle>,
    black: Option<egui::TextureHandle>,
    since: Option<f64>,
    created: Option<f64>,
    transition_s: f64,
    time: f32,
    speed: f32,
    play: f32,
    energy: f32,
    last: Option<f64>,
    /// The track whose Canvas was last looked up or is playing.
    canvas_track: Option<String>,
    canvas_pending: Option<std::sync::mpsc::Receiver<Option<crate::canvas::BlurredVideo>>>,
    video: Option<VideoFrames>,
    /// When the current Canvas began fading in from the cover.
    video_since: Option<f64>,
}

/// A decoded Canvas loop on the graphics card. The texture is updated in
/// place as the loop advances, so only one frame is ever held by egui.
struct VideoFrames {
    frames: Vec<egui::ColorImage>,
    fps: f32,
    texture: Option<egui::TextureHandle>,
    clock: f64,
    index: usize,
}

type Shared = Arc<Mutex<Backdrop>>;

fn shared(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::new("lyrics-kawarp"), || {
            Arc::new(Mutex::new(Backdrop::default()))
        })
        .clone()
    })
}

impl Backdrop {
    fn want(
        &mut self,
        ctx: &egui::Context,
        app: &App,
        art: Option<&str>,
        track: Option<&str>,
        track_id: Option<&str>,
    ) {
        if self.uri.as_deref() != art {
            self.uri = art.map(str::to_owned);
            if let Some(uri) = art {
                let (tx, rx) = std::sync::mpsc::channel();
                self.pending = Some(rx);
                let art_loader = app.backend.art();
                let ctx = ctx.clone();
                art_loader.kawarp(uri.to_owned(), blur, move |image| {
                    let _ = tx.send(image);
                    ctx.request_repaint();
                });
            }
        }
        if self.canvas_track.as_deref() != track {
            self.canvas_track = track.map(str::to_owned);
            self.video = None;
            self.video_since = None;
            self.canvas_pending = None;
            let Some(track_id) = track_id.filter(|id| crate::canvas::is_track_id(id)) else {
                return;
            };
            if !app.settings.lyrics_canvas {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            self.canvas_pending = Some(rx);
            let art = app.backend.art().clone();
            let ctx = ctx.clone();
            let video_art = art.clone();
            art.canvas_url(track_id.to_owned(), move |url| {
                let Some(url) = url else {
                    let _ = tx.send(None);
                    ctx.request_repaint();
                    return;
                };
                let tx = tx.clone();
                let ctx = ctx.clone();
                video_art.canvas_video(url, move |video| {
                    let _ = tx.send(video);
                    ctx.request_repaint();
                });
            });
        }
    }

    fn receive(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(rx) = &self.canvas_pending {
            match rx.try_recv() {
                Ok(video) => {
                    self.canvas_pending = None;
                    if let Some(video) = video {
                        self.start_video(ctx, video);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.canvas_pending = None,
            }
        }
        let Some(rx) = &self.pending else { return };
        let Ok(image) = rx.try_recv() else { return };
        self.pending = None;
        let Some(image) = image else { return };
        let texture = ctx.load_texture("lyrics-kawarp", image, egui::TextureOptions::LINEAR);
        match self.next.take() {
            None => {
                self.current = Some(texture.clone());
                self.next = Some(texture);
                self.since = None;
            }
            Some(previous) => {
                self.current = Some(previous);
                self.next = Some(texture);
                self.since = Some(now);
            }
        }
    }

    fn start_video(&mut self, ctx: &egui::Context, video: crate::canvas::BlurredVideo) {
        let frames = video.frames;
        if frames.is_empty() {
            return;
        }
        log::debug!(
            "backdrop: canvas video started with {} frames at {:.1} fps",
            frames.len(),
            video.fps
        );
        let texture = ctx.load_texture(
            "lyrics-canvas",
            frames[0].clone(),
            egui::TextureOptions::LINEAR,
        );
        self.video = Some(VideoFrames {
            frames,
            fps: video.fps.max(1.0),
            texture: Some(texture),
            clock: 0.0,
            index: 0,
        });
        self.video_since = None;
    }

    fn blend(&mut self, now: f64) -> f32 {
        let Some(since) = self.since else { return 1.0 };
        let f = ((now - since) / self.transition_s.max(0.001)).min(1.0);
        if f >= 1.0 {
            self.since = None;
            self.current = self.next.clone();
        }
        (0.5 - 0.5 * (f * std::f64::consts::PI).cos()) as f32
    }
}

/// Paints the background over `rect`; `filter` is the CSS `saturate()` and
/// `brightness()` over it (2.5 and 0.65 on the page, 2.65 and 0.6 in the
/// side panel). Returns whether it could draw.
pub fn paint(app: &App, ui: &egui::Ui, rect: Rect, filter: (f32, f32)) -> bool {
    let ctx = ui.ctx().clone();
    let shared = shared(&ctx);
    let mut backdrop = shared.lock().expect("backdrop");
    let now = ui.input(|i| i.time);
    let created = *backdrop.created.get_or_insert(now);
    backdrop.transition_s = if now - created < FIRST_CROSSFADE_S * 2.0 {
        FIRST_CROSSFADE_S
    } else {
        CROSSFADE_S
    };
    let now_playing = app.now_playing();
    let playing = now_playing.as_ref().is_some_and(|n| n.playing);
    let uri = now_playing
        .as_ref()
        .and_then(|n| n.art_small.clone().or_else(|| n.art_url.clone()));
    let track = now_playing.as_ref().map(|now| now.uri.as_str());
    let track_id = now_playing.as_ref().and_then(|now| now.id.as_deref());
    backdrop.want(&ctx, app, uri.as_deref(), track, track_id);
    backdrop.receive(&ctx, now);
    if !app.settings.lyrics_canvas {
        backdrop.video = None;
        backdrop.video_since = None;
        backdrop.canvas_pending = None;
        backdrop.canvas_track = None;
    }
    let black = backdrop
        .black
        .get_or_insert_with(|| {
            ctx.load_texture(
                "lyrics-kawarp-black",
                egui::ColorImage::filled([1, 1], Color32::BLACK),
                egui::TextureOptions::LINEAR,
            )
        })
        .id();
    let low = app.settings.lyrics_low_performance;
    let reduced = app.motion.reduced();
    let dt = backdrop
        .last
        .map_or(0.0, |last| (now - last).clamp(0.0, 0.1) as f32);
    backdrop.last = Some(now);
    let target_energy = audio_energy(app, now_playing.as_ref()).unwrap_or(NEUTRAL_ENERGY);
    let target_play = if playing { 1.0 } else { 0.0 };
    backdrop.play += (target_play - backdrop.play) * ease(dt, PLAY_EASE_S);
    let energy_tau = if target_energy > backdrop.energy {
        ENERGY_ATTACK_S
    } else {
        ENERGY_RELEASE_S
    };
    backdrop.energy += (target_energy - backdrop.energy) * ease(dt, energy_tau);
    let target = if reduced || low {
        0.0
    } else if playing {
        BASE_RATE + ENERGY_RATE * backdrop.energy
    } else {
        PAUSED_RATE
    };
    if backdrop.speed == 0.0 {
        backdrop.speed = target;
    }
    backdrop.speed += (target - backdrop.speed) * ease(dt, PLAY_EASE_S);
    if !low && !reduced {
        backdrop.time += dt * backdrop.speed;
    }
    let scale = if reduced || low {
        1.0
    } else {
        1.0 + 0.03 * backdrop.energy * backdrop.play
    };
    let video_frame = backdrop.video.as_mut().and_then(|video| {
        if low || reduced || video.frames.is_empty() {
            return None;
        }
        if playing {
            video.clock += f64::from(dt);
            let index = ((video.clock * f64::from(video.fps)) as usize) % video.frames.len();
            if index != video.index {
                video.index = index;
                if let Some(texture) = &mut video.texture {
                    texture.set(video.frames[index].clone(), egui::TextureOptions::LINEAR);
                }
            }
        }
        video.texture.as_ref().map(egui::TextureHandle::id)
    });
    let blend = backdrop.blend(now);
    let current = backdrop.current.as_ref().map_or(black, |t| t.id());
    let next = backdrop.next.as_ref().map_or(black, |t| t.id());
    let (current, next, blend) = match video_frame {
        Some(frame) => {
            let started = *backdrop.video_since.get_or_insert(now);
            let f = ((now - started) / 0.6).clamp(0.0, 1.0);
            let eased = (0.5 - 0.5 * (f * std::f64::consts::PI).cos()) as f32;
            if eased >= 1.0 {
                (frame, frame, 1.0)
            } else {
                (current, frame, eased)
            }
        }
        None => (current, next, blend),
    };
    super::gpu::background(
        &ui.painter().with_clip_rect(rect),
        rect,
        current,
        next,
        backdrop.time,
        blend,
        scale,
        filter,
    );
    let crossfading = backdrop.since.is_some();
    drop(backdrop);
    if crossfading || (!low && playing && !reduced) {
        ctx.request_repaint();
    } else if !low && !reduced {
        // Paused, a tenth of the speed: 20 frames a second look the same.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_background_breathes_with_the_music() {
        assert_eq!(energy_from_rms(0.0), 0.0);
        assert!(energy_from_rms(0.02) < energy_from_rms(0.2));
        assert!((energy_from_rms(0.5) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn playing_eases_faster_than_the_energy_alone() {
        // A frame at 60 Hz moves play most of the way in about half a second,
        // while the energy attack is deliberately slower.
        assert!(ease(1.0 / 60.0, PLAY_EASE_S) > ease(1.0 / 60.0, ENERGY_ATTACK_S));
        assert!(ease(0.6, PLAY_EASE_S) > 0.6);
    }
}
