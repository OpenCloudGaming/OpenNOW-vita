# Changelog

All notable changes to OpenNOW Vita are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2026-08-02

Error handling pass on top of 0.3.0: launch failures now speak GeForce NOW's
own error codes instead of raw JSON, and two causes of a stuck or locked-out
launch are fixed.

### Changed

- Failed launches now say what went wrong. GeForce NOW's error codes are modelled
  properly — 121 of them, 69 with wording in English and Spanish, ported from
  OpenNOW's `gfnErrorCodeEnum.ts` — so a failure shows "Membership Upgrade
  Required" or "Region At Capacity" instead of a truncated dump of NVIDIA's JSON.
  An unrecognised code is named rather than pasted.
- The code also drives behaviour, replacing three separate substring classifiers
  that had grown up around the error text — one picking the error screen's
  wording, one deciding whether to refresh the token, one deciding whether a
  catalog failure was an authorization problem. They matched on text that had
  *already been translated*, so a Spanish player took different branches than an
  English one.
- A poll failure NVIDIA reports as final now stops immediately instead of
  spending the whole 5xx retry allowance — about 2.5 minutes — re-asking about a
  banned region or a membership problem.
- The catalog now defaults to the account's owned games instead of the whole
  GFN catalog (same `library.status` filter as OpenNOW's `LIBRARY_APPS_FILTER`),
  with a "My Games / All Games" picker next to the sort dropdown to switch back.

### Fixed

- Launches abandoned with `HTTP 503` while NVIDIA was installing a game update.
  CloudMatch reports patching as a 5xx with `statusCode` 41, which counted
  against the server-error allowance and gave up after ~2.5 minutes — far short
  of how long a patch takes. It is now recognised as progress, with its own
  "Updating the game" screen. Mirrors OpenNOW-Switch's `IsAppPatchingResponse`.
- Launch failures now lead with the CloudMatch status code and description
  instead of opening with raw JSON that the error screen truncates.
- "A session is already open" locking an account out for the several minutes
  NVIDIA takes to reap a session, after a crash or a force-quit. Three causes:
  - The device id sent to CloudMatch was a UUIDv5 of a fixed string, so it was
    *identical on every Vita running the app* — and NVIDIA refuses a `DELETE`
    from a device that does not own the session. It is now the same per-install
    id sign-in already persisted, matching OpenNOW-Switch's `GenerateDeviceId`.
  - The open session is now written to the memory card with the zone that
    provisioned it, and deleted on the next launch. The normal stop does not run
    when the process dies, and asking CloudMatch which sessions are open does not
    say which zone to delete them at.
  - Both cleanup paths deleted against the generic CloudMatch entry point rather
    than the session's own zone, so they could not remove anything.

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

[0.3.1]: https://github.com/OpenCloudGaming/OpenNOW-vita/releases/tag/v0.3.1
[0.3.0]: https://github.com/OpenCloudGaming/OpenNOW-vita/releases/tag/v0.3.0
[0.2.1]: https://github.com/OpenCloudGaming/OpenNOW-vita/releases/tag/v0.2.1
