use alvr_common::prelude::*;
use alvr_sockets::{StreamReceiver, StreamSender};
use parking_lot::Mutex;
use serde::Serialize;
use std::{collections::VecDeque, sync::Arc};

#[cfg(not(target_os = "android"))]
mod cpal_audio;

#[cfg(target_os = "linux")]
mod pipewire_audio;

#[derive(Serialize)]
pub struct AudioDevicesList {
    pub output: Vec<String>,
    pub input: Vec<String>,
}

#[derive(Clone)]
pub enum AudioDeviceType {
    Output,
    Input,

    // for the virtual microphone devices, input and output labels are swapped
    VirtualMicrophoneInput,
    VirtualMicrophoneOutput { matching_input_device_name: String },
}

impl AudioDeviceType {
    pub fn is_output(&self) -> bool {
        matches!(self, Self::Output | Self::VirtualMicrophoneInput)
    }
}

pub enum AudioDevice {
    #[cfg(not(target_os = "android"))]
    Cpal(cpal_audio::CpalAudioDevice),
    #[cfg(target_os = "linux")]
    PipeWire(pipewire_audio::PipeWireAudioDevice),
    #[cfg(target_os = "android")]
    None,
}

impl AudioDevice {
    pub fn new(
        linux_backend: alvr_session::LinuxAudioBackend,
        id: alvr_session::AudioDeviceId,
        device_type: AudioDeviceType,
    ) -> StrResult<Self> {
        #[allow(unused_variables)]
        let (linux_backend, id, device_type) = (linux_backend, id, device_type);

        #[cfg(target_os = "linux")]
        return match linux_backend {
            alvr_session::LinuxAudioBackend::Alsa | alvr_session::LinuxAudioBackend::Jack => {
                Ok(Self::Cpal(cpal_audio::CpalAudioDevice::new(
                    linux_backend,
                    id,
                    device_type,
                )?))
            }
            alvr_session::LinuxAudioBackend::PipeWire => Ok(Self::PipeWire(
                pipewire_audio::PipeWireAudioDevice::new(id, device_type)?,
            )),
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        return Ok(Self::Cpal(cpal_audio::CpalAudioDevice::new(
            linux_backend,
            id,
            device_type,
        )?));
        #[cfg(target_os = "android")]
        return Ok(Self::None);
    }

    pub fn name(&self) -> StrResult<String> {
        match self {
            #[cfg(not(target_os = "android"))]
            Self::Cpal(d) => d.name(),
            #[cfg(target_os = "linux")]
            Self::PipeWire(d) => d.name(),
            #[cfg(target_os = "android")]
            Self::None => Ok("Android Audio".to_owned()),
        }
    }

    #[allow(unreachable_patterns)]
    pub fn is_same_device(&self, other: &Self) -> bool {
        match (self, other) {
            #[cfg(not(target_os = "android"))]
            (Self::Cpal(d1), Self::Cpal(d2)) => d1.is_same_device(d2),
            #[cfg(target_os = "linux")]
            (Self::PipeWire(d1), Self::PipeWire(d2)) => d1.is_same_device(d2),
            #[cfg(target_os = "android")]
            (Self::None, Self::None) => true,
            _ => false,
        }
    }
}

pub fn get_devices_list(
    linux_backend: alvr_session::LinuxAudioBackend,
) -> StrResult<AudioDevicesList> {
    #[allow(unused_variables)]
    let linux_backend = linux_backend;

    #[cfg(target_os = "linux")]
    return match linux_backend {
        alvr_session::LinuxAudioBackend::Alsa | alvr_session::LinuxAudioBackend::Jack => {
            cpal_audio::get_devices_list(linux_backend)
        }
        alvr_session::LinuxAudioBackend::PipeWire => pipewire_audio::get_devices_list(),
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    return cpal_audio::get_devices_list(linux_backend);
    #[cfg(target_os = "android")]
    return Ok(AudioDevicesList {
        output: vec![],
        input: vec![],
    });
}

pub async fn record_audio_loop(
    device: AudioDevice,
    channels_count: u16,
    mute: bool,
    sender: StreamSender<()>,
) -> StrResult {
    #[allow(unused_variables)]
    let (channels_count, mute, sender) = (channels_count, mute, sender);

    match device {
        #[cfg(not(target_os = "android"))]
        AudioDevice::Cpal(d) => {
            cpal_audio::record_audio_loop(d, channels_count, mute, sender).await
        }
        #[cfg(target_os = "linux")]
        AudioDevice::PipeWire(d) => {
            pipewire_audio::record_audio_loop(d, channels_count, mute, sender).await
        }
        #[cfg(target_os = "android")]
        AudioDevice::None => std::future::pending().await,
    }
}

pub async fn play_audio_loop(
    device: AudioDevice,
    channels_count: u16,
    sample_rate: u32,
    config: alvr_session::AudioConfig,
    receiver: StreamReceiver<()>,
) -> StrResult {
    #[allow(unused_variables)]
    let (channels_count, sample_rate, config, receiver) =
        (channels_count, sample_rate, config, receiver);

    match device {
        #[cfg(not(target_os = "android"))]
        AudioDevice::Cpal(d) => {
            cpal_audio::play_audio_loop(d, channels_count, sample_rate, config, receiver).await
        }
        #[cfg(target_os = "linux")]
        AudioDevice::PipeWire(d) => {
            pipewire_audio::play_audio_loop(d, channels_count, sample_rate, config, receiver).await
        }
        #[cfg(target_os = "android")]
        AudioDevice::None => std::future::pending().await,
    }
}

pub fn get_sample_rate(device: &AudioDevice) -> StrResult<u32> {
    match device {
        #[cfg(not(target_os = "android"))]
        AudioDevice::Cpal(d) => cpal_audio::get_sample_rate(d),
        #[cfg(target_os = "linux")]
        AudioDevice::PipeWire(p) => pipewire_audio::get_sample_rate(p),
        #[cfg(target_os = "android")]
        AudioDevice::None => Ok(48000),
    }
}

#[cfg(windows)]
pub fn get_windows_device_id(device: &AudioDevice) -> StrResult<String> {
    match device {
        AudioDevice::Cpal(d) => cpal_audio::get_windows_device_id(d),
    }
}

trait ToF32 {
    fn to_f32(self) -> f32;
}

#[cfg(not(target_os = "android"))]
impl ToF32 for i16 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        use cpal::Sample;
        self.to_sample()
    }
}

