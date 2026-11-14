---
title: Privacy
description: What Spotifast stores on your computer, what it sends and to whom, and what it never collects.
nav_order: 4
---

Spotifast is a desktop app that runs entirely on your computer. It has no
account of its own, no server, no telemetry, no analytics, and no advertising.
Its author receives nothing about you or how you use it.

This page covers the Spotifast app, version 0.8.0 and later. Earlier versions
kept sign-ins in files instead of the system credential store; update to a
current release.

## What stays on your computer

- **Spotify sign-ins.** The grants Spotify issues when you sign in, and the
  reusable playback credential, are kept in the system credential store:
  Credential Manager on Windows, Keychain on macOS, and Secret Service on
  Linux. On Windows a copy encrypted for your Windows account also stays in
  the data folder, for when Credential Manager loses it. Your Spotify
  password never passes through Spotifast; you sign in on Spotify's own
  pages. A proxy password, if you set one, uses the same store.
- **Settings and history.** Settings, window positions, recent plays, the
  last session, skins, themes and MilkDrop presets live in the config
  directory. A copy started from the Windows ZIP keeps them, its state and
  its caches in `spotifast-data` next to it, so anyone who can read that
  folder, on a shared USB drive say, can read them.
- **Caches.** Downloaded audio, artwork, lyrics and library metadata live in
  the cache directory and can be deleted at any time.
- **Log.** `spotifast.log` records errors and diagnostics. It stays on your
  computer and never contains credentials; share it only if you choose to
  attach it to a bug report.

[Settings & Files](/settings-and-files/) lists every location and what is safe
to delete. **Sign out** in Settings removes the stored credentials and the
cached library metadata: playlists, Home and Liked Songs.

## What is sent, and to whom

Spotifast connects only to the services below.

- **Spotify.** Sign-in, your library, search, playlists, playback and Spotify
  Connect all go to Spotify, under your account. Spotify's own
  [privacy policy](https://www.spotify.com/legal/privacy-policy/) applies to
  that data. Home's **Made for you** and **Your top mixes** requests send an
  already verified Web API access token to Spotify's Home service as well as
  the playback session's Login5 bearer and client token. The resulting cards
  are cached in memory for ten minutes and saved in the account-scoped Home
  cache for the next start. Both caches are cleared on sign-out. If the Made
  for you request fails, the Web API receives playlist-name searches
  for approximate suggestions. Home's **New releases** request sends the
  playback session's Login5 bearer and client token to Spotify's Pathfinder
  service, which answers with the account's feed of new releases; when it
  cannot, the Web API is asked for the followed artists and their latest
  albums. The shelf is saved in the same Home cache.
- **LRCLIB.** When the lyrics panel is open and no other source has lyrics for
  the song, Spotifast sends its artist, title, album and length to
  [lrclib.net](https://lrclib.net). Nothing identifying you is included. Short
  overloads are retried within a 20-second lookup budget, respecting
  `Retry-After`; search can recover when the exact lookup is temporarily
  unavailable. A failed lookup can use older cached lyrics, but never caches
  an outage as a missing transcription.
- **Spicy Lyrics.** When the lyrics panel is open, a local file has no words,
  and a key is configured in Settings or via `SPICY_LYRICS_KEY` /
  `SPICY_LYRICS_SECRET_KEY`, Spotifast sends the Spotify track id to
  [api.spicylyrics.org](https://developers.spicylyrics.org) with that key.
  The key identifies your Spicy Lyrics account; no other account data is sent.
  The key is never written to log files and is omitted from settings export
  when empty. Answers are cached for 30 days, then discarded. Without a key,
  or when the API has no sync for the song, the catalogues below answer.
- **AMLL TTML DB.** The track's artist and title go to
  [api.amll.dev](https://api.amll.dev), which returns community TTML. Nothing
  identifying you is included. Answers are cached for 30 days.
- **Unison.** The artist, title and a search phrase go to
  [unison.boidu.dev](https://unison.boidu.dev). Its answer includes the
  submitter's display name and, when they have one, their curator page link.
  Nothing identifying you is included. Answers are cached for 30 days.
- **lrc.red.** The title and artist go to [lrc.red](https://lrc.red), followed
  by a request for the matched recording by ISRC. Nothing identifying you is
  included. Answers are cached for 30 days.
- **NetEase Cloud Music.** A search phrase built from the artist and title,
  then the matched song id, go to
  [music.163.com](https://music.163.com). Nothing identifying you is
  included. Answers are cached for 30 days.
- **GitHub.** Once a day, Spotifast asks GitHub for the latest release. You
  can turn automatic checks off in Settings. Downloading an update, and the
  first opening of MilkDrop, also fetch files from GitHub. No Spotify data is
  sent.
- **Your local network.** Spotifast looks for Spotify Connect speakers over
  mDNS and talks to the ones you choose.

Links you open from the app, such as the Winamp Skin Museum or this website,
open in your browser.

## This website

spotifast.rocks counts page visits with [Plausible](https://plausible.io/data-policy),
which uses no cookies and collects no personal data. The app itself contains
no analytics.

## Questions

Ask on [GitHub](https://github.com/crmne/spotifast/issues).

## Optional Spotify Soloist

Enabling [Soloist](/soloist/) downloads Spotify's separate software from
`soloist-builds.spotifycdn.com`. It makes its own Spotify connections under
Spotify's privacy policy, including any diagnostics its binary implements.
Spotifast's proxy does not configure those streaming connections. The personal
key is saved in the system credential store, but Soloist requires it in the
running process's arguments. Spotifast does not log those arguments or the
child's output. Pairing state is stored in Spotifast's private state folder
and removed at sign-out. Its unauthenticated local control socket is bound
to loopback only, never to the LAN. See the setup page for details and limits.
