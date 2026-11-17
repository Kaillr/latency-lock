# LatencyLock

A tiny Windows tray app that keeps the audio engine at the **smallest buffer your audio driver supports**, and **puts it back automatically** whenever something resets it.

By default Windows mixes all shared-mode audio (games, Discord, browsers) in 10 ms chunks. Since Windows 10, an app can ask the engine to run at a smaller period if the driver allows it, and while that request is active **every** app on that output gets the lower latency. LatencyLock holds that request for you, all the time.

## Why not REAL?

[REAL](https://github.com/miniant-git/REAL) does the same basic trick, but if anything resets the device it silently stops working and stays at 10 ms until you restart it. Resets include changing the sample rate, unplugging a USB DAC, sleep and wake, a driver update, or the audio service restarting. LatencyLock checks its hold every second and re-applies it within about a second of any reset.

| | REAL | LatencyLock |
|---|---|---|
| Lowest buffer the driver allows | yes | yes |
| Recovers after device resets | no | yes, logged |
| Follows default-device changes | no | yes |
| All outputs at once | no | optional |
| Visible status | console / tray | tray icon color + menu |
| Dependencies | VC++ 2017 runtime | none (single ~220 KB exe) |

## What it gets you

The floor is set by the **driver**, not by the tool. Measured on one machine:

| Output | Driver | Windows default | With LatencyLock |
|---|---|---|---|
| Onboard Realtek ALC892 | Microsoft inbox HD Audio | 10.00 ms | **2.67 ms** (128 frames @ 48 kHz) |
| USB headphone adapter (Realtek USB2.0) | Microsoft USB Audio 2.0 | 10.00 ms | 7.00 ms |
| NVIDIA HDMI/DP audio | NVIDIA | 10.00 ms | 10.00 ms (nothing lower offered) |

The USB adapter's 7 ms floor was the same at every sample rate from 44.1 to 384 kHz, so a higher sample rate does **not** lower latency. Microsoft's inbox HD Audio driver supports 128-sample buffers ([docs](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio)), which makes onboard audio the lowest-latency option on many PCs.

## Using it

Download `latency-lock.exe` from the [latest release](https://github.com/Kaillr/latency-lock/releases/latest) and run it. A headphones icon appears in the notification area. Its headband is a padlock shackle, with a keyhole between the earcups:

| Icon | Meaning |
|---|---|
| green | holding the minimum buffer |
| amber | something prevents it (error, or another app fixed a different period) |
| grey | nothing to do (no device, or the driver offers nothing below 10 ms) |
| grey, headband lifted off the right earcup | paused |

Right-click (or left-click) the icon for:

- the current buffer per device, and how many resets it has recovered from
- **Default output only** / **All outputs**
- **Pause**, which hands control back to Windows defaults
- **Start with Windows**
- **Install updates automatically** (on by default, see [Updates](#updates))
- **Device details...** (min, default and current buffer of every output)
- **Open log** (`%LOCALAPPDATA%\LatencyLock\latency-lock.log`)
- **Check for updates...**

From a terminal:

```
latency-lock.exe --probe     # print every output's buffer limits
```

If you hear crackling, your system can't keep up with the smallest buffer. Use **Pause**, or close CPU-heavy background apps.

## Updates

LatencyLock checks [GitHub Releases](https://github.com/Kaillr/latency-lock/releases) a minute after it starts and every 12 hours after that. When a newer version is out, it downloads it, checks it against the SHA-256 published with the release, puts it in place of the running exe and restarts. The audio engine is at the Windows default for about a second while that happens. While paused, updates wait until you resume. Updates are noted in the log.

Untick **Install updates automatically** to turn this off; **Check for updates...** still works and asks before installing. The updater uses what ships with Windows (WinHTTP for downloads, BCrypt for the hash), so it adds no dependencies. Keep the exe in a folder you can write to (not Program Files), or updates can't be installed.

## How it works

1. For each target output it calls `IAudioClient3::GetSharedModeEnginePeriod` to find the driver minimum.
2. It opens a shared-mode stream with `InitializeSharedAudioStream` at that period and starts it. It never writes audio, so the stream is silent and shows up in the Volume Mixer as *LatencyLock (silent, keeps latency low)*.
3. Every second it verifies the stream is alive and the engine still runs at the minimum. If a reset killed it (`AUDCLNT_E_DEVICE_INVALIDATED`) or the default device changed, it re-opens it.

Exclusive-mode and ASIO apps (DAWs) bypass the shared engine and aren't affected. Microphones are not handled: most USB mic drivers only offer 10 ms anyway.

## Building

Requires Rust with the MSVC toolchain.

```
cargo build --release
# -> target\release\latency-lock.exe
```
