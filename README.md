# Lonewolf for PS Vita

Experimental PS Vita port of **LONEWOLF 1.2.90**, using Ruffle's ActionScript 3 runtime and a VitaGL renderer. Earlier hardware runs reached the game. Audio output and saves are implemented; audible playback, mission completion and saved progress still need device verification.

This repository contains the runtime source, tools, pinned dependencies and their licenses. The game APK, SWF, artwork, audio, saves and bundled game VPK are not distributed here. Russian is the default for a new game; an existing language preference remains authoritative.

## Required game input

The current profile accepts the **1.2.90** APK recorded as `LONEWOLF_v1.2.90_Mod.apk`, application ID `com.FDGEntertainment.Lonewolf.gp` (Adobe AIR 32). The entire APK must match `tools/inputs.lock.json`:

```text
aeddf65888006671463234b1cb8efbeb7b4bb297744fe0f15cffe38fea99542c
```

A filename or matching version number does not establish compatibility. Other APK builds are not covered by this input profile. Supply your own copy locally; do not commit it.

## Build and inspect the host runtime

Requirements: the Rust toolchain from `rust-toolchain.toml` (**nightly-2026-10-01**, including `rust-src`), Python 3.10+, Java for the pinned ActionScript compiler, and a host C compiler. Cargo resolves dependencies using `Cargo.lock`; project-specific Ruffle changes and required upstream compiler/font resources are retained under `vendor/`.

```sh
cargo build --locked
python3 tools/inspect_apk.py /path/to/LONEWOLF_v1.2.90_Mod.apk --verify tools/inputs.lock.json
python3 tools/prepare_runtime.py /path/to/LONEWOLF_v1.2.90_Mod.apk --output build/Lonewolf-air.swf
cargo run --locked -- build/Lonewolf-air.swf
python3 tools/test_prepare_runtime.py /path/to/LONEWOLF_v1.2.90_Mod.apk --probe target/debug/lonewolf-vita
```

The host probe runs three frames, checks the AVM2 display tree and rejects runtime errors. It does not render or certify gameplay. Its navigator is offline and its saves are memory-only.

Preparation verifies the APK, combines the game's original extension definitions with the SWF and adds the checked AIR support class. It extracts the matching locale files beside the SWF. The current settings patch enables premium and disables advertising after profile loading; other original game bytecode is retained. No purchase or rewarded-ad callbacks are fabricated, and installation does not rewrite existing save files. Generated game files remain under ignored `build/`.

## Private Vita packaging

The Vita build requires VitaSDK with VitaGL, vitaShark, math-neon, SceShaccCgExt and their stub libraries. The SDK's `armvita.x` must support `__sce_headroom`. Install `libshacccg.suprx` for runtime shader compilation.

`tools/build_vita.py` is a local test packager that **includes game data**. It also requires the exact approved LiveArea images listed in `tools/livearea.lock.json`; those game-derived images are not included in this source repository. A clean checkout can build the host runtime, but cannot create a complete VPK without these private inputs. Pillow is needed by the artwork preparation tools.

With your locally prepared game, required artwork and SDK available:

```sh
python3 tools/build_vita.py --sdk /path/to/vitasdk
```

The default is a Release build; `--debug` selects Debug. Output: `build/vita/LonewolfVita.vpk`, Title ID **LONE00001**, version **00.02**. The packager checks source inputs, prepared SWF, artwork hashes, ELF identity and VPK CRC, and writes `build/vita/package.json`. A successful build does not prove a physical Vita run. Do not publish a VPK that contains game data.

## Controls

| Control | Action |
| --- | --- |
| Left or right stick | Aim during gameplay |
| L held | Fine aim with the gyroscope |
| R or Cross | Fire; sniper: tap to shoot, hold to steady/slow time, release to shoot |
| D-pad up/down | Sniper zoom in/out |
| Square | Reload |
| Circle held | Lower pistol/assault rifle; release to raise |
| Start | Pause/resume |
| Select | Objective panel |
| Left stick and Cross in menus | Pointer and click |
| Front touch | Native game touch controls |

Logs append to `ux0:data/lonewolf/runtime.log`. Vita saves live under `ux0:data/lonewolf/saves` with two checked generations. Preserve existing saves during code updates.

## Dependencies and licenses

Ruffle core is pinned to `563854a8911eda672b1836fb374063162879035d` and licensed MIT OR Apache-2.0. Retained dependency license files apply to their respective components. These source licenses do not grant rights to the original LONEWOLF game content.
