// https://github.com/floooh/sokol/blob/master/sokol_audio.h
// https://github.com/norse-rs/audir/blob/master/audir/src/wasapi/mod.rs

use crate::PlaySoundParams;

pub use crate::mixer::Playback;

use winapi::shared::guiddef::{CLSID, IID};
use winapi::shared::ksmedia;
use winapi::shared::minwindef::*;
use winapi::shared::mmreg::*;
use winapi::um::audioclient::*;
use winapi::um::audiosessiontypes::*;
use winapi::um::combaseapi::*;
use winapi::um::mmdeviceapi::*;
use winapi::um::objbase::*;
use winapi::um::synchapi::*;
use winapi::um::winbase::*;

use std::sync::mpsc;

// thanks sokol_audio!
// https://github.com/floooh/sokol/blob/master/sokol_audio.h#L559
static IID_IAudioClient: IID = IID {
    Data1: 0x1cb9ad4c,
    Data2: 0xdbfa,
    Data3: 0x4c32,
    Data4: [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2],
};
static IID_IMMDeviceEnumerator: IID = IID {
    Data1: 0xa95664d2,
    Data2: 0x9614,
    Data3: 0x4f35,
    Data4: [0xa7, 0x46, 0xde, 0x8d, 0xb6, 0x36, 0x17, 0xe6],
};
static CLSID_IMMDeviceEnumerator: CLSID = CLSID {
    Data1: 0xbcde0395,
    Data2: 0xe52f,
    Data3: 0x467c,
    Data4: [0x8e, 0x3d, 0xc4, 0x57, 0x92, 0x91, 0x69, 0x2e],
};
static IID_IAudioRenderClient: IID = IID {
    Data1: 0xf294acfc,
    Data2: 0x3146,
    Data3: 0x4483,
    Data4: [0xa7, 0xbf, 0xad, 0xdc, 0xa7, 0xc2, 0x60, 0xe2],
};
static IID_Devinterface_Audio_Render: IID = IID {
    Data1: 0xe6327cad,
    Data2: 0xdcec,
    Data3: 0x4949,
    Data4: [0xae, 0x8a, 0x99, 0x1e, 0x97, 0x6a, 0x79, 0xd2],
};
static IID_IActivateAudioInterface_Completion_Handler: IID = IID {
    Data1: 0x94ea2b94,
    Data2: 0xe9cc,
    Data3: 0x49e0,
    Data4: [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90],
};

mod consts {
    pub const CHANNELS: u32 = 2;
    pub const SAMPLE_RATE: u32 = 44100;
    pub const BUFFER_FRAMES: u32 = 4096;
}

// Own every COM interface and event on the audio thread. Initialization and
// device-loss errors retain a live mixer instead of panicking its receiver.
use std::{
    cell::RefCell,
    ptr,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use winapi::shared::ntdef::HANDLE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputStatus {
    Device,
    SilentRequested,
    Unavailable(String),
}
thread_local! {
    static OUTPUT: RefCell<Option<Arc<Mutex<OutputStatus>>>> = const { RefCell::new(None) };
}
pub fn output_status() -> Option<OutputStatus> {
    OUTPUT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|status| status.lock().unwrap().clone())
    })
}

