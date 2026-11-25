# oxidisplay

An unofficial, clean-room, **zero-dependency pure Rust** Windows Indirect Display Driver (**UMDF 2.33 + IddCx 1.10**) for **DisplayLink DL-1x5 (USB 2.0)** displays.

DisplayLink DL-1x5（USB 2.0）チップセット搭載ディスプレイ向けに、**外部クレート依存ゼロの純Rust**で一から実装した非公式 Windows 間接ディスプレイドライバ（**UMDF 2.33 + IddCx 1.10**）です。

Tested on **Wacom DTU-1031** (`USB\VID_17E9&PID_04A3`, 1280×800 @ 60 Hz) on Windows 11 (x64).

---

## Highlights / 主な特徴

- **Up to ~60 Hz Frame Pacing on USB 2.0**: Achieves **`p50 = 16.7–16.8 ms`** (`47.9–56.7 FPS` sustained on full-screen 1280×800 workloads) — **+49% to +76% higher frame rate** than the official DisplayLink driver (`32.1–32.3 FPS`, `p50 = 31.1–31.3 ms`) while using **50% less CPU** and **21% less USB traffic** on the same hardware.
- **Zero Per-Frame Allocations & Leak-Free**: Pre-allocated shadow framebuffer, pre-allocated 12 × 128 KiB asynchronous USB Bulk OUT URB ring (`WdfRequestReuse`), and fixed command scratch buffer eliminate runtime heap churn and memory leaks.
- **Zero External Crate Dependencies**: Links directly against Windows WDK (`WdfDriverStubUm.lib`, `iddcxstub.lib`), `d3d11.lib`, `dxgi.lib`, `avrt.lib`, and `ntdll.lib` with compile-time ABI `size_of` assertions.
- **Tiny Footprint**: `dl_rust_idd.dll` is **~241 KB** (vs. **10.6 MB** for official `dlidusb4.dll`), using **30% less Working Set** and **31% fewer kernel handles**.
- **Low-Latency Pipeline**:
  - **Double-buffered ping-pong D3D11 staging textures** (non-blocking `CopyResource` overlapped with CPU encode of the previous frame) + early `IddCxSwapChainFinishedProcessingFrame`.
  - **Runtime-dispatched AVX2 / SSE2 SIMD** BGRA→RGB565 conversion and shadow-buffer diff scanning.
  - **Hardware `WRITECOPY16` (`0xAF 0x6A`)** VRAM blit acceleration for IddCx move regions (scrolling/window drag).
  - **3-tier adaptive quantization** (`Lossless` → `MotionFast` RGB454 + temporal deadzone → `MotionUltra` RGB444 + 2 px RLE smoothing + alternating scanlines) with **automatic lossless static refinement** once motion settles.
  - **MMCSS (`DisplayPostProcessing`)** + `IddCxSetRealtimeGPUPriority`.

---

## Benchmark: Official Driver vs. `oxidisplay` `v1.0.0.0`

Measured head-to-head on **Wacom DTU-1031 (full-screen 1280×800 @ 60 Hz, Per-Monitor-V2 DPI-aware)** using [`dl-bench`](crates/dl-probe/src/bin/dl-bench.rs) (18-second standardized 60 Hz D3D11 Flip-Model workload across 4 phases) and Windows kernel ETW (`Microsoft-Windows-IndirectDisplays-ClassExtension-Events`, Event ID 25):

