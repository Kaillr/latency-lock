<p align="center">
  <img src="assets/logo.png" width="112" alt="LatencyLock logo: headphones whose headband is a padlock shackle">
</p>

<h1 align="center">LatencyLock</h1>

<p align="center">
  Lower audio latency on Windows, for every game and app, and it keeps working.
</p>

<p align="center">
  <a href="https://github.com/Kaillr/latency-lock/releases/latest"><img src="https://img.shields.io/github/v/release/Kaillr/latency-lock" alt="Latest release"></a>
  <a href="https://github.com/Kaillr/latency-lock/releases"><img src="https://img.shields.io/github/downloads/Kaillr/latency-lock/total" alt="Downloads"></a>
  <a href="https://github.com/Kaillr/latency-lock/actions/workflows/ci.yml"><img src="https://github.com/Kaillr/latency-lock/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Kaillr/latency-lock" alt="MIT license"></a>
</p>

<p align="center">
  <a href="https://github.com/Kaillr/latency-lock/releases/latest/download/latency-lock.exe"><b>Download latency-lock.exe</b></a>
  &nbsp;·&nbsp; Windows 10 and 11, 64-bit
</p>

---

Windows mixes all sound in 10 ms chunks by default, even when your audio driver can go lower. LatencyLock asks Windows for the smallest chunk your driver supports and holds it, so everything playing through that output gets the lower latency: games, Discord, your browser.

It's the same trick as [REAL](https://github.com/miniant-git/REAL), but REAL stops working without telling you when your audio device resets. LatencyLock notices within a second and puts it back.

## Features

- **The lowest latency your driver allows**, for every app on that output, not just one game.
- **Keeps working** after sleep, unplugging a headset, sample rate changes, driver updates and audio service restarts.
- **Follows your default output** when you switch between speakers and headphones, or covers every output at once.
- **Set and forget.** It starts with Windows and sits in the tray, where the icon's color tells you it's working.
- **Updates itself** quietly in the background. A new version runs the next time it starts.
- **Lightweight.** One small exe with no runtime to install and no admin rights needed. Uninstall it from Apps & features.

## What to expect

Your audio driver sets the limit, not LatencyLock. On one PC:

| Output | Windows default | With LatencyLock |
|---|---|---|
| Onboard Realtek, Microsoft's built-in HD Audio driver | 10 ms | **2.67 ms** |
| USB headphone adapter | 10 ms | 7 ms |
| NVIDIA HDMI / DisplayPort audio | 10 ms | 10 ms (the driver offers nothing lower) |

So it saves a few milliseconds. In rhythm games you'll mostly notice hitsounds feeling snappier, and you may want to redo your offset. A few more things worth knowing:

- Raising the sample rate doesn't lower the limit. The USB adapter above stayed at 7 ms at every rate from 44.1 to 384 kHz.
- Onboard audio on Microsoft's built-in driver is often the fastest option, since it [supports buffers that small](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio).
- Apps that use ASIO or exclusive mode bypass the Windows mixer, so they're unaffected. That includes most DAWs.
- osu!lazer's new audio mode, which is now its default, already asks Windows for the lowest buffer itself. For osu!, LatencyLock mostly helps stable.

To see what your own outputs support, open **Device details...** from the tray menu.

## Getting started