#[cfg(target_os = "android")]
impl ToF32 for i16 {
    #[inline(always)]
    fn to_f32(self) -> f32 {
        self as f32 / 32768.0
    }
}

// Audio callback. This is designed to be as less complex as possible. Still, when needed, this
// callback can render a fade-out autonomously.
#[inline]
pub fn get_next_frame_batch(
    sample_buffer: &mut VecDeque<f32>,
    channels_count: usize,
    batch_frames_count: usize,
    output_buffer: &mut Vec<f32>,
) {
    output_buffer.clear();

    if sample_buffer.len() / channels_count >= batch_frames_count {
        output_buffer.extend(sample_buffer.drain(0..batch_frames_count * channels_count));
        // fade-ins and cross-fades are rendered in the receive loop directly inside sample_buffer.
    } else {
        output_buffer.resize(batch_frames_count * channels_count, 0.);
    }
}

// The receive loop is resposible for ensuring smooth transitions in case of disruptions (buffer
// underflow, overflow, packet loss). In case the computation takes too much time, the audio
// callback will gracefully handle an interruption, and the callback timing and sound wave
// continuity will not be affected.
pub async fn receive_samples_loop(
    mut receiver: StreamReceiver<()>,
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    channels_count: usize,
    batch_frames_count: usize,
    average_buffer_frames_count: usize,
) -> StrResult {
    // Pre-allocate for cross-fade operations (batch_frames_count * channels_count samples)
    let mut recovery_sample_buffer = Vec::with_capacity(batch_frames_count * channels_count);
    loop {
        let packet = receiver.recv().await?;
        let mut sample_buffer_ref = sample_buffer.lock();

        if packet.had_packet_loss {
            info!("Audio packet loss!");

            if sample_buffer_ref.len() / channels_count < batch_frames_count {
                sample_buffer_ref.clear();
            } else {
                // clear remaining samples
                sample_buffer_ref.drain(batch_frames_count * channels_count..);
            }

            recovery_sample_buffer.clear();
        }

        if sample_buffer_ref.len() / channels_count < batch_frames_count {
            recovery_sample_buffer.extend(sample_buffer_ref.drain(..));
        }

        if sample_buffer_ref.len() == 0 || packet.had_packet_loss {
            recovery_sample_buffer.extend(
                packet
                    .buffer
                    .chunks_exact(2)
                    .map(|c| i16::from_ne_bytes([c[0], c[1]]).to_f32()),
            );

            if recovery_sample_buffer.len() / channels_count
                > average_buffer_frames_count + batch_frames_count
            {
                // Fade-in
                for f in 0..batch_frames_count {
                    let volume = f as f32 / batch_frames_count as f32;
                    for c in 0..channels_count {
                        recovery_sample_buffer[f * channels_count + c] *= volume;
                    }
                }

                if packet.had_packet_loss
                    && sample_buffer_ref.len() / channels_count == batch_frames_count
                {
                    // Add a fade-out to make a cross-fade.
                    for f in 0..batch_frames_count {
                        let volume = 1. - f as f32 / batch_frames_count as f32;
                        for c in 0..channels_count {
                            recovery_sample_buffer[f * channels_count + c] +=
                                sample_buffer_ref[f * channels_count + c] * volume;
                        }
                    }

                    sample_buffer_ref.clear();
                }

                sample_buffer_ref.extend(recovery_sample_buffer.drain(..));
                info!("Audio recovered");
            }
        } else {
            sample_buffer_ref.extend(
                packet
                    .buffer
                    .chunks_exact(2)
                    .map(|c| i16::from_ne_bytes([c[0], c[1]]).to_f32()),
            );
        }

        // todo: use smarter policy with EventTiming
        let buffer_frames_size = sample_buffer_ref.len() / channels_count;
        if buffer_frames_size > 2 * average_buffer_frames_count + batch_frames_count {
            info!("Audio buffer overflow! size: {buffer_frames_size}");

            // Ensure we keep at least batch_frames_count for the cross-fade
            let target_frames = average_buffer_frames_count.max(batch_frames_count);
            let drain_count = (buffer_frames_size - target_frames) * channels_count;
            recovery_sample_buffer.clear();
            recovery_sample_buffer.extend(
                sample_buffer_ref
                    .iter()
                    .take(batch_frames_count * channels_count)
                    .copied(),
            );

            sample_buffer_ref.drain(0..drain_count);

            // Render a cross-fade.
            for f in 0..batch_frames_count {
                let volume = f as f32 / batch_frames_count as f32;
                for c in 0..channels_count {
                    let index = f * channels_count + c;
                    sample_buffer_ref[index] = sample_buffer_ref[index] * volume
                        + recovery_sample_buffer[index] * (1. - volume);
                }
            }
        }
    }
}
