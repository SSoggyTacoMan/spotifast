# Spotifast Spicy Lyrics fork: third-party notices

This is a modified fork, not an official release of Spotifast or Spicy Lyrics.
The native lyrics port was made on **17 November 2026**. The combined fork is
licensed under **AGPL-3.0-only**; unchanged upstream Spotifast code retains its
MIT permission and copyright notice. [LICENSE](LICENSE) contains the full AGPL
text, the original Spotifast MIT notice, and the adapted MIT-code notices. Keeping
them in that file also includes them in existing binary package payloads.

## Adapted code

| Project | Reference revision | Adapted parts | License |
| --- | --- | --- | --- |
| [Spicy Lyrics Mobile](https://github.com/spicylyrics/mobile), TX24 and contributors | `f005aaa33a51d1fedfab8af15fce9f929dc41407` | Timeline preparation, layout, Spring and Bloom animation, scrolling, held letters, background vocals, interlude dots and renderer settings | AGPL-3.0 |
| [Spicy Lyrics](https://github.com/Spikerko/spicy-lyrics), Spikerko and contributors | `1c02f47cd64ed8c34bb76dd8ff00755db9d1d66f` | Now Playing lyrics card, fullscreen arrangement, background filters and related interaction defaults | AGPL-3.0 |
| [@kawarp/core](https://github.com/better-lyrics/kawarp) 1.2.2, Copyright (c) 2026 Better Lyrics | Via Mobile's fused shader and blur implementation | Artwork preparation and the animated background, adapted to GLSL/OpenGL | MIT |
| [cubic-spline](https://github.com/morganherlocker/cubic-spline), Copyright (c) 2014 Morgan Herlocker | Via Mobile's spline implementation | Natural cubic interpolation used by word and letter animation | MIT |
| [spr](https://github.com/Fraktality/spr), Copyright (c) 2023 Fractality | Via Mobile's spring implementation | Damped spring simulation | MIT |
| [OpenH264](https://github.com/cisco/openh264), Copyright (c) Cisco | Used through the `openh264` crate | H.264 decoding for the Spotify Canvas background | BSD-2-Clause |
| [mp4](https://github.com/alfg/mp4-rust), Copyright (c) 2016 Alfred Gutierrez | Used directly | MP4 demuxing for the Spotify Canvas background | MIT |

The Rust port lives in `src/ui/lyrics/`. This is source-derived behavior, not
a claim of pixel-perfect or frame-by-frame parity with the reference apps.
No mobile source-fetching, account, romanization dictionaries, fonts or hosted
services were copied. Spotifast's existing lyrics providers and playback remain
in use.

## Upstream and assets

Upstream [Spotifast](https://github.com/crmne/spotifast):
Copyright (c) 2026 Carmine Paolino, MIT License (retained in `LICENSE`).
Existing dependencies retain their own licenses. Bundled font and icon notices
remain in `assets/fonts/` and `assets/icons/`. The lyrics font on macOS is read
from the local system font installation, not redistributed by this fork.

## Before distributing this fork

Publish the complete corresponding source for the exact binary revision,
including build scripts and dependency information, and provide access alongside
the binary download as required by AGPL section 6. Retain these notices and
`LICENSE`, and ensure the source link in Settings > About points to the published
fork source. A license label or attribution alone does not meet the source
distribution requirement. This change does not publish a release or upload code.
