use crate::{mixer, PlaySoundParams};
use std::{
    cell::{RefCell, UnsafeCell},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub use crate::mixer::Playback;
#[path = "coreaudio/coreaudio.rs"]
mod coreaudio;
use coreaudio::*;

const RATE: u32 = 44100;
const CHANNELS: u32 = 2;
const BUFFER_FRAMES: u32 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputStatus {
    Device,
    SilentRequested,
    Unavailable(String),
}

thread_local! {
    static OUTPUT: RefCell<Option<Arc<Mutex<OutputStatus>>>> = const { RefCell::new(None) };
}

/// Status of the most recently created context on the calling thread.
pub fn output_status() -> Option<OutputStatus> {
    OUTPUT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|status| status.lock().unwrap().clone())
    })
}

struct CallbackState {
    // Audio Queue serializes its callbacks. Only that callback accesses the
    // mixer until synchronous disposal; the owner may access the atomic flags.
    mixer: UnsafeCell<mixer::Mixer>,
    stopping: AtomicBool,
    failed: Arc<AtomicI32>,
}

unsafe extern "C" fn callback(
    data: *mut std::ffi::c_void,
    queue: _saudio_AudioQueueRef,
    buffer: _saudio_AudioQueueBufferRef,
) {
    let state = data as *mut CallbackState;
    if (*state).stopping.load(Ordering::Acquire) {
        return;
    }
    if buffer.is_null()
        || (*buffer).mAudioData.is_null()
        || (*buffer).mAudioDataByteSize > (*buffer).mAudioDataBytesCapacity
    {
        (*state).failed.store(-1, Ordering::Release);
        return;
    }
    let frames = (*buffer).mAudioDataByteSize as usize / (CHANNELS as usize * 4);
    let samples = std::slice::from_raw_parts_mut((*buffer).mAudioData as *mut f32, frames * 2);
    (&mut *(*state).mixer.get()).fill_audio_buffer(samples, frames);
    let result = AudioQueueEnqueueBuffer(queue, buffer, 0, ptr::null());
    if result != 0 {
        // Only signal the owner; disposing a queue from its callback cannot
        // guarantee that callbacks have stopped. No locks or allocation here.
        (*state).failed.store(result, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct QueueApi {
    new: unsafe extern "C" fn(
        *const _saudio_AudioStreamBasicDescription,
        _saudio_AudioQueueOutputCallback,
        *mut std::ffi::c_void,
        _saudio_CFRunLoopRef,
        _saudio_CFStringRef,
        u32,
        *mut _saudio_AudioQueueRef,
    ) -> i32,
    allocate:
        unsafe extern "C" fn(_saudio_AudioQueueRef, u32, *mut _saudio_AudioQueueBufferRef) -> i32,
    enqueue: unsafe extern "C" fn(
        _saudio_AudioQueueRef,
        _saudio_AudioQueueBufferRef,
        u32,
        *const _saudio_AudioStreamPacketDescription,
    ) -> i32,
    start: unsafe extern "C" fn(_saudio_AudioQueueRef, *const _saudio_AudioTimeStamp) -> i32,
    stop: unsafe extern "C" fn(_saudio_AudioQueueRef, bool) -> i32,
    dispose: unsafe extern "C" fn(_saudio_AudioQueueRef, bool) -> i32,
}
const SYSTEM_API: QueueApi = QueueApi {
    new: AudioQueueNewOutput,
    allocate: AudioQueueAllocateBuffer,
    enqueue: AudioQueueEnqueueBuffer,
    start: AudioQueueStart,
    stop: AudioQueueStop,
    dispose: AudioQueueDispose,
};

struct QueueOutput {
    queue: _saudio_AudioQueueRef,
    state: Option<Box<CallbackState>>,
    api: QueueApi,
}
impl QueueOutput {
    fn start(&mut self) -> Result<(), String> {
        let fmt = _saudio_AudioStreamBasicDescription {
            mSampleRate: RATE as f64,
            mFormatID: 1819304813,             // 'lpcm'
            mFormatFlags: (1 << 0) | (1 << 3), // float, packed
            mFramesPerPacket: 1,
            mChannelsPerFrame: CHANNELS,
            mBytesPerFrame: 4 * CHANNELS,
            mBytesPerPacket: 4 * CHANNELS,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        let check = |name: &str, code: i32| {
            if code == 0 {
                Ok(())
            } else {
                Err(format!("{name}: OSStatus {code}"))
            }
        };
        unsafe {
            let data = &mut **self.state.as_mut().unwrap() as *mut CallbackState as *mut _;
            check(
                "AudioQueueNewOutput",
                (self.api.new)(
                    &fmt,
                    Some(callback),
                    data,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                    &mut self.queue,
                ),
            )?;
            if self.queue.is_null() {
                return Err("AudioQueueNewOutput returned no queue".into());
            }
            for _ in 0..2 {
                let mut buffer = ptr::null_mut();
                let size = BUFFER_FRAMES * fmt.mBytesPerFrame;
                check(
                    "AudioQueueAllocateBuffer",
                    (self.api.allocate)(self.queue, size, &mut buffer),
                )?;
                if buffer.is_null()
                    || (*buffer).mAudioData.is_null()
                    || (*buffer).mAudioDataBytesCapacity < size
                {
                    return Err("AudioQueueAllocateBuffer returned an invalid buffer".into());
                }
                (*buffer).mAudioDataByteSize = size;
                ptr::write_bytes((*buffer).mAudioData as *mut u8, 0, size as usize);
                check(
                    "AudioQueueEnqueueBuffer",
                    (self.api.enqueue)(self.queue, buffer, 0, ptr::null()),
                )?;
            }
            check("AudioQueueStart", (self.api.start)(self.queue, ptr::null()))?;
        }
        Ok(())
    }
    fn close(&mut self) {
        if !self.queue.is_null() {
            self.state
                .as_ref()
                .unwrap()
                .stopping
                .store(true, Ordering::Release);
            unsafe {
                (self.api.stop)(self.queue, true);
                if (self.api.dispose)(self.queue, true) != 0 {
                    // Retain callback memory if OS disposal cannot be proven.
                    std::mem::forget(self.state.take());
                }
            }
            self.queue = ptr::null_mut();
        }
    }
    fn into_mixer(mut self) -> Option<mixer::Mixer> {
        self.close();
        self.state.take().map(|state| state.mixer.into_inner())
    }
}
impl Drop for QueueOutput {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct AudioContext {
    pub(crate) mixer_ctrl: mixer::MixerControl,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl AudioContext {
    pub fn new() -> Self {
        Self::create(
            std::env::var_os("ATOMIC_BOMBERMAN_SILENT_AUDIO").is_some(),
            SYSTEM_API,
        )
    }
    fn create(silent: bool, api: QueueApi) -> Self {
        let (builder, mixer_ctrl) = mixer::Mixer::new();
        let (stop, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(OutputStatus::SilentRequested));
        OUTPUT.with(|slot| *slot.borrow_mut() = Some(status.clone()));
        let worker = thread::Builder::new()
            .name("bomberman-coreaudio".into())
            .spawn(move || {
                let failed = Arc::new(AtomicI32::new(0));
                let mut output = QueueOutput {
                    queue: ptr::null_mut(),
                    api,
                    state: Some(Box::new(CallbackState {
                        mixer: UnsafeCell::new(builder.build()),
                        stopping: AtomicBool::new(false),
                        failed: failed.clone(),
                    })),
                };
                let device = if silent {
                    false
                } else {
                    match output.start() {
                        Ok(()) => {
                            *status.lock().unwrap() = OutputStatus::Device;
                            true
                        }
                        Err(error) => {
                            *status.lock().unwrap() = OutputStatus::Unavailable(error);
                            false
                        }
                    }
                };
                if !device {
                    output.close();
                }
                let _ = ready_tx.send(());
                if device {
                    loop {
                        match receiver.recv_timeout(Duration::from_millis(20)) {
                            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                        }
                        let error = failed.load(Ordering::Acquire);
                        if error != 0 {
                            *status.lock().unwrap() = OutputStatus::Unavailable(format!(
                                "AudioQueue callback: OSStatus {error}"
                            ));
                            break;
                        }
                    }
                }
                if let Some(mut mixer) = output.into_mixer() {
                    let mut samples = vec![0.; BUFFER_FRAMES as usize * 2];
                    let period = Duration::from_secs_f64(BUFFER_FRAMES as f64 / RATE as f64);
                    loop {
                        let started = Instant::now();
                        mixer.fill_audio_buffer(&mut samples, BUFFER_FRAMES as usize);
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
    pub fn load(ctx: &AudioContext, data: &[u8]) -> Self {
        Self {
            sound_id: ctx.mixer_ctrl.load(data),
        }
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
    use std::sync::atomic::AtomicUsize;
    static CREATED: AtomicUsize = AtomicUsize::new(0);
    static DISPOSED: AtomicUsize = AtomicUsize::new(0);
    struct FakeQueue {
        buffer: _saudio_AudioQueueBuffer,
        samples: Vec<f32>,
        state: *mut CallbackState,
    }
    unsafe extern "C" fn new_queue(
        _: *const _saudio_AudioStreamBasicDescription,
        _: _saudio_AudioQueueOutputCallback,
        data: *mut std::ffi::c_void,
        _: _saudio_CFRunLoopRef,
        _: _saudio_CFStringRef,
        _: u32,
        out: *mut _saudio_AudioQueueRef,
    ) -> i32 {
        *out = Box::into_raw(Box::new(FakeQueue {
            buffer: std::mem::zeroed(),
            samples: vec![0.; BUFFER_FRAMES as usize * 2],
            state: data as *mut CallbackState,
        })) as *mut _;
        CREATED.fetch_add(1, Ordering::SeqCst);
        0
    }
    unsafe extern "C" fn allocate(
        queue: _saudio_AudioQueueRef,
        size: u32,
        out: *mut _saudio_AudioQueueBufferRef,
    ) -> i32 {
        let queue = &mut *(queue as *mut FakeQueue);
        queue.buffer.mAudioDataBytesCapacity = size;
        queue.buffer.mAudioData = queue.samples.as_mut_ptr() as *mut _;
        *out = &mut queue.buffer;
        0
    }
    unsafe extern "C" fn enqueue(
        _: _saudio_AudioQueueRef,
        _: _saudio_AudioQueueBufferRef,
        _: u32,
        _: *const _saudio_AudioStreamPacketDescription,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn start(_: _saudio_AudioQueueRef, _: *const _saudio_AudioTimeStamp) -> i32 {
        0
    }
    unsafe extern "C" fn fail_after_start(
        queue: _saudio_AudioQueueRef,
        _: *const _saudio_AudioTimeStamp,
    ) -> i32 {
        (*(*(queue as *mut FakeQueue)).state)
            .failed
            .store(-1234, Ordering::Release);
        0
    }
    unsafe extern "C" fn stop(_: _saudio_AudioQueueRef, _: bool) -> i32 {
        0
    }
    unsafe extern "C" fn dispose(queue: _saudio_AudioQueueRef, _: bool) -> i32 {
        drop(Box::from_raw(queue as *mut FakeQueue));
        DISPOSED.fetch_add(1, Ordering::SeqCst);
        0
    }
    unsafe extern "C" fn fail_new(
        _: *const _saudio_AudioStreamBasicDescription,
        _: _saudio_AudioQueueOutputCallback,
        _: *mut std::ffi::c_void,
        _: _saudio_CFRunLoopRef,
        _: _saudio_CFStringRef,
        _: u32,
        _: *mut _saudio_AudioQueueRef,
    ) -> i32 {
        -50
    }
    unsafe extern "C" fn fail_allocate(
        _: _saudio_AudioQueueRef,
        _: u32,
        _: *mut _saudio_AudioQueueBufferRef,
    ) -> i32 {
        -50
    }
    unsafe extern "C" fn fail_enqueue(
        _: _saudio_AudioQueueRef,
        _: _saudio_AudioQueueBufferRef,
        _: u32,
        _: *const _saudio_AudioStreamPacketDescription,
    ) -> i32 {
        -50
    }
    unsafe extern "C" fn fail_start(
        _: _saudio_AudioQueueRef,
        _: *const _saudio_AudioTimeStamp,
    ) -> i32 {
        -50
    }
    const FAKE: QueueApi = QueueApi {
        new: new_queue,
        allocate,
        enqueue,
        start,
        stop,
        dispose,
    };
    fn short_wav() -> Vec<u8> {
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(436u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(RATE.to_le_bytes());
        wav.extend((RATE * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(400u32.to_le_bytes());
        wav.extend([0; 400]);
        wav
    }
    #[test]
    fn mute_and_initialization_failures_keep_a_paced_mixer_and_dispose_once() {
        for (silent, api) in [
            (true, FAKE),
            (
                false,
                QueueApi {
                    new: fail_new,
                    ..FAKE
                },
            ),
            (
                false,
                QueueApi {
                    allocate: fail_allocate,
                    ..FAKE
                },
            ),
            (
                false,
                QueueApi {
                    enqueue: fail_enqueue,
                    ..FAKE
                },
            ),
            (
                false,
                QueueApi {
                    start: fail_start,
                    ..FAKE
                },
            ),
        ] {
            let context = AudioContext::create(silent, api);
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
        for _ in 0..5 {
            let context = AudioContext::create(false, FAKE);
            assert_eq!(output_status(), Some(OutputStatus::Device));
            drop(context);
        }
        let context = AudioContext::create(
            false,
            QueueApi {
                start: fail_after_start,
                ..FAKE
            },
        );
        let sound = Sound::load(&context, &short_wav());
        sound.play(&context, PlaySoundParams::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        while (crate::active_one_shots() > 0 || output_status() == Some(OutputStatus::Device))
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(crate::active_one_shots(), 0);
        assert!(
            matches!(output_status(), Some(OutputStatus::Unavailable(reason)) if reason.contains("-1234"))
        );
        drop(context);
        assert_eq!(CREATED.load(Ordering::SeqCst), 9);
        assert_eq!(DISPOSED.load(Ordering::SeqCst), 9);
    }
}
