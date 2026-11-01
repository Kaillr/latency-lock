//! Holds the shared-mode audio engine at the smallest period the driver supports.
//!
//! How it works: Windows runs every shared-mode endpoint at one engine period (10 ms by
//! default). Since Windows 10, `IAudioClient3::InitializeSharedAudioStream` lets one client
//! ask for a smaller period, and while that stream is running *every* app on the endpoint
//! is serviced at that period. We open such a stream at the driver minimum, start it, never
//! write to it (the engine mixes nothing from it), and check every tick that it still
//! holds: device resets (format change, unplug/replug, audio service restart) silently
//! kill the stream, which is exactly what REAL does not recover from.

use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_ENGINE_PERIODICITY_LOCKED,
    AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE,
    IAudioClient3, IAudioSessionControl, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVEFORMATEX, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{Error, HRESULT, PCWSTR, Result, w};

use crate::log;

/// HRESULT_FROM_WIN32(ERROR_NOT_FOUND): returned when there is no default device at all.
const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    DefaultOutput,
    AllOutputs,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// We hold the engine at the driver minimum.
    Locked,
    /// The driver offers nothing below its default period.
    NothingToLock,
    /// Another low-latency app already fixed the engine at a different period.
    LockedByOther,
    Failed(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub rate: u32,
    pub min: u32,
    pub default: u32,
    pub max: u32,
    pub fundamental: u32,
    pub current: u32,
    pub state: State,
}

impl DeviceStatus {
    fn new(id: &str, name: &str, is_default: bool) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            is_default,
            rate: 0,
            min: 0,
            default: 0,
            max: 0,
            fundamental: 0,
            current: 0,
            state: State::Failed(String::new()),
        }
    }

    pub fn ms(&self, frames: u32) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            frames as f64 * 1000.0 / self.rate as f64
        }
    }
}

/// Human-readable HRESULT for the log and the menu.
pub fn hr_text(e: &Error) -> String {
    let code = e.code();
    let what = if code == AUDCLNT_E_DEVICE_INVALIDATED {
        " (device was reset or removed)"
    } else if code == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED {
        " (engine period fixed by another app)"
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
        " (unsupported format)"
    } else if code == AUDCLNT_E_DEVICE_IN_USE {
        " (device in exclusive use)"
    } else {
        ""
    };
    format!("0x{:08X}{what}", code.0 as u32)
}

/// Frees a WAVEFORMATEX returned by the audio APIs.
struct Format(*mut WAVEFORMATEX);
impl Drop for Format {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
    }
}

fn device_id(d: &IMMDevice) -> Result<String> {
    unsafe {
        let p = d.GetId()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(s)
    }
}

fn device_name(d: &IMMDevice) -> String {
    unsafe {
        d.OpenPropertyStore(STGM_READ)
            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_else(|_| "Unknown device".to_owned())
    }
}

fn current_period(client: &IAudioClient3) -> Result<u32> {
    unsafe {
        let mut fmt = null_mut();
        let mut cur = 0;
        client.GetCurrentSharedModeEnginePeriod(&mut fmt, &mut cur)?;
        drop(Format(fmt));
        Ok(cur)
    }
}

/// Fill rate and period limits of `client`'s mix format into `s`; returns the mix format.
fn read_caps(client: &IAudioClient3, s: &mut DeviceStatus) -> Result<Format> {
    unsafe {
        let fmt = Format(client.GetMixFormat()?);
        s.rate = (*fmt.0).nSamplesPerSec;
        client.GetSharedModeEnginePeriod(
            fmt.0,
            &mut s.default,
            &mut s.fundamental,
            &mut s.min,
            &mut s.max,
        )?;
        s.current = current_period(client).unwrap_or(s.default);
        Ok(fmt)
    }
}

/// A running low-latency stream we own. Dropping it stops the stream and frees the event.
struct Held {
    client: Option<IAudioClient3>,
    event: HANDLE,
    status: DeviceStatus,
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe {
            if let Some(c) = self.client.take() {
                let _ = c.Stop();
            }
            let _ = CloseHandle(self.event);
        }
    }
}

impl Held {
    /// Ok(current period) if the hold is still effective, Err(reason) if it was lost.
    fn verify(&self) -> std::result::Result<u32, String> {
        let c = self.client.as_ref().expect("held stream has a client");
        unsafe { c.GetCurrentPadding() }.map_err(|e| hr_text(&e))?;
        let cur = current_period(c).map_err(|e| hr_text(&e))?;
        if cur != self.status.min {
            return Err(format!(
                "engine period is {cur} frames instead of {}",
                self.status.min
            ));
        }
        Ok(cur)
    }
}

struct Target {
    id: String,
    device: IMMDevice,
    is_default: bool,
}