struct Wasapi {
    com: bool,
    event: HANDLE,
    enumerator: *mut IMMDeviceEnumerator,
    device: *mut IMMDevice,
    client: *mut IAudioClient,
    render: *mut IAudioRenderClient,
    frames: u32,
}
fn check(operation: &str, code: i32) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation}: HRESULT 0x{:08x}", code as u32))
    } else {
        Ok(())
    }
}
impl Wasapi {
    fn open() -> Result<Self, String> {
        let mut output = Self {
            com: false,
            event: ptr::null_mut(),
            enumerator: ptr::null_mut(),
            device: ptr::null_mut(),
            client: ptr::null_mut(),
            render: ptr::null_mut(),
            frames: 0,
        };
        unsafe {
            check(
                "CoInitializeEx",
                CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED),
            )?;
            output.com = true;
            output.event = CreateEventA(ptr::null_mut(), FALSE, FALSE, ptr::null());
            if output.event.is_null() {
                return Err("CreateEvent failed".into());
            }
            check(
                "CoCreateInstance",
                CoCreateInstance(
                    &CLSID_IMMDeviceEnumerator,
                    ptr::null_mut(),
                    CLSCTX_ALL,
                    &IID_IMMDeviceEnumerator,
                    &mut output.enumerator as *mut _ as _,
                ),
            )?;
            if output.enumerator.is_null() {
                return Err("No audio device enumerator".into());
            }
            check(
                "GetDefaultAudioEndpoint",
                (*output.enumerator).GetDefaultAudioEndpoint(eRender, eConsole, &mut output.device),
            )?;
            if output.device.is_null() {
                return Err("No default audio output".into());
            }
            check(
                "Activate",
                (*output.device).Activate(
                    &IID_IAudioClient,
                    CLSCTX_ALL,
                    ptr::null_mut(),
                    &mut output.client as *mut _ as _,
                ),
            )?;
            if output.client.is_null() {
                return Err("No audio client".into());
            }
            let format = WAVEFORMATEXTENSIBLE {
                Format: WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                    nChannels: consts::CHANNELS as _,
                    nSamplesPerSec: consts::SAMPLE_RATE,
                    nAvgBytesPerSec: consts::SAMPLE_RATE * consts::CHANNELS * 4,
                    nBlockAlign: consts::CHANNELS as u16 * 4,
                    wBitsPerSample: 32,
                    cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                        - std::mem::size_of::<WAVEFORMATEX>()) as _,
                },
                Samples: 32,
                dwChannelMask: 3,
                SubFormat: ksmedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            };
            let duration =
                (consts::BUFFER_FRAMES as f64 / consts::SAMPLE_RATE as f64 * 10_000_000.) as i64;
            check(
                "Initialize",
                (*output.client).Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK | 0x80000000 | 0x08000000,
                    duration,
                    0,
                    &format as *const _ as _,
                    ptr::null(),
                ),
            )?;
            check(
                "GetBufferSize",
                (*output.client).GetBufferSize(&mut output.frames),
            )?;
            check(
                "GetService",
                (*output.client)
                    .GetService(&IID_IAudioRenderClient, &mut output.render as *mut _ as _),
            )?;
            if output.render.is_null() {
                return Err("No audio render client".into());
            }
            check(
                "SetEventHandle",
                (*output.client).SetEventHandle(output.event),
            )?;
            check("Start", (*output.client).Start())?;
        }
        Ok(output)
    }
    fn fill(&mut self, mixer: &mut crate::mixer::Mixer) -> Result<(), String> {
        unsafe {
            match WaitForSingleObject(self.event, 20) {
                258 => return Ok(()),
                0 => (),
                _ => return Err("Audio event wait failed".into()),
            }
            let mut padding = 0;
            check(
                "GetCurrentPadding",
                (*self.client).GetCurrentPadding(&mut padding),
            )?;
            if padding > self.frames {
                return Err("Invalid audio buffer padding".into());
            }
            let frames = self.frames - padding;
            if frames == 0 {
                return Ok(());
            }
            let mut buffer = ptr::null_mut();
            check("GetBuffer", (*self.render).GetBuffer(frames, &mut buffer))?;
            if buffer.is_null() {
                return Err("No audio render buffer".into());
            }
            mixer.fill_audio_buffer(
                std::slice::from_raw_parts_mut(buffer as *mut f32, frames as usize * 2),
                frames as usize,
            );
            check("ReleaseBuffer", (*self.render).ReleaseBuffer(frames, 0))?;
        }
        Ok(())
    }
}
impl Drop for Wasapi {
    fn drop(&mut self) {
        unsafe {
            if !self.client.is_null() {
                (*self.client).Stop();
            }
            if !self.render.is_null() {
                (*self.render).Release();
            }
            if !self.client.is_null() {
                (*self.client).Release();
            }
            if !self.device.is_null() {
                (*self.device).Release();
            }
            if !self.enumerator.is_null() {
                (*self.enumerator).Release();
            }
            if !self.event.is_null() {
                winapi::um::handleapi::CloseHandle(self.event);
            }
            if self.com {
                CoUninitialize();
            }
        }
    }
}