1. Download [`latency-lock.exe`](https://github.com/Kaillr/latency-lock/releases/latest/download/latency-lock.exe).
2. Run it. Windows SmartScreen will probably warn you, because the exe isn't code signed yet. Click **More info**, then **Run anyway**.
3. Look for the headphones icon in the notification area (it may be hidden under the **^** arrow).

That's it. It installs itself for your Windows account, starts with Windows from now on, and you can delete the file you downloaded.

## The tray icon

| Icon | Meaning |
|---|---|
| Green | Holding the lowest buffer |
| Amber | Something is in the way (an error, or another app set a different buffer) |
| Grey | Nothing to do (no output device, or the driver has nothing below 10 ms) |
| Grey, headband lifted | Paused |

Click it for the current buffer of each output and these options:

- **Default output only** or **All outputs**
- **Pause**, which hands control back to Windows
- **Start with Windows**
- **Update automatically**
- **Device details...**, **Open log** and **Check for updates...**

## LatencyLock vs REAL

| | REAL | LatencyLock |
|---|---|---|
| Lowest buffer the driver allows | Yes | Yes |
| Recovers after the device resets | No, stays at 10 ms until restarted | Yes, within a second |
| Follows a change of default output | No, sticks to the device it started with | Yes |
| All outputs at once | No | Optional |
| Starts with Windows | No, add it to Startup yourself | Yes |
| Shows whether it's still working | No | Tray icon color |
| Needs | Visual C++ 2017 runtime | Nothing |
| Last release | 2019 | 2026 |

## FAQ

<details>
<summary><b>I hear crackling or pops</b></summary>

Your PC can't keep up with the smallest buffer. Click **Pause**, or close CPU-heavy apps running in the background.
</details>

<details>
<summary><b>Which apps get lower latency?</b></summary>

Every app playing through an output LatencyLock is holding, as long as it uses the normal Windows mixer (shared mode). That's almost every game, browser and voice chat app.
</details>

<details>
<summary><b>Does it connect to the internet?</b></summary>

Only to GitHub, to check for and download updates. Nothing else is sent anywhere. Untick **Update automatically** to stop even that.
</details>

<details>
<summary><b>How do updates work?</b></summary>

A minute after it starts, and every 12 hours after that, it checks GitHub for a new release. If there is one, it downloads it, checks it against the SHA-256 published with the release, and swaps it in. The running copy is left alone, so the new version takes over the next time LatencyLock starts, usually your next sign-in. **Check for updates...** does this right away and offers to restart.
</details>

<details>
<summary><b>Where does it install, and how do I uninstall it?</b></summary>

It installs to `%LOCALAPPDATA%\Programs\LatencyLock`, with a Start menu shortcut. To remove it, go to **Settings > Apps > Installed apps > LatencyLock > Uninstall**. That removes the app, its shortcut, its settings, the startup entry and its log.
</details>

<details>
<summary><b>Can I run it without installing?</b></summary>

Yes. Start it with `latency-lock.exe --portable` and it runs from where it is. Updates then replace that exe, so keep it in a folder you can write to.
</details>

<details>
<summary><b>Does it handle microphones?</b></summary>

No. Most USB microphone drivers only offer 10 ms anyway.
</details>

## How it works

1. For each output it's holding, it asks Windows (`IAudioClient3::GetSharedModeEnginePeriod`) for the smallest period the driver supports.
2. It opens a silent shared-mode stream at that period (`InitializeSharedAudioStream`). While that stream is open, Windows runs the whole output at the smaller period. The stream appears in the Volume Mixer as *LatencyLock (silent, keeps latency low)*.
3. Every second it checks the stream is still alive and the period is still the smallest. If a device reset killed the stream, or the default output changed, it opens a new one.

Settings live in `HKEY_CURRENT_USER\Software\LatencyLock` and the log in `%LOCALAPPDATA%\LatencyLock`.

## Building from source

You need Rust with the MSVC toolchain. The build also compiles the icon and version info into the exe, using the Windows SDK that comes with the toolchain.

```
cargo build --release
```

Debug builds always run in place. A release build installs itself like the published exe, so start a test build with `--portable` to keep it from replacing your installed copy. `latency-lock.exe --probe` prints every output's buffer limits to the terminal.

<details>
<summary><b>How releases are made</b></summary>

Every push and pull request runs rustfmt, clippy and the tests on Windows. Releases are cut by hand: run the **Release** workflow from the Actions tab, or `gh workflow run release.yml`. [semantic-release](https://semantic-release.gitbook.io/) picks the version from the [Conventional Commits](https://www.conventionalcommits.org/) since the last release, bumps `Cargo.toml`, tags `vX.Y.Z`, and publishes `latency-lock.exe` with its `.sha256`.

| Commit message | Release |
|---|---|
| `fix: ...`, `perf: ...` | Patch (1.0.0 → 1.0.1) |
| `feat: ...` | Minor (1.0.0 → 1.1.0) |
| `feat!: ...`, or a `BREAKING CHANGE:` footer | Major (1.0.0 → 2.0.0) |
| `docs:`, `ci:`, `chore:`, `refactor:`, `test:` | None |
</details>

## License

[MIT](LICENSE). If LatencyLock is useful to you, a ⭐ on GitHub helps other people find it.
