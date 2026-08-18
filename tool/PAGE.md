# Renoise2Mod

Export the current song to a classic `.mod` (ProTracker) or `.xm` (FastTracker II) tracker file,
right from Renoise. Native binaries for Windows, macOS (Intel + Apple Silicon), and Linux are
bundled in -- nothing else to install.

## Usage

Once installed, use **File > Export to MOD/XM...** on a saved song.

## Fidelity

A from-scratch Rust rewrite of [xrns2xmod](https://github.com/fstarred/xrns2xmod) that reproduces
the original's real format limitations (MOD's 31-instrument cap, one sample per instrument,
64-row patterns, 4-channel minimum; XM's 96-note keymap, 12 envelope points), while fixing several
bugs found along the way -- sample bit-depth flags, panning wraparound, pattern-break corruption,
tick-per-row misdetection, and a few others. Resampling isn't byte-identical to the original's
proprietary resampler, but should sound equivalent.

## Standalone CLI

The same conversion engine also ships as a plain command-line tool, `renoise2mod`, for scripting
conversions outside Renoise -- see the [GitHub
releases](https://github.com/konsumer/renoise2mod/releases/latest) page for per-platform
downloads.

Source, issues, and full changelog: https://github.com/konsumer/renoise2mod
