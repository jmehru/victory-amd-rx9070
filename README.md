# VIctory fork (work in progress)

## rx9070 branch

This branch is [myokex/victory-amd](https://github.com/myokex/victory-amd) at
`070df42` with changes found while running on one machine: Windows 11, Radeon
RX 9070 (RDNA4, driver 32.0.32029.2010), Ryzen 5 7600 (6 cores), 32 GB RAM.
Nothing here has been tested on another card. Work goes to the `perf/submit`
branch first.

### State on that machine (2026-11-26, release build, Series S profile, FSR on)

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Prologue, the cell, standing still | 50 to 52 ms a frame (19 to 20 fps) |
| First mission, the boat at rest | about 47 ms (21 fps) |
| The same with frame generation (`VICTORY_FRAME_GEN=1`) | 17 to 18 real frames shown as 32 to 34 |
| Series X profile, the boat (one run) | about 60 ms (16 to 17 fps) |
| Opening cutscene of the boat save | median about 60 ms; a camera cut still drops to a few frames a second |
| Turning the camera while the title streams | 8 to 10 fps reported by the tester; being worked on |
| Memory | 18 to 20 GB for the process; 32 GB of RAM is tight |

It started at 12.7 to 12.9 fps in the cell the day before. The frame is bound by
the thread of ours that prepares submissions (about 42 ms of work a frame) and
by the card (about 37 ms of the title's own draws and dispatches); 30 real
frames a second are not reached.

The picture:

- Faces are formed right and their expressions move (two faults in the
  translation of the title's face network, see below).
- With the PC upscaler (FSR) the Series S picture is clean; without it the
  title's dithered effects stay as speckle.
- Triangle-edge lines on walls and panels are gone (quad reads in pixel
  shaders).
- A texture the title replaces keeps showing its old copy until the new one is
  uploaded, and a view with nothing to show yet reads mid grey instead of black.
- In a run with an empty cache, pipelines that are still being made are waited
  for by all recording threads together, so characters keep their heads.

### How it is run here

Set the FidelityFX SDK's `signedbin` folder as `ffx_dir` under `[paths]` in
`victory.toml` (see `victory.example.toml`), then:

```powershell
$env:VICTORY_INPLACE = "3"; $env:VICTORY_RELAY = "2"
$env:VICTORY_INDIRECT_WINDOW = "1"; $env:VICTORY_NO_PREV_CULL = "1"
$env:VICTORY_TEXTURE_GATE_S = "0"; $env:VICTORY_LATE_RUNS = "16384"
target\release\victory.exe run <game dir>\<the title's exe> --execute --console series-s
```

`VICTORY_FRAME_GEN=1` adds FSR frame generation (off by default; the interface
is interpolated with the picture, so subtitles may ghost). The first run after
an update translates every shader again and is slow; judge the second.

### Known problems

- The wall right of the window in the first cell is in shadow since quad reads
  are on (one lamp's shadow-map program; a fix is being checked).
  `VICTORY_QUAD_HELPERS=0` gives the old picture, with the edge lines.
- Skin of some characters turns red, black or to noise in cutscenes, and eyes
  can lose their iris. Suspected: character textures baked from inputs that had
  not arrived.
- Interiors that the sun does not reach are too dark: the title's ray-traced
  light does not run (4 of the structure builder's 53 programs do not
  translate).
- Clear glass shows a blurred orange smear instead of what is behind it; floors
  can look wet and lose their painted lettering; some runs have a pink cast on
  floors and ceilings.
- The scripted runs used for testing only leave the first cell when the stick
  is moved; that is the title waiting for the player, not a hang.
- Three unit tests of the shader crate fail (heap handles in `glc` programs).

### What it adds

Each is explained where it is in the code or in its commit.

Speed:
- Both per-frame settles removed; constant buffers of tables in slots of their
  own instead of a table made afresh for every draw (`VICTORY_CBV_SLOTS`);
  guards as bit sets; argument clamps in batches (`VICTORY_INDIRECT_BATCH`);
  shared recorder locks; lists with nothing recorded left out of their
  submissions; the texture pass capped at 40 ms a frame; staging buffers pooled.
- A sampling profiler (`VICTORY_PROFILE=<file>`) and per-pass card times
  (`VICTORY_CARD_TIMES=1`).
- Built and measured without a gain here, so off: submissions prepared by a
  thread of ours (`VICTORY_ASYNC_SUBMIT`), guards by the 4 KiB part
  (`VICTORY_GUARD_PAGE=4096`), textures placed in blocks made ahead
  (`VICTORY_TEXTURE_BLOCKS`).

Picture:
- The face network: a wave sum of 16-bit floats where some lanes have left is
  added as the console adds it, and 16-bit typed accesses through integer views
  keep their bits, also for views in tables.
- Quad reads in pixel shaders give what the console gives for lanes that are
  not running.
- `ffx_dir` in `victory.toml`; FSR frame generation.

RDNA4:
- Four compute programs that crash the driver's compiler
  (`amdxc64.dll+0x861b9f`) are left out.
- `VICTORY_NO_PREV_CULL=1` leaves out the two meshlet-culling programs
  (`fc5e66f8f873d3bb`, `0452fc2d2e2fc4c3`) in which the device was lost whenever
  submissions were sent ahead (victory-fork issue #3).

Earlier:
- Title view memory and big buffers used in place (`VICTORY_INPLACE=3`), from
  gmehru's `wip/radeon-8060s`.
- With `VICTORY_INDIRECT_WINDOW=1`, up to eight windows of indirect records stand
  side by side in the list's scratch buffer, of up to 256 records each.
- JoshuuDev's fix for raw DirectStorage reads under the guard over all views.
- From victory-fork's pull requests, as their authors wrote them: #92, #188 (with
  #32), #253, #452, #38, #144, #239, #175, #240, #255, #670, #467, #221, #384.

### Taken from other forks

Commits taken from elsewhere name the commit and its author.

- [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX):
  batched clamps, the submit thread, the face-network fixes, the quad-read
  forms, the guard rest.
- [PastiiJay/klbf-lab-beta](https://github.com/PastiiJay/klbf-lab-beta) (branch `klbf-amd`): FSR frame
  generation.
- [fxceless/victory-fork](https://github.com/fxceless/victory-fork),
  [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)
  and upstream [VIctory](https://github.com/Marvisak/VIctory): fixes as named
  in their commits here.

Developed with [Claude Code](https://claude.com/claude-code).

## victory-amd

This copy is a fork of
[fxceless/victory-fork](https://github.com/fxceless/victory-fork), itself a fork
of [VIctory](https://github.com/Marvisak/VIctory). It adds fixes found while
running on an AMD card, tested on one machine only: Windows 11, Radeon RX 7800 XT
16 GB (driver 32.0.32029.2010), Ryzen 5 7500F, 32 GB RAM.

State on that machine (2026-11-25, Series X profile): the prologue's cutscene and
the first room are drawn, with the player moving, at 2 to 3 frames per second.
Runs of 8 minutes mostly end without a device loss; one ended with
`DXGI_ERROR_DEVICE_HUNG` after about 5 and a half minutes, cause unknown. Lamps
were dark in one run. The first room's walls were missing in several runs; with the
guard over all the views of a page (below) they were there in two runs out of two,
too few to call it fixed. Some shots of the opening cutscene come out white.
Ray-traced light does not run.

What it changes over `victory-fork` (each is explained in a comment where it is):

- Texture views are told apart by the memory their mips really take, so textures
  streamed in part are not taken for one another and are uploaded from their first
  mip that is there.
- Views of placed textures the host lost sight of are put back
  (`VICTORY_TEXTURE_SWEEP`).
- Dispatches of more than 65,535 groups are made in two parts (the final gather at
  2560x1440 asks for 115,200).
- DirectStorage requests read their page array when they run, and wait for it.
- The list of pipelines to make early keeps what earlier runs listed, and compute
  pipelines get their own waiting time while a world loads.
- A write of the title's through one view of a physical page is seen by the copies
  taken through its other views (`VICTORY_VIEW_GUARD=0` turns it off).
- Textures picked by index are looked at again after their upload
  (`VICTORY_HEAP_RECHECK=0` turns it off).
- View indices past the host heap are clamped on AMD too.
- Programs that stop the AMD driver's compiler, or take it minutes and tens of GB,
  are left out (ray-tracing resolves).
- Diagnostics: `VICTORY_NULL_MAGENTA`, `victory warm`, more captures.

The guard over all the views of a page is code of
[victory-fork #115](https://github.com/fxceless/victory-fork/pull/115) by Spexivus
with [its correction](https://github.com/Spexivus/victory-fork/pull/1) by joroyt and
a part of [VIctory #1122](https://github.com/Marvisak/VIctory/pull/1122) by tightfall,
adapted here. The second look at textures is fxceless's idea (victory-fork `a803c87`),
written anew. Other parts were prompted by pull requests and issues of VIctory and
victory-fork, and their code was written anew. Developed with [Claude Code](https://claude.com/claude-code).

A work-in-progress fork of [VIctory](https://github.com/Marvisak/VIctory), a
compatibility layer that runs an Xbox Series title natively on Windows from
files the user supplies. This fork follows upstream
`main` and adds fixes aimed at one goal: getting the game's world to render
correctly and playably on one particular PC.

**This repository contains no game files, code or assets.** You need your own
copy of the game. It is not affiliated with or endorsed by the game's
developer or publisher.

## Credits

- **[VIctory](https://github.com/Marvisak/VIctory)** by Marvisak and its
  contributors is the base of everything here: the loader, the runtime
  services, the Direct3D 12 back end and the shader recompiler. Licensed
  GPL-3.0-or-later; so is this fork. Upstream's history is kept intact below
  the single commit that carries this fork's changes.
- **[XCL](https://github.com/ivall/xcl)** by ivall and its contributors
  (GPL-2.0-only) was used as a reference for how the same title behaves under
  another layer. Fixes inspired by it were written anew in VIctory's code.
- This fork's changes were developed with [Claude Code](https://claude.com/claude-code).

## Status of this fork (2026-11-25)

**Not playable yet: 2.5 to 4 frames per second, and shadowed areas are black.**
Tested on one machine: Windows 11, Intel i7-14700KF, NVIDIA RTX 4070 12 GB,
32 GB RAM. The Series X profile is used throughout.

| Stage | State in this fork |
|-------|--------------------|
| Boot, menus, start screen | works, 30 fps (as upstream) |
| Opening cutscene | plays from start to finish, in order, lit: title cards, credits cards, night and dusk shots |
| Saves | the game autosaves a checkpoint and loads it again |
| First gameplay scene (the prologue) | renders: characters, textures, sky, sunlit exterior; HUD, pause menu and pad input work |
| Light in shadow | missing: the game's ray-traced bounce light and ray-traced lamp and sun shadows do not run yet, so anything the sun does not hit directly is black |
| Frame rate in the world | 2-4 fps: 420-450 ms per frame by default, 240-350 ms with the newer switches (`VICTORY_RELAY=2`, or `VICTORY_LATE_WAIT=3 VICTORY_WIDE_BACK=1 VICTORY_CARD_BUFFERS_MIN_MB=1 VICTORY_CARD_BUFFERS_MB=1024`); it started at 3-6 seconds per frame |
| Stability | no driver hang or device loss in normal runs (a hang introduced by the wave64 update was found and fixed: its barrier form for 64-lane programs is now off unless `VICTORY_WAVE_WIDE=1`) |

### What this fork changed (summary)

Rendering:
- Meshlet culling read its scene buffers as the wrong kind of view, so the world drew almost nothing; fixed.
- Per-draw descriptor tables could run out, after which draws used slots never made for them and the card hung; fixed.
- Light packed by the game's compute shaders was read back through empty copies (sky, probes, fog); fixed for every mip level and layer.
- The fog reconstruction, shadow-mask, hair, blend-shape skinning, fire and several material programs did not translate; fixed.
- Gathers at a named mip level (with and without an offset) read level 0; fixed.
- Implicit-level samples outside pixel shaders produced invalid DXIL in 730 compute programs; fixed.
- Rectangle lists, depth and stencil twins for depth textures written by compute, texture read-backs.
- Ray tracing, in progress and off by default: the game's own structure-builder programs translate, title memory can be read by raw address from shaders, the hardware ray/box and ray/triangle test is emulated in DXIL (checked against a reference on the GPU), and 59 of the 64 trace programs translate. Nothing is lit by it yet. See [docs/fork-ray-tracing.md](docs/fork-ray-tracing.md).

Speed and memory:
- Waits for memory values, write-watch queries and page queries no longer walk page tables per call.
- Large title buffers get a copy in card memory, with shader writes found by hashing on the card.
- Submissions can be sent ahead of the wait for the card (`VICTORY_LATE_WAIT`), with a relay thread that lands results as the card finishes (`VICTORY_RELAY`): in a first five-minute run the card's idle time behind the submitting thread fell from about 136 to 30 ms per frame.
- Independent host-copy refreshes run in parallel by default on systems with at least four logical CPUs (`VICTORY_WIDE_REFRESH=0` disables this; `=1` forces it). This targets the serial memory reads that previously cost about 45–50 ms per frame on the test run.
- Title memory can be opened as a graphics heap so that buffers are used in place instead of copied (`VICTORY_INPLACE`, first stage only; a probe confirmed the approach on the test GPU).
- The bindless texture queue no longer uploads stale views; the view table no longer grows without bound; the pipeline library is mapped instead of read into memory.
- An intermittent device loss was traced to the pipeline library file being replaced while mapped; fixed.

- Translated shaders get clean-up passes over the finished module (dead code, constant folding, reuse of values, branch facts); the pack's host instruction count falls by about 43 %, worth about 20 ms of GPU time per frame on the test machine (`VICTORY_SHADER_OPT=0` turns them off).

Most of the newer speed changes sit behind environment switches that are off
by default (`VICTORY_LATE_WAIT`, `VICTORY_RELAY`, `VICTORY_WIDE_BACK`,
`VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_INPLACE`, `VICTORY_BVH_BUILDER`, ...).
Independent host-copy refreshes are the exception: they use parallel workers
automatically on systems with at least four logical CPUs. Each switch is
described in a comment where it is read.

### Known problems

- Everything in shadow is black (see above).
- The interior of the first gameplay room is sometimes not drawn after the cutscene hands over to gameplay.
- Around 24 GB of memory is in use during play.
- A few style lints are switched off for this fork in the workspace `Cargo.toml` until the newer code is tidied.
- Many changes here are specific to this title and this GPU, and some are marked `shortcut:` in the code where they approximate what the console does.

Everything below this line is upstream VIctory's own README, unchanged; its
status section describes upstream, not this fork.

---

# VIctory

Runs the title (Xbox Series X|S) natively on Windows, from files
the user supplies.

The title's x86-64 code runs unchanged inside our process; VIctory supplies
the platform underneath it and converts the title's data, ahead of time where
it can and on first use where it must (shaders). See
[docs/architecture.md](docs/architecture.md).

## Status (2026-11-23)

**Not playable yet.** The title boots, runs its menus at its own 30 fps and
loads the world, where rendering fails.

| Stage | State |
|-------|-------|
| Boot, first-boot screens (accessibility, language, settings, legal) | works, 30 fps |
| Intro video with sound | works |
| Start screen, profile and saves | works |
| Loading into the world (about 50 s) | works |
| The world (first mission's intro) | reached and runs with indirect draws; its title card renders, the scene after it is still dark; 0.5-1.8 s per frame |

Tested on one machine: Windows 11, AMD RX 6700 XT.

> **Warning.** Entering the world on a real GPU can currently hang it, and
> Windows then resets the display driver (every monitor goes black for a
> moment). To look further, use a diagnostics build with
> `VICTORY_D3D12_WARP=1`, which renders on the software adapter.

What each part does today and what it still needs:

| Subsystem | Today | Needs |
|-----------|-------|-------|
| Loader | Runs the title's executable and its DLLs in our process; crash reports name the faulting code and are kept in `crashes.log` | – |
| Runtime services | One signed-in offline user, licences, packages, saves that survive a kill at any step, text entry dialog | Speech is silent; the dialog and relaunch are untested live |
| Network | Offline by design: the title cannot reach the network | – |
| Files and streaming | Archives and streamed data read, zlib and BCPack decoded in parallel | Loading is limited by writes into title memory |
| Input | XInput pads, keyboard, keyboard stand-in pad | Trigger rumble |
| Audio | Menu, video and loading sound; XMA voices and Opus streams decode | Reverb is silent; Opus results unverified against the title |
| GPU: menus | Draws, compute and presentation on host D3D12; only changed title memory is copied | Frame pacing drifts on one screen in release builds |
| GPU: world | Mesh pipelines, shader-written buffers, descriptor lifetimes, indirect execution (arguments clamped), pipelines made by worker threads and kept on disk | Most of the scene is still missing (untranslated shaders drop draws); frames take seconds; real queries, depth bounds, HDR and scaling are missing |
| Shaders | Console GPU code recompiled to DXIL at first use, cached on disk. Menus: all. World: mesh 8,191 of 8,506, vertex/pixel 10,617 of 12,346, compute 1,643 of 2,539 | 64-bit image atomics, merged geometry stages, the ray-tracing builder's shaders; some compute programs need a GPU with 64-lane waves |
| Textures | Console tiled layouts and detiling for the modes the title uses | Render-target metadata planes |
| Series S mode | Runs through the menus to the loading screen | The world, as above |

## Getting started

Requirements: Windows 10/11 x64, a CPU with AVX2, a Direct3D 12 GPU with
shader model 6.6 (and mesh shaders for the world), Rust (pinned in
`rust-toolchain.toml`).

The title also needs several GiB of available Windows commit at startup.
Check **Committed** under **Task Manager → Performance → Memory**. If
`crashes.log` reports Windows error 1455, close other applications or increase
the Windows paging file size, then retry.

```powershell
cargo xtask install-hooks                # asset-policy check on every commit
copy victory.example.toml victory.toml   # then set your paths (outside the repo)
cargo test --workspace                   # needs no game data
cargo run -p victory -- config           # check your paths
```

Run the title:

```powershell
cargo build --release -p victory
target\release\victory.exe run <game dir>\<the title's exe> --execute
```

The window caption shows the framerate. Saves, settings and `crashes.log`
are kept under the data folder (default `%LOCALAPPDATA%\victory\data`), derived
data under the cache folder.

Input: XInput pads. With none connected the keyboard is a pad while the
title's window has focus:

| Keys | Pad |
|------|-----|
| W A S D / I J K L | left / right stick |
| arrows | D-pad |
| Enter, Backspace, X, Y | A, B, X, Y |
| Q / E, Z / C | shoulders, triggers |
| F / H | stick clicks |
| Esc, Tab | Menu, View |

Upscaler: the title uses AMD's FidelityFX upscaler (FSR) on Series S. By
default its upscale dispatch is answered with a plain bilinear stretch.
Point `VICTORY_FFX_DIR` at `Kits\FidelityFX\signedbin` of AMD's public
[FidelityFX SDK](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK)
(2.x) to answer its FidelityFX calls with the PC upscaler for D3D12 instead
(experimental; from PastiiJay/klbf-lab-beta).
`ffx_dir` under `[paths]` in `victory.toml` names the same folder without an
environment variable; `VICTORY_FFX_DIR` wins over it, and set to nothing it
turns the PC upscaler off.
`VICTORY_FFX_SIMPLE=1` takes the same calls with no upscaler: a bilinear
scale with one jitter phase, to tell what FSR adds to a picture.

## Commands

```
victory inspect <exe|dll> [--imports] [--exports]
victory census [paths...] [--missing] [--strict]
victory lib-exports <import.lib> [--list]
victory shader-info [paths...] [--verbose] [--disasm]
victory translate [paths...] [--validate] [--pipelines] [--list] [--jobs N]
victory run <exe> [--execute] [--unimplemented trap|zero] [--strict]
            [--console series-x|series-s] [-- <title arguments>]
victory config
```

Logging: `VICTORY_LOG=victory_loader=trace,info` (a release build logs
info and above).

## Debugging

A release build has no debugging aids. Build them in with

```powershell
cargo build --profile diag -p victory --features diagnostics   # target\diag\victory.exe
```

| Variable | Effect |
|----------|--------|
| `VICTORY_LOG=<target>=debug,...` | debug and trace output |
| `VICTORY_STACKS=20,60` | every thread's stack at those seconds |
| `VICTORY_WATCH=<rva>,...` | logs a title u32 when it changes |
| `VICTORY_KEYS=30:Enter,35:X` | presses keys at those seconds, focus or not |
| `VICTORY_CRASH_SAVE=<rva>:<len>,...` | saves title memory to the cache folder on a crash |
| `VICTORY_CAPTURE=<dir>` | saves presented frames (powers of two, or `VICTORY_CAPTURE_FRAMES=<list>`) |
| `VICTORY_DUMP_TARGETS=1` | with the two above, saves every render target of those frames |
| `VICTORY_D3D12_DEBUG=1` | host debug layer |
| `VICTORY_D3D12_GBV=1` | with the above, GPU-based validation (bad descriptor use reported, not faulted; slow) |
| `VICTORY_D3D12_DRED=1` | on a GPU failure, the allocation hit and the commands in flight |
| `VICTORY_PIPELINE_WAIT_MS=24` | reduce the per-frame wait for background pipeline creation from the default of 500 ms. A draw can be omitted until its pipeline is ready |
| `VICTORY_SKIP_PIPELINES=<hash>,...` | omit draws and dispatches, including indirect ones, whose pipeline contains a listed 16-digit shader hash |
| `VICTORY_GPU_TIMING=1` | with the above, each list of a submission run and timed on its own; slow lists logged with their slowest commands |
| `VICTORY_CARD_HASH_WIDE=0` | the passes that find what shaders wrote to copies in the card's memory hash with one thread per 64 KiB, as before |
| `VICTORY_D3D12_WARP=1` | host device on the software adapter (slow; cannot reset the driver) |
| `VICTORY_SHADER_LOOP_LIMIT=<n>` | translated shader loops stop after `n` iterations per lane instead of 2^20 (own shader cache entries); if a lower `n` makes world frames much faster, loops hitting the limit are what makes them slow |
| `VICTORY_AUDIO_CAPTURE=<file.wav>` | records what the title plays (our process only) |
| `VICTORY_XMA_DUMP=<dir>` | saves the first 32 XMA voices' context and input |
| `VICTORY_UPSCALER=<rva>[,<kind>]` | the title's upscaler picker returns `kind` (default 7, FidelityFX; 1.0.2.1: `7E93F0`); FidelityFX on Series X |
| `VICTORY_FFX_SCALE=<scale>[,<rva>]` | render scale of the title's FidelityFX path (`0.6667`: 1440p for 4K, `0.5`: 1080p; 1.0.2.1 default rva `28EF680`); Series X otherwise renders native 4K and overflows its memory plan |

Every build appends crashes and host GPU failures to `crashes.log` in the
title's data folder.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

