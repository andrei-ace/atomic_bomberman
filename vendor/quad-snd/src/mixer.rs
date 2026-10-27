use crate::{AudioContext, PlaySoundParams};

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

thread_local! {
    // AudioContext is created on the application thread. The worker shares
    // this counter, while separate contexts/tests keep independent meters.
    static ONE_SHOT_METER: RefCell<Option<Arc<AtomicUsize>>> = const { RefCell::new(None) };
}

pub fn active_one_shots() -> usize {
    ONE_SHOT_METER.with(|meter| {
        meter
            .borrow()
            .as_ref()
            .map_or(0, |v| v.load(Ordering::Acquire))
    })
}

enum AudioMessage {
    AddSound(u32, Vec<f32>),
    Play(u32, u32, bool, f32),
    Stop(u32),
    StopAll(u32),
    SetVolume(u32, f32),
    SetVolumeAll(u32, f32),
    Delete(u32),
}

#[derive(Debug)]
pub struct SoundState {
    sound_id: u32,
    play_id: u32,
    sample: usize,
    data: Arc<[f32]>,
    looped: bool,
    volume: f32,
}

impl SoundState {
    fn get_samples(&mut self, n: usize) -> &[f32] {
        let data = &self.data[self.sample..];

        self.sample += n;

        match data.get(..n) {
            Some(data) => data,
            None => data,
        }
    }

    fn rewind(&mut self) {
        self.sample = 0;
    }
}

pub struct Mixer {
    rx: mpsc::Receiver<AudioMessage>,
    sounds: HashMap<u32, Arc<[f32]>>,
    mixer_state: Vec<SoundState>,
    active_one_shots: Arc<AtomicUsize>,
}

pub struct MixerBuilder {
    rx: mpsc::Receiver<AudioMessage>,
    active_one_shots: Arc<AtomicUsize>,
}

pub struct MixerControl {
    tx: mpsc::Sender<AudioMessage>,
    sound_id: Cell<u32>,
    play_id: Cell<u32>,
    active_one_shots: Arc<AtomicUsize>,
}

pub struct Playback {
    play_id: u32,
}

impl Playback {
    pub fn stop(self, ctx: &AudioContext) {
        ctx.mixer_ctrl.send(AudioMessage::Stop(self.play_id));
    }

    pub fn set_volume(&self, ctx: &AudioContext, volume: f32) {
        ctx.mixer_ctrl
            .send(AudioMessage::SetVolume(self.play_id, volume));
    }
}

impl MixerControl {
    pub fn load(&self, data: &[u8]) -> u32 {
        let sound_id = self.sound_id.get();

        let samples = load_samples_from_file(data).unwrap();

        self.tx
            .send(crate::mixer::AudioMessage::AddSound(sound_id, samples))
            .unwrap_or_else(|_| println!("Audio thread died"));
        self.sound_id.set(sound_id + 1);

        sound_id
    }

    pub fn play(&self, sound_id: u32, params: PlaySoundParams) -> Playback {
        let play_id = self.play_id.get();

        if !params.looped {
            self.active_one_shots.fetch_add(1, Ordering::AcqRel);
        }
        let queued = self.send(AudioMessage::Play(
            sound_id,
            play_id,
            params.looped,
            params.volume,
        ));
        if !queued && !params.looped {
            self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
        }

        self.play_id.set(play_id + 1);

        Playback { play_id }
    }

    pub fn stop(&self, play_id: u32) {
        self.send(AudioMessage::Stop(play_id));
    }

    pub fn stop_all(&self, sound_id: u32) {
        self.send(AudioMessage::StopAll(sound_id));
    }

    pub fn set_volume_all(&self, sound_id: u32, volume: f32) {
        self.send(AudioMessage::SetVolumeAll(sound_id, volume));
    }

    pub fn delete(&self, sound_id: u32) {
        self.send(AudioMessage::Delete(sound_id));
    }

    fn send(&self, message: AudioMessage) -> bool {
        self.tx
            .send(message)
            .map_err(|_| println!("Audio thread died"))
            .is_ok()
    }
}

