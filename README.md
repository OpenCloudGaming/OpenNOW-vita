<div align="center">

# OpenNOW Vita

**GeForce NOW on PlayStation Vita**

A native Rust homebrew client — sign in, browse your library, and stream games
with hardware H.264 decode and full controller input.

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-brightgreen.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-nightly-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-PS%20Vita-003791.svg)](https://vitasdk.org/)
[![Status](https://img.shields.io/badge/status-working%20on%20hardware-success.svg)](#status)

</div>

> [!WARNING]
> **Unofficial project.** OpenNOW Vita is not affiliated with, endorsed by, or
> associated with NVIDIA or GeForce NOW. You need your own GeForce NOW account
> to use it.

Built in the spirit of [green-vita](https://github.com/Day-OS/green-vita) (Xbox
Cloud Gaming on the Vita): SDL2 + egui UI, direct-to-texture hardware video
decoding, and VPK packaging via `cargo-vita`. GFN protocol work references
[OpenNOW](https://github.com/OpenCloudGaming/OpenNOW).

---

## Features

| | |
|---|---|
| **NVIDIA login on console** | Device-code flow (QR + short code). Tokens encrypted at rest with ChaCha20-Poly1305; key in Vita Safe Memory. |
| **Game library** | GFN catalog with cover art, server-side search, and detail pages (GraphQL). |
| **Session brokering** | CloudMatch create / queue / seat-aware polling against the assigned game server. |
| **Real WebRTC streaming** | NVST signaling, SDP offer/answer, ICE-lite, DTLS-SRTP, H.264 RTP — via the sans-I/O [`rtc`](https://github.com/webrtc-rs/rtc) stack (no GStreamer, no browser). |
| **Hardware video decode** | `sceAvcdec` → SDL/GXM textures (zero per-frame allocs, double-buffered, YUV420/BGR565 negotiated at runtime). |
| **Audio** | Opus RTP via `libopus` + SDL2, with small jitter buffers so A/V stay locked. |
| **Controller input** | Full gamepad state at 60 Hz over NVST `input_channel_v1` (XInput). |
| **Session resilience** | Transient CloudMatch 5xx tolerated; disconnects tear down the server session cleanly. |
| **Language picker** | English / Spanish on the catalog screen (`src/i18n/`). |

---

## Status

| Phase | Scope | State |
|:-----:|-------|-------|
| 0 | Protocol research | Done |
| 1 | App skeleton (VitaSDK / `cargo-vita`, SDL2 + egui) | Done |
| 2 | Authentication + game library | Done |
| 3 | Signaling + CloudMatch lifecycle | Done |
| 4 | WebRTC peer, H.264 decode, gamepad | Working (Vita3K + hardware) |
| 5 | Opus audio, resilience, UI polish | Working (Vita3K + hardware) |
| 6 | Real-hardware validation | Confirmed on original PS Vita |

> [!NOTE]
> **Known gaps:** analog triggers / L3 / R3 need a rear-touchpad mapping (no
> physical controls on Vita). Language picker is English/Spanish only so far;
> UI outside the catalog is still mostly hardcoded Spanish. Validated on
> [Vita3K](https://vita3k.org/) (YUV420-only `sceAvcdec`, handled at runtime)
> and real hardware.

See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for green-vita (MPL-2.0)
reuse and OpenNOW protocol references.

---

## Build requirements

1. **[VitaSDK](https://vitasdk.org/)** with `VITASDK` set (not installed by this project).
2. **Rust nightly** + [`cargo-vita`](https://github.com/vita-rust/cargo-vita):

   ```sh
   rustup toolchain install nightly
   cargo +nightly install cargo-vita
   rustup target add armv7-sony-vita-newlibeabihf --toolchain nightly
   ```

3. **`pkg-config`** (e.g. `brew install pkg-config` on macOS).

---

## Building

```sh
make vpk                                    # → target/.../release/jade-vita.vpk
make ftp VITA_IP=192.168.0.103              # upload over VitaShell FTP (recommended)
make upload-vpk VITA_IP=192.168.0.103       # upload via cargo-vita / vitacompanion
make update-run-vita VITA_IP=192.168.0.103  # rebuild eboot, push, and launch
```

Uploading needs [VitaShell](https://github.com/TheOfficialFloW/VitaShell) FTP or
[`vitacompanion`](https://github.com/devnoname120/vitacompanion) on the same
network. The VPK also installs and runs in Vita3K.

> [!TIP]
> Prefer `make ftp` over `upload-vpk` when VitaShell FTP is available — it
> verifies the remote size after transfer. Copying the VPK to the card does
> **not** install it; open it in VitaShell once.

> [!IMPORTANT]
> Enable **Unsafe Homebrew** in HENkaku Settings so the app can load the
> hardware video-decoder module.

---

## Project layout

```text
.cargo/config.toml      Cross-compile target (armv7-sony-vita-newlibeabihf)
tools/                  vita-gcc / vita-ar / vita-pkg-config wrappers
static/sce_sys/         Icon + LiveArea assets packaged into the VPK
src/
  main.rs               Entry; heap/stack sizing, CDRAM reservation
  app/                  State machine + egui UI
  shell/                Main loop: SDL2, egui painter, direct video surface
  input.rs              SDL2 → XInput snapshots
  locale.rs / i18n/     Fluent locales (EN / ES on catalog so far)
  streaming/
    video/              Direct-texture pipeline (sceAvcdec + decode worker)
    audio.rs            Opus RTP + SDL2 playback
  gfn/
    auth.rs             Device-code OAuth + encrypted tokens
    catalog.rs          Library + search (GraphQL)
    covers.rs           Cover-art cache
    cloudmatch.rs       Session create / poll / stop
    signaling.rs        NVST WebSocket signaling
    sdp.rs              Offer sanitation + answer blob
    peer.rs             Sans-I/O WebRTC peer
    input_protocol.rs   NVST input-channel wire format
```

LiveArea assets under `static/sce_sys/` are placeholders — replace before
distributing a public VPK.

---

## Acknowledgements

- [green-vita](https://github.com/Day-OS/green-vita) — direct-texture video path,
  Vita-patched `ring` / `rtc-shared`, and proof cloud gaming on Vita is viable
- [OpenNOW](https://github.com/OpenCloudGaming/OpenNOW) — GFN protocol reference
  (CloudMatch, NVST, input channel)
- MattKC's [Vanilla](https://github.com/vanilla-wiiu/vanilla) — single-reference-frame
  decoder trick
- [VitaSDK](https://vitasdk.org/) and [vita-rust](https://github.com/vita-rust)

---

## License

[Mozilla Public License 2.0](LICENSE)
