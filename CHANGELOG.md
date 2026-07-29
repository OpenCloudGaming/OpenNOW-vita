# Changelog

All notable changes to OpenNOW Vita are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-07-31

Closes the two gaps 0.2.1 shipped with — no rear-touch mapping for the analog
triggers or L3/R3, and no way to type — and turns the stream from "it runs" into
something tunable.

### Added

#### Input
- In-game keyboard, on the Vita's inline IME. Characters, Backspace, Enter and
  the arrow keys are inferred from the IME's buffer edits and forwarded to the
  game as real keystrokes.
- Rear touch panel mapped to L2/R2, with selectable trigger intensity.
- L3/R3 zones on the front touch screen, optionally drawn over the stream.
- Trackpad mode: the front panel as a mouse, for games that want one.
- In-stream toolbar — exit, stats, control settings, trackpad and keyboard —
  collapsible so it stays out of the picture.

#### Catalog
- Favourites, kept on the memory card. Each entry stores enough of the game to
  draw its row, so a favourite past the catalog's 1000-title page cut-off still
  appears instead of vanishing until you search for it.
- Sorting by recently played, recommended, or title.

#### Streaming
- Opus pipeline reworked with a jitter buffer, RED packet recovery and a gain
  stage. NVST audio arrives out of order often enough to matter over 2.4 GHz,
  and much quieter than a local GameStream host.
- Audio boost, selectable and persisted.
- Link estimation: the client remembers what the network actually delivered and
  asks for a ceiling the link has been seen to reach, rather than a hardcoded
  guess that costs the opening seconds of every session in lost packets and
  resolution drops.
- Selectable frame rate, persisted between sessions.
- Stats overlay for the live session.

#### Platform
- CPU/GPU clocks raised to a streaming profile. The Vita boots homebrew at
  conservative clocks, and the shell loop paces the whole video pipeline.
- Explicit thread-to-core affinity across the three user cores, so the shell
  loop, video decode and network threads stop contending for the same one.

### Fixed

- In-game keyboard taking the firmware down with `C2-12828-1` the moment it
  opened. Four independent causes, found by diffing against vita-moonlight's
  `keyboardsystem.c`:
  - `SCE_SYSMODULE_IME` was never loaded, so the first call into libime jumped
    through an unresolved import.
  - `sdkVersion` was a hand-written guess instead of `PSP2_SDK_VERSION`.
  - The IME event handler called back into libime (`sceImeSetText` /
    `sceImeSetCaret`). The caret reset now runs on the owning thread in
    `update()`, behind a flag the handler raises.
  - `initialText` and `inputTextBuffer` pointed at the same buffer, leaving
    libime reading the text it was concurrently writing.
- `sceImeOpen` ran on a scratch thread that exited immediately, leaving
  `sceImeUpdate` pumping a session whose owning thread was gone. Every libime
  call now shares the shell loop's thread.
- SDL's text input — itself an IME dialog — is no longer started while the
  inline IME is open. libime does not tolerate both at once.
- Double `sceImeClose` when the keyboard was dismissed from the IME's own close
  button.

## [0.2.1] - 2026-07-28

First public release.

### Added

- On-console NVIDIA login with device-code flow and encrypted tokens.
- Game library with cover art and server-side search.
- Session brokering through CloudMatch, with queue tracking.
- WebRTC streaming: NVST signalling and H.264 depacketization.
- Hardware video decode via `sceAvcdec`.
- Opus audio decode and playback through SDL2.
- Controller input at 60×/s over the NVST data channel.
- Session resilience for transient failures.
- English and Spanish UI.

### Known gaps

- Analog triggers and L3/R3 had no rear-touchpad mapping. *(Addressed in 0.3.0.)*

[0.3.0]: https://github.com/OpenCloudGaming/OpenNOW-vita/releases/tag/v0.3.0
[0.2.1]: https://github.com/OpenCloudGaming/OpenNOW-vita/releases/tag/v0.2.1