pub struct Engine {
    enumerator: Option<IMMDeviceEnumerator>,
    held: Vec<Held>,
    /// Devices we could not lock, and when to try them again.
    parked: HashMap<String, (Instant, DeviceStatus)>,
    names: HashMap<String, String>,
    last_state: HashMap<String, State>,
    audio_down: bool,
    /// How many times a lost hold was re-applied since start.
    pub recoveries: u32,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            enumerator: None,
            held: Vec::new(),
            parked: HashMap::new(),
            names: HashMap::new(),
            last_state: HashMap::new(),
            audio_down: false,
            recoveries: 0,
        }
    }

    fn enumerator(&mut self) -> Result<IMMDeviceEnumerator> {
        if let Some(e) = &self.enumerator {
            return Ok(e.clone());
        }
        let e: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        self.enumerator = Some(e.clone());
        Ok(e)
    }

    fn default_device(en: &IMMDeviceEnumerator) -> Result<Option<IMMDevice>> {
        match unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole) } {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.code() == E_NOTFOUND => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn targets(&mut self, mode: Mode) -> Result<Vec<Target>> {
        let en = self.enumerator()?;
        // Enumerating also serves as a liveness check of the enumerator itself
        // (it dies with the audio service and must then be recreated).
        let collection = unsafe { en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)? };
        let default = Self::default_device(&en)?;
        let default_id = default.as_ref().map(device_id).transpose()?;

        let mut out = Vec::new();
        match mode {
            Mode::DefaultOutput => {
                if let (Some(device), Some(id)) = (default, default_id) {
                    out.push(Target {
                        id,
                        device,
                        is_default: true,
                    });
                }
            }
            Mode::AllOutputs => unsafe {
                for i in 0..collection.GetCount()? {
                    let device = collection.Item(i)?;
                    let id = device_id(&device)?;
                    let is_default = default_id.as_deref() == Some(id.as_str());
                    out.push(Target {
                        id,
                        device,
                        is_default,
                    });
                }
            },
        }
        Ok(out)
    }

    /// One pass: make sure every target device is held at its minimum period.
    pub fn tick(&mut self, mode: Mode, paused: bool) -> Vec<DeviceStatus> {
        if paused {
            if !self.held.is_empty() {
                log::info("Paused: released all holds, Windows default buffers apply");
                self.held.clear();
                self.last_state.clear();
            }
            return Vec::new();
        }

        let targets = match self.targets(mode) {
            Ok(t) => {
                if self.audio_down {
                    log::info("Audio system is back");
                    self.audio_down = false;
                }
                t
            }
            Err(e) => {
                if !self.audio_down {
                    log::info(format!(
                        "Audio system unavailable: {} - retrying",
                        hr_text(&e)
                    ));
                    self.audio_down = true;
                }
                self.enumerator = None;
                self.held.clear();
                self.last_state.clear();
                return Vec::new();
            }
        };

        // Let go of devices that are no longer targets (default changed, unplugged, mode change).
        let before = self.held.len();
        self.held.retain(|h| {
            let keep = targets.iter().any(|t| t.id == h.status.id);
            if !keep {
                log::info(format!("Released {} (no longer a target)", h.status.name));
            }
            keep
        });
        if self.held.len() != before {
            self.last_state
                .retain(|id, _| targets.iter().any(|t| &t.id == id));
        }

        let now = Instant::now();
        let mut out = Vec::with_capacity(targets.len());
        for t in &targets {
            let mut recovering = false;

            // Already holding it: is the hold still effective?
            if let Some(i) = self.held.iter().position(|h| h.status.id == t.id) {
                match self.held[i].verify() {
                    Ok(cur) => {
                        let mut s = self.held[i].status.clone();
                        s.current = cur;
                        s.is_default = t.is_default;
                        out.push(s);
                        continue;
                    }
                    Err(reason) => {
                        log::info(format!(
                            "Lost the hold on {}: {reason} - re-applying",
                            self.held[i].status.name
                        ));
                        self.held.remove(i);
                        self.parked.remove(&t.id);
                        self.recoveries += 1;
                        recovering = true;
                    }
                }
            }

            // Recently found unlockable: don't retry on every tick.
            if let Some((until, s)) = self.parked.get(&t.id)
                && now < *until
            {
                let mut s = s.clone();
                s.is_default = t.is_default;
                out.push(s);
                continue;
            }

            let name = self
                .names
                .entry(t.id.clone())
                .or_insert_with(|| device_name(&t.device))
                .clone();
            let s = self.lock(t, &name);
            self.report(&s, recovering);
            out.push(s);
        }
        out
    }

    /// Log state transitions once instead of on every tick.
    fn report(&mut self, s: &DeviceStatus, recovering: bool) {
        let changed = self.last_state.get(&s.id) != Some(&s.state);
        if !(changed || recovering && s.state == State::Locked) {
            return;
        }
        self.last_state.insert(s.id.clone(), s.state.clone());
        let msg = match &s.state {
            State::Locked => format!(
                "{} {} at {} frames = {:.2} ms (driver default {:.2} ms, {} Hz)",
                if recovering { "Re-locked" } else { "Locked" },
                s.name,
                s.min,
                s.ms(s.min),
                s.ms(s.default),
                s.rate
            ),
            State::NothingToLock => format!(
                "{}: the driver offers nothing below {:.2} ms, so there is nothing to lock",
                s.name,
                s.ms(s.default)
            ),
            State::LockedByOther => format!(
                "{}: another app fixed the engine at {} frames ({:.2} ms); will keep trying",
                s.name,
                s.current,
                s.ms(s.current)
            ),
            State::Failed(err) => format!("{}: could not lock ({err}); will keep trying", s.name),
        };
        log::info(msg);
    }

    fn lock(&mut self, t: &Target, name: &str) -> DeviceStatus {
        let mut s = DeviceStatus::new(&t.id, name, t.is_default);
        match Self::try_lock(t, &mut s) {
            Ok(Some(held)) => {
                self.parked.remove(&t.id);
                self.held.push(held);
            }
            Ok(None) => {
                let wait = if s.state == State::NothingToLock {
                    30
                } else {
                    5
                };
                self.parked.insert(
                    t.id.clone(),
                    (Instant::now() + Duration::from_secs(wait), s.clone()),
                );
            }
            Err(e) => {
                s.state = State::Failed(hr_text(&e));
                self.parked.insert(
                    t.id.clone(),
                    (Instant::now() + Duration::from_secs(5), s.clone()),
                );
            }
        }
        s
    }

    fn try_lock(t: &Target, s: &mut DeviceStatus) -> Result<Option<Held>> {
        unsafe {
            let client: IAudioClient3 = t.device.Activate(CLSCTX_ALL, None)?;
            let fmt = read_caps(&client, s)?;
            if s.min == 0 || s.min >= s.default {
                s.state = State::NothingToLock;
                return Ok(None);
            }
            if let Err(e) = client.InitializeSharedAudioStream(
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                s.min,
                fmt.0,
                None,
            ) {
                if e.code() == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED {
                    s.current = current_period(&client).unwrap_or(s.current);
                    s.state = State::LockedByOther;
                    return Ok(None);
                }
                return Err(e);
            }
            drop(fmt);

            // From here on, Held's Drop cleans up if a later step fails.
            let event = CreateEventW(None, false, false, PCWSTR::null())?;
            let mut held = Held {
                client: Some(client),
                event,
                status: s.clone(),
            };
            let c = held.client.as_ref().unwrap();
            // Event-driven streams must have an event before Start; nobody needs to wait on it.
            c.SetEventHandle(event)?;
            // Give our silent session a clear name in the Volume Mixer.
            if let Ok(ctl) = c.GetService::<IAudioSessionControl>() {
                let _ = ctl.SetDisplayName(w!("LatencyLock (silent, keeps latency low)"), null());
            }
            c.Start()?;

            s.state = State::Locked;
            s.current = s.min;
            held.status = s.clone();
            Ok(Some(held))
        }
    }

    /// Snapshot of every active output device, for the details dialog and `--probe`.
    pub fn probe(&mut self) -> Vec<(DeviceStatus, bool)> {
        let Ok(en) = self.enumerator() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        unsafe {
            let default_id = Self::default_device(&en)
                .ok()
                .flatten()
                .and_then(|d| device_id(&d).ok());
            let Ok(col) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
                return rows;
            };
            for i in 0..col.GetCount().unwrap_or(0) {
                let Ok(device) = col.Item(i) else { continue };
                let Ok(id) = device_id(&device) else { continue };
                let is_default = default_id.as_deref() == Some(id.as_str());
                let mut s = DeviceStatus::new(&id, &device_name(&device), is_default);
                let caps = device
                    .Activate::<IAudioClient3>(CLSCTX_ALL, None)
                    .and_then(|c| read_caps(&c, &mut s).map(drop));
                s.state = match caps {
                    Err(e) => State::Failed(hr_text(&e)),
                    Ok(()) if s.min >= s.default => State::NothingToLock,
                    Ok(()) => State::Locked,
                };
                let held_by_us = self.held.iter().any(|h| h.status.id == id);
                rows.push((s, held_by_us));
            }
        }
        rows.sort_by_key(|(s, _)| !s.is_default);
        rows
    }
}

/// Plain-text report used by the details dialog and `--probe`.
pub fn describe(rows: &[(DeviceStatus, bool)]) -> String {
    if rows.is_empty() {
        return "No active output devices found.".to_owned();
    }
    let mut out = String::new();
    for (s, held) in rows {
        let mut tags = Vec::new();
        if s.is_default {
            tags.push("default output");
        }
        if *held {
            tags.push("held by LatencyLock");
        }
        out.push_str(&s.name);
        if !tags.is_empty() {
            out.push_str(&format!("  [{}]", tags.join(", ")));
        }
        out.push('\n');
        if let State::Failed(e) = &s.state {
            out.push_str(&format!("    could not query: {e}\n\n"));
            continue;
        }
        out.push_str(&format!(
            "    {} Hz | now {} ({:.2} ms) | min {} ({:.2} ms) | default {} ({:.2} ms)\n",
            s.rate,
            s.current,
            s.ms(s.current),
            s.min,
            s.ms(s.min),
            s.default,
            s.ms(s.default)
        ));
        if s.state == State::NothingToLock {
            out.push_str("    driver offers no smaller buffer\n");
        }
        out.push('\n');
    }
    out.trim_end().to_owned()
}
