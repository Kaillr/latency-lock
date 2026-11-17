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
| Starts with Windows | no (add it to Startup yourself) | yes, on by default |
| Visible status | console / tray | tray icon color + menu |
| Dependencies | VC++ 2017 runtime | none (one small exe) |

## What it gets you

The floor is set by the **driver**, not by the tool. Measured on one machine:

| Output | Driver | Windows default | With LatencyLock |
|---|---|---|---|
| Onboard Realtek ALC892 | Microsoft inbox HD Audio | 10.00 ms | **2.67 ms** (128 frames @ 48 kHz) |
| USB headphone adapter (Realtek USB2.0) | Microsoft USB Audio 2.0 | 10.00 ms | 7.00 ms |
| NVIDIA HDMI/DP audio | NVIDIA | 10.00 ms | 10.00 ms (nothing lower offered) |

The USB adapter's 7 ms floor was the same at every sample rate from 44.1 to 384 kHz, so a higher sample rate does **not** lower latency. Microsoft's inbox HD Audio driver supports 128-sample buffers ([docs](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio)), which makes onboard audio the lowest-latency option on many PCs.

## Using it

Download `latency-lock.exe` from the [latest release](https://github.com/Kaillr/latency-lock/releases/latest) and run it. The first time, it installs itself for your Windows account (no admin prompt) in `%LOCALAPPDATA%\Programs\LatencyLock`, adds a Start menu shortcut and an entry in **Apps & features**, and starts with Windows from then on. After that you can delete the downloaded file; running it again just opens the installed copy, or upgrades it if the download is newer.

A headphones icon appears in the notification area. Its headband is a padlock shackle, with a keyhole between the earcups:

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
- **Start with Windows** (on from the first run)
- **Update automatically** (on by default, see [Updates](#updates))
- **Device details...** (min, default and current buffer of every output)
- **Open log** (`%LOCALAPPDATA%\LatencyLock\latency-lock.log`)
- **Check for updates...**

From a terminal:

```
latency-lock.exe --probe      # print every output's buffer limits
latency-lock.exe --portable   # run from where it is, without installing
latency-lock.exe --uninstall  # same as Uninstall in Apps & features
```

To uninstall, use **Settings > Apps > Installed apps > LatencyLock > Uninstall**. It removes the app, its shortcut, its settings, the Start with Windows entry and the log.

With `--portable`, nothing is installed and updates replace the exe where it is, so keep it in a folder you can write to (not Program Files).

If you hear crackling, your system can't keep up with the smallest buffer. Use **Pause**, or close CPU-heavy background apps.

## Updates

LatencyLock checks [GitHub Releases](https://github.com/Kaillr/latency-lock/releases) a minute after it starts and every 12 hours after that. When a newer version is out, it downloads it in the background, checks it against the SHA-256 published with the release, and puts it in place of `latency-lock.exe` on disk. The copy that's running is left alone, so nothing restarts or pops up: the new version runs the next time LatencyLock starts, usually your next sign-in. Downloads are noted in the log.

**Check for updates...** checks right away and, if it downloads a newer version, offers to restart into it. Untick **Update automatically** to stop the background checks. The updater uses what ships with Windows (WinHTTP for downloads, BCrypt for the hash), so it adds no dependencies.

## How it works

1. For each target output it calls `IAudioClient3::GetSharedModeEnginePeriod` to find the driver minimum.
2. It opens a shared-mode stream with `InitializeSharedAudioStream` at that period and starts it. It never writes audio, so the stream is silent and shows up in the Volume Mixer as *LatencyLock (silent, keeps latency low)*.
3. Every second it verifies the stream is alive and the engine still runs at the minimum. If a reset killed it (`AUDCLNT_E_DEVICE_INVALIDATED`) or the default device changed, it re-opens it.

Exclusive-mode and ASIO apps (DAWs) bypass the shared engine and aren't affected. Microphones are not handled: most USB mic drivers only offer 10 ms anyway.

## Building

Requires Rust with the MSVC toolchain. The build also compiles the icon and version info into the exe, using the Windows SDK that comes with it.

```
cargo build --release
# -> target\release\latency-lock.exe
```

Debug builds always run in place. A release build installs itself like the published exe, so run a test build with `--portable` to keep it from replacing your installed copy.

## Releasing

Every push and pull request runs the checks (rustfmt, clippy, tests) on Windows. Releases are made by hand: run the **Release** workflow from the Actions tab, or `gh workflow run release.yml`. It runs the same checks, then [semantic-release](https://semantic-release.gitbook.io/) reads the commits since the last release and, if any of them call for one, bumps the version in `Cargo.toml`, tags `vX.Y.Z`, and publishes `latency-lock.exe` with its `latency-lock.exe.sha256` on the release. Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

| Commit message | Release |
|---|---|
| `fix: ...`, `perf: ...` | patch (1.0.0 -> 1.0.1) |
| `feat: ...` | minor (1.0.0 -> 1.1.0) |
| `feat!: ...`, or a `BREAKING CHANGE:` footer | major (1.0.0 -> 2.0.0) |
| `docs:`, `ci:`, `chore:`, `refactor:`, `test:`, ... | none |

## License

MIT, see [LICENSE](LICENSE).
