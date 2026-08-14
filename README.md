# renoise2mod

Converts Renoise `.xrns` songs to classic tracker formats: `.mod` (ProTracker) and `.xm`
(FastTracker II). It's a from-scratch Rust rewrite of
[xrns2xmod](https://github.com/fstarred/xrns2xmod), with no external runtime dependencies (the
original needs Mono and a proprietary Windows audio library, which made it a pain to run on
Linux/macOS). Ships as a Renoise Tool with native binaries for Windows, macOS (Intel + Apple
Silicon), and Linux bundled in, so there's nothing else to install.

## Install

Download `com.konsumer.Renoise2Mod.xrnx` from the [latest
release](https://github.com/konsumer/renoise2mod/releases/latest) and drag it onto the running
Renoise window (or double-click it with Renoise installed). That's it — Renoise installs it
immediately, no unzipping or copying files into a tools folder by hand.

Once installed, use **File > Export to MOD/XM...** on a saved song.

## Standalone CLI

The same conversion engine is also a plain command-line tool, `renoise2mod`, if you'd rather script
conversions or don't need Renoise's UI. Grab the binary for your platform from the same
[release](https://github.com/konsumer/renoise2mod/releases/latest) page.

```sh
renoise2mod song.xrns --type xm
renoise2mod song.xrns --type mod --ptmode hardware --ntsc
renoise2mod --help
```

## Fidelity to the original

This aims to reproduce xrns2xmod's actual behavior faithfully — including its real format
limitations (MOD's 31-instrument cap, one sample per instrument, 64-row patterns, 4-channel
minimum; XM's 96-note keymap, 12 envelope points) — while fixing bugs found along the way rather
than reproducing them:

- **XM sample bit-depth flag**: the original wrote the raw bit-depth value (8 or 16) into the
  sample type byte instead of a proper flag bit, silently setting a stray bit for every 8-bit
  sample (it only "worked" for 16-bit samples because 16 happens to equal the correct flag value,
  0x10, in hex).
- **XM panning wraparound**: hard-right pan (`255`) computed as `256`, which wrapped around to `0`
  (hard-left) when stored in a byte.
- **MOD never read the master-track effect column**: XM parsed one master-track column per row;
  MOD's equivalent was hardcoded to parse zero, seemingly by oversight. Both now behave the same.
- **Tick-per-row detection bug**: the original only checked the *second* character of an effect
  command when looking for a tempo-timing command, so a per-note command like `"0L"` could be
  misread as a global tempo change. Now requires the correct `"Z"` prefix, and also checks the
  master track (which the original never did).
- **Pattern-break corruption**: `0Dxx` (pattern break) round-tripped its target row through a
  decimal-then-hex reinterpretation, silently landing on the wrong row for any value ≥ 10.
- **Inconsistent volume-slide precision**: `'I'` (fade in) picked between a fine and coarse slide
  command based on precision loss; `'O'` (fade out) always used the coarse one. Now symmetric.
- **Pattern-order crash**: a song whose pattern order references non-contiguous pattern indices
  (e.g. patterns `{0, 1, 5}` but never `2`/`3`/`4`) could crash the original outright. Rewritten to
  produce the same output for every case that didn't crash, without crashing on the rest.
- Added warning logs for a few previously-silent truncations (too many instruments, oversized
  patterns, oversized pattern-order tables) for consistency with how other limits were already
  logged.

Resampling quality is not byte-identical to the original (which used a proprietary commercial
resampler) — output should sound equivalent, not bit-for-bit identical.

## Development

A Cargo workspace:

- `crates/renoise2mod-core` — the conversion library (`.xrns` parsing, MOD/XM writers, audio
  decode/resample/encode)
- `crates/renoise2mod` — the CLI binary
- `tool/` — the Renoise Tool source (`manifest.xml` + `main.lua`); CI drops platform binaries into
  `tool/bin/` before zipping it into the `.xrnx` release artifact

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```