pub struct AudioContext {
    pub(crate) mixer_ctrl: crate::mixer::MixerControl,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl AudioContext {
    pub fn new() -> Self {
        Self::create(
            std::env::var_os("ATOMIC_BOMBERMAN_SILENT_AUDIO").is_some(),
            Wasapi::open,
        )
    }
    fn create(silent: bool, initialize: fn() -> Result<Wasapi, String>) -> Self {
        let (builder, mixer_ctrl) = crate::mixer::Mixer::new();
        let (stop, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(OutputStatus::SilentRequested));
        OUTPUT.with(|slot| *slot.borrow_mut() = Some(status.clone()));
        let worker = thread::Builder::new()
            .name("bomberman-wasapi".into())
            .spawn(move || {
                let mut mixer = builder.build();
                let mut device = if silent {
                    None
                } else {
                    match initialize() {
                        Ok(device) => {
                            *status.lock().unwrap() = OutputStatus::Device;
                            Some(device)
                        }
                        Err(error) => {
                            *status.lock().unwrap() = OutputStatus::Unavailable(error);
                            None
                        }
                    }
                };
                let _ = ready_tx.send(());
                let mut samples = vec![0.; consts::BUFFER_FRAMES as usize * 2];
                let period = Duration::from_secs_f64(
                    consts::BUFFER_FRAMES as f64 / consts::SAMPLE_RATE as f64,
                );
                loop {
                    match receiver.try_recv() {
                        Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => (),
                    }
                    if let Some(output) = &mut device {
                        if let Err(error) = output.fill(&mut mixer) {
                            *status.lock().unwrap() = OutputStatus::Unavailable(error);
                            device = None;
                        }
                    } else {
                        let started = Instant::now();
                        mixer.fill_audio_buffer(&mut samples, consts::BUFFER_FRAMES as usize);
                        match receiver.recv_timeout(period.saturating_sub(started.elapsed())) {
                            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                        }
                    }
                }
            })
            .expect("start audio worker");
        ready_rx.recv().expect("initialize audio worker");
        Self {
            mixer_ctrl,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for AudioContext {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct Sound {
    sound_id: u32,
}

impl Sound {
    pub fn load(ctx: &AudioContext, data: &[u8]) -> Sound {
        let sound_id = ctx.mixer_ctrl.load(data);

        Sound { sound_id }
    }

    pub fn play(&self, ctx: &AudioContext, params: PlaySoundParams) -> Playback {
        ctx.mixer_ctrl.play(self.sound_id, params)
    }

    pub fn stop(&self, ctx: &AudioContext) {
        ctx.mixer_ctrl.stop_all(self.sound_id);
    }

    pub fn set_volume(&self, ctx: &AudioContext, volume: f32) {
        ctx.mixer_ctrl.set_volume_all(self.sound_id, volume);
    }

    pub fn delete(&self, ctx: &AudioContext) {
        ctx.mixer_ctrl.delete(self.sound_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn short_wav() -> Vec<u8> {
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(436u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(44100u32.to_le_bytes());
        wav.extend(176400u32.to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(400u32.to_le_bytes());
        wav.extend([0; 400]);
        wav
    }
    fn unavailable() -> Result<Wasapi, String> {
        Err("synthetic missing output device".into())
    }
    #[test]
    fn mute_and_device_failure_keep_consuming_commands_and_shut_down() {
        for silent in [true, false] {
            let context = AudioContext::create(silent, unavailable);
            if silent {
                assert_eq!(output_status(), Some(OutputStatus::SilentRequested));
            } else {
                assert!(matches!(
                    output_status(),
                    Some(OutputStatus::Unavailable(_))
                ));
            }
            let sound = Sound::load(&context, &short_wav());
            sound.play(&context, PlaySoundParams::default());
            let started = Instant::now();
            while crate::active_one_shots() > 0 && started.elapsed() < Duration::from_secs(2) {
                thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(crate::active_one_shots(), 0);
            sound.delete(&context);
            drop(context);
        }
    }
}