| Metric | Official DisplayLink (`11.2.3146.0`) | **`oxidisplay` `v1.0.0.0` (AVX2 + Ping-Pong)** | vs. Official |
| --- | ---: | ---: | ---: |
| **Total IddCx Frames (18s)** | 603 | **957** | **+58.7%** |
| **Phase 1: Idle (0–3s)** | 32.2 FPS (`p50 31.2 ms`) | **56.7 FPS (`p50 16.7 ms`)** | **+76%** |
| **Phase 2: UI Scroll (3–8s)** | 32.1 FPS (`p50 31.3 ms`) | **51.1 FPS (`p50 16.8 ms`)** | **+59%** |
| **Phase 3: Video Box (8–13s)** | 32.3 FPS (`p50 31.2 ms`) | **51.3 FPS (`p50 16.8 ms`)** | **+59%** |
| **Phase 4: Full Motion (13–18s)** | 32.1 FPS (`p50 31.1 ms`) | **47.9 FPS (`p50 16.8 ms`)** | **+49%** |
| **Driver Host CPU (12 logical CPUs)** | Avg 1.35% (1-core: 16.2%) | **Avg 0.67% (1-core: 8.0%)** | **-50% CPU** |
| **USB Bulk Traffic (`IO Other`)** | 407.3 MB (`26.16 MB/s`) | **323.0 MB (`20.73 MB/s`)** | **-21% bytes** |
| **USB IOCTL Rate** | 477 IOCTLs/s | **196 IOCTLs/s** | **-59% IOCTLs** |
| **Driver Binary Size** | 10,820 KB | **241 KB** | **-97.8%** |
| **Host Working Set (Delta)** | 71.6 MB (`0.0 MB`) | **50.4 MB (`0.0 MB`)** | **-30%** |
| **Host Private Memory (Delta)** | 85.3 MB (`0.0 MB`) | **67.1 MB (`0.0 MB`)** | **-21%** |
| **Host Kernel Handles (Delta)** | 728 (`0`) | **503 (`0`)** | **-31%** |

> **Why is CPU usage only ~0.67%?**  
> On Windows, **DWM and applications still render on your GPU** (Direct3D / DXGI). The DisplayLink driver only reads back dirty regions, converts BGRA→RGB565/RLE using AVX2 SIMD (taking `0.07–0.93 ms` per frame for typical desktop/video frames), and streams Bulk URBs over USB.

---

## Architecture / アーキテクチャ

```mermaid
flowchart LR
    DWM["Windows DWM / GPU<br/>(IddCx 1.10 SwapChain)"] -->|"AcquireBuffer<br/>+ Dirty/Move Rects"| D3D["D3D11 Ping-Pong Staging<br/>(staging[0] / staging[1])"]
    D3D -->|"Non-blocking CopyResource<br/>(Previous Frame Mapped)"| ENC["dl-protocol FrameEncoder<br/>(AVX2 RGB565 + WRITECOPY16<br/>+ Adaptive RLE)"]
    ENC -->|"0xAF Bulk Command Stream"| USB["12 × 128 KiB Async URB Ring<br/>(WDF WinUSB Bulk OUT EP 0x01)"]
    USB -->|"USB 2.0 High-Speed"| HW["DisplayLink DL-1x5<br/>(e.g. Wacom DTU-1031)"]
```

### Workspace Crates

| Path | Description |
| --- | --- |
| [`crates/dl-protocol`](crates/dl-protocol) | Clean-room DL-1x5 protocol library: `0xAF` register/mode-set/blank commands, LFSR16 timing encoder, EDID 1.3 parser, AVX2/SSE2 RGB565 & diff encoder, `WRITECOPY16` move-region blitter, adaptive RLE encoder, and unit tests with a software VRAM decoder simulator. |
| [`crates/dl-idd`](crates/dl-idd) | Native Windows UMDF 2.33 + IddCx 1.10 driver (`dl_rust_idd.dll`). Handles WDF PnP/Power, WinUSB Bulk OUT async ring, D3D11 ping-pong readback, MMCSS realtime thread pump, and WDK ABI validation. |
| [`crates/dl-probe`](crates/dl-probe) | Developer utilities: `dl-probe` (offline AVX2 compression & EDID/LFSR16 benchmark) and `dl-bench` (60 Hz 4-phase D3D11 workload generator for ETW A/B benchmarking). |
| [`driver/dl_rust_idd.inf`](driver/dl_rust_idd.inf) | Windows driver INF (`UmdfLibraryVersion = 2.33.0`, `UmdfExtensions = IddCx0102`, `UpperFilters = IndirectKmd`, `UmdfDispatcher = NativeUSB`). |
| [`scripts/`](scripts) | PowerShell scripts to build/test-sign (`package_and_sign.ps1`), install (`install_driver.ps1`), roll back to the official driver (`rollback_driver.ps1`), and run ETW A/B benchmarks (`run_ab_benchmark.ps1`). |

---

## Prerequisites / ビルド・動作環境