impl MixerBuilder {
    pub fn build(self) -> Mixer {
        Mixer {
            rx: self.rx,
            sounds: HashMap::new(),
            mixer_state: vec![],
            active_one_shots: self.active_one_shots,
        }
    }
}

impl Mixer {
    pub fn new() -> (MixerBuilder, MixerControl) {
        let (tx, rx) = mpsc::channel();
        let active_one_shots = Arc::new(AtomicUsize::new(0));
        ONE_SHOT_METER.with(|meter| *meter.borrow_mut() = Some(active_one_shots.clone()));

        (
            MixerBuilder {
                rx,
                active_one_shots: active_one_shots.clone(),
            },
            MixerControl {
                tx,
                sound_id: Cell::new(0),
                play_id: Cell::new(0),
                active_one_shots,
            },
        )
    }

    pub fn fill_audio_buffer(&mut self, buffer: &mut [f32], frames: usize) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                AudioMessage::AddSound(id, data) => {
                    self.sounds.insert(id, data.into());
                }
                AudioMessage::Play(sound_id, play_id, looped, volume) => {
                    if let Some(data) = self.sounds.get(&sound_id) {
                        self.mixer_state.push(SoundState {
                            sound_id,
                            play_id,
                            sample: 0,
                            data: data.clone(),
                            looped,
                            volume,
                        });
                    } else if !looped {
                        self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                AudioMessage::Stop(play_id) => {
                    if let Some(i) = self.mixer_state.iter().position(|s| s.play_id == play_id) {
                        let removed = self.mixer_state.swap_remove(i);
                        if !removed.looped {
                            self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                }
                AudioMessage::StopAll(sound_id) => {
                    for i in (0..self.mixer_state.len()).rev() {
                        if self.mixer_state[i].sound_id == sound_id {
                            let removed = self.mixer_state.swap_remove(i);
                            if !removed.looped {
                                self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                    }
                }
                AudioMessage::SetVolume(play_id, volume) => {
                    if let Some(sound) = self.mixer_state.iter_mut().find(|s| s.play_id == play_id)
                    {
                        sound.volume = volume;
                    }
                }
                AudioMessage::SetVolumeAll(sound_id, volume) => {
                    for sound in self
                        .mixer_state
                        .iter_mut()
                        .filter(|s| s.sound_id == sound_id)
                    {
                        sound.volume = volume;
                    }
                }
                AudioMessage::Delete(sound_id) => {
                    for i in (0..self.mixer_state.len()).rev() {
                        if self.mixer_state[i].sound_id == sound_id {
                            let removed = self.mixer_state.swap_remove(i);
                            if !removed.looped {
                                self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                    }
                    self.sounds.remove(&sound_id);
                }
            }
        }

        // zeroize the buffer
        buffer.fill(0.0);

        // Note: Doing manual iteration so we can remove sounds that finished playing
        let mut i = 0;

        while let Some(sound) = self.mixer_state.get_mut(i) {
            let volume = sound.volume;
            let mut remainder = buffer.len();

            loop {
                let samples = sound.get_samples(remainder);

                for (b, s) in buffer.iter_mut().zip(samples) {
                    *b += s * volume;
                }

                remainder -= samples.len();

                if remainder > 0 && sound.looped {
                    sound.rewind();
                    continue;
                }

                break;
            }

            if remainder > 0 || (!sound.looped && sound.sample >= sound.data.len()) {
                let removed = self.mixer_state.swap_remove(i);
                if !removed.looped {
                    self.active_one_shots.fetch_sub(1, Ordering::AcqRel);
                }
            } else {
                i += 1;
            }
        }
    }
}

/// Parse ogg/wav/etc and get  resampled to 44100, 2 channel data
pub fn load_samples_from_file(bytes: &[u8]) -> Result<Vec<f32>, ()> {
    let mut audio_stream = {
        let file = std::io::Cursor::new(bytes);
        audrey::Reader::new(file).unwrap()
    };

    let description = audio_stream.description();
    let channels_count = description.channel_count();
    assert!(channels_count == 1 || channels_count == 2);

    let mut frames: Vec<f32> = Vec::with_capacity(4096);
    let mut samples_iterator = audio_stream
        .samples::<f32>()
        .map(std::result::Result::unwrap);

    // audrey's frame docs: "TODO: Should consider changing this behaviour to check the audio file's actual number of channels and automatically convert to F's number of channels while reading".
    // lets fix this TODO here
    if channels_count == 1 {
        frames.extend(samples_iterator.flat_map(|sample| [sample, sample]));
    } else if channels_count == 2 {
        frames.extend(samples_iterator);
    }

    let sample_rate = description.sample_rate();

    // stupid nearest-neighbor resampler
    if sample_rate != 44100 {
        let mut new_length = ((44100 as f32 / sample_rate as f32) * frames.len() as f32) as usize;

        // `new_length` must be an even number
        new_length -= new_length % 2;

        let mut resampled = vec![0.0; new_length];

        for (n, sample) in resampled.chunks_exact_mut(2).enumerate() {
            let ix = 2 * ((n as f32 / new_length as f32) * frames.len() as f32) as usize;
            sample[0] = frames[ix];
            sample[1] = frames[ix + 1];
        }
        return Ok(resampled);
    }

    Ok(frames)
}

#[cfg(test)]
mod native_voice_meter_tests {
    use super::*;

    fn setup() -> (Mixer, MixerControl) {
        let (builder, control) = Mixer::new();
        let mut mixer = builder.build();
        mixer.sounds.insert(1, vec![0.25; 8].into());
        mixer.sounds.insert(2, vec![0.5; 4].into());
        (mixer, control)
    }
    fn count(control: &MixerControl) -> usize {
        control.active_one_shots.load(Ordering::Acquire)
    }
    fn play(control: &MixerControl, sound: u32, looped: bool) -> Playback {
        control.play(sound, PlaySoundParams { looped, volume: 1. })
    }
    fn mix(mixer: &mut Mixer, samples: usize) {
        mixer.fill_audio_buffer(&mut vec![0.; samples], samples / 2);
    }

    #[test]
    fn pending_commands_are_counted_before_a_burst_reaches_the_worker() {
        let (mut mixer, control) = setup();
        for _ in 0..6 {
            play(&control, 1, false);
        }
        assert_eq!(count(&control), 6);
        mix(&mut mixer, 4);
        assert_eq!(count(&control), 6);
        mix(&mut mixer, 4);
        assert_eq!(count(&control), 0); // Exact-length completion, not one buffer late.
    }
    #[test]
    fn shorter_completion_and_looping_music_have_distinct_lifetimes() {
        let (mut mixer, control) = setup();
        play(&control, 1, true);
        play(&control, 2, false);
        assert_eq!(count(&control), 1);
        mix(&mut mixer, 6);
        assert_eq!(count(&control), 0);
        assert_eq!(mixer.mixer_state.len(), 1);
        mix(&mut mixer, 32);
        assert_eq!(count(&control), 0);
    }
    #[test]
    fn stop_and_repeated_stop_release_a_voice_once() {
        let (mut mixer, control) = setup();
        let voice = play(&control, 1, false);
        control.stop(voice.play_id);
        control.stop(voice.play_id);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 0);
    }
    #[test]
    fn stop_all_and_delete_release_pending_and_live_voices() {
        let (mut mixer, control) = setup();
        play(&control, 1, false);
        play(&control, 1, false);
        play(&control, 1, true);
        control.stop_all(1);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 0);
        play(&control, 2, false);
        control.delete(2);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 0);
        play(&control, 1, false);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 1);
        control.delete(1);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 0);
    }
    #[test]
    fn missing_sound_and_dead_worker_do_not_leak_capacity() {
        let (mut mixer, control) = setup();
        play(&control, 99, false);
        assert_eq!(count(&control), 1);
        mix(&mut mixer, 2);
        assert_eq!(count(&control), 0);
        drop(mixer);
        play(&control, 1, false);
        assert_eq!(count(&control), 0);
    }
    #[test]
    fn contexts_have_independent_voice_meters() {
        let (_first, first_control) = setup();
        play(&first_control, 1, false);
        let (_second, second_control) = setup();
        assert_eq!(count(&first_control), 1);
        assert_eq!(count(&second_control), 0);
        assert_eq!(active_one_shots(), 0);
    }
}