- **OS**: Windows 10/11 x64
- **Rust**: `rustup` with `x86_64-pc-windows-msvc` toolchain (tested with `rustc 1.100.0`)
- **Visual Studio Build Tools / MSVC** (C++ desktop toolchain)
- **Windows Driver Kit (WDK) 10** (tested with `10.0.28000.0`, requires UMDF 2.33, IddCx 1.10, `Inf2Cat.exe`, `signtool.exe`)
- **Test Signing enabled** (required to load self-signed UMDF/IddCx drivers on Windows):
  ```powershell
  # Run in an elevated PowerShell, then reboot (Secure Boot must be disabled in UEFI)
  bcdedit /set testsigning on
  ```

---

## Build, Sign & Install / ビルド・署名・インストール

### 1. Run Unit Tests & Offline Encoder Benchmark

```powershell
cargo test --workspace
cargo run --release -p dl-probe
```

### 2. Build & Test-Sign Driver Package

Creates a local self-signed test certificate (stored in gitignored `dist/cert/`), builds `dl_rust_idd.dll` in release mode, generates `dl_rust_idd.cat` via `Inf2Cat`, and Authenticode-signs both files into `dist/`:

```powershell
.\scripts\package_and_sign.ps1
```

### 3. Install Driver (Administrator PowerShell)

Trusts the generated test certificate in `Root` & `TrustedPublisher` and installs `dist/dl_rust_idd.inf` via `pnputil`:

```powershell
.\scripts\install_driver.ps1
```

### 4. Roll Back to Official DisplayLink Driver (Administrator PowerShell)

Cleanly uninstalls the Rust driver and restores the official DisplayLink INF if present in the Driver Store:

```powershell
.\scripts\rollback_driver.ps1
```

---

## Supporting Other DisplayLink DL-1x5 Devices / 他の DL-1x5 デバイスへの対応

By default, [`driver/dl_rust_idd.inf`](driver/dl_rust_idd.inf) binds to Wacom DTU-1031 (`USB\VID_17E9&PID_04A3`).  
To add another DL-1x5 USB 2.0 display (e.g. DL-125 / DL-165 / DL-195), add its Hardware ID under `[Standard.NTamd64.10.0...22621]` in `driver/dl_rust_idd.inf`, re-run `.\scripts\package_and_sign.ps1`, and install:

```ini
%DeviceName% = DlRustIdd_Install, USB\VID_17E9&PID_XXXX
```

> **Note**: This driver targets **DL-1x5 (USB 2.0)** devices (`0xAF` bulk protocol). Newer DL-3xxx/4xxx/5xxx/6xxx (USB 3.0) chipsets use a completely different protocol and are not supported.

---

## Running the ETW A/B Benchmark / ベンチマーク実行

To automatically locate the connected 1280×800 monitor (DPI-aware) and run the head-to-head A/B benchmark measuring real IddCx frame cadence, USB Bulk bandwidth, CPU usage, and memory/handle footprint (Administrator PowerShell):

```powershell
.\scripts\run_ab_benchmark.ps1
```

---

## Optional Debug Logging / デバッグログ

In release builds, `dl_rust_idd.dll` **does not write any files to disk** by default (zero disk I/O overhead).  
To enable diagnostic file logging to `C:\Users\Public\dl_rust_idd.log`, create the empty sentinel file before connecting/restarting the device:

```powershell
New-Item -ItemType File -Path C:\Users\Public\dl_rust_idd.debug -Force
```

Delete the sentinel file to disable disk logging again. Live trace messages are always available via `OutputDebugStringA` (e.g. Sysinternals DebugView).

---

## Clean-Room Notice, Trademark Disclaimer & License

The DL-1x5 wire protocol implementation in `dl-protocol` is a clean-room Rust implementation based on publicly documented open-source specifications (such as Linux `udl` / `udlfb` / `libdlo`). No proprietary DisplayLink binaries or Huffman tables are included.

> **Trademark Disclaimer**: *DisplayLink* is a registered trademark of Synaptics Incorporated. *Wacom* and *DTU-1031* are trademarks of Wacom Co., Ltd. `oxidisplay` is an independent, unofficial open-source project and is not affiliated with, endorsed by, or supported by Synaptics or Wacom.

Licensed under the **BSD Zero Clause License (`0BSD`)** — see [LICENSE](LICENSE). You may use, copy, modify, and/or distribute this software for any purpose with or without fee, with no attribution required.
