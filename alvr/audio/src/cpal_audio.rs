use alvr_common::{lazy_static, prelude::*};
use alvr_session::{AudioConfig, AudioDeviceId, LinuxAudioBackend};
use alvr_sockets::{AUDIO, SenderBuffer, StreamReceiver, StreamSender};
use cpal::{
    BufferSize, Device, Sample, SampleFormat, StreamConfig, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use parking_lot::Mutex;
use rodio::Source;
use std::{
    collections::VecDeque,
    num::NonZero,
    sync::{Arc, mpsc as smpsc},
    thread,
};
use tokio::sync::mpsc as tmpsc;

#[cfg(windows)]
use windows::Win32::{
    Devices::{FunctionDiscovery::PKEY_Device_FriendlyName, Properties::DEVPKEY_Device_DeviceDesc},
    Media::Audio::{
        DEVICE_STATE_ACTIVE, Endpoints::IAudioEndpointVolume, IMMDevice, IMMDeviceCollection,
        IMMDeviceEnumerator, MMDeviceEnumerator, eAll,
    },
    System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        STGM_READ,
    },
    System::Variant::VT_LPWSTR,
    UI::Shell::PropertiesSystem::IPropertyStore,
};

use crate::{AudioDeviceType, AudioDevicesList, get_next_frame_batch, receive_samples_loop};

lazy_static! {
    static ref VIRTUAL_MICROPHONE_PAIRS: Vec<(String, String)> = vec![
        ("CABLE Input".into(), "CABLE Output".into()),
        ("VoiceMeeter Input".into(), "VoiceMeeter Output".into()),
        (
            "VoiceMeeter Aux Input".into(),
            "VoiceMeeter Aux Output".into()
        ),
        (
            "VoiceMeeter VAIO3 Input".into(),
            "VoiceMeeter VAIO3 Output".into()
        ),
    ];
}

#[inline(always)]
fn device_name(device: &Device) -> Result<String, cpal::DeviceNameError> {
    device.description().map(|desc| desc.name().to_string())
}

#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn get_devices_list(linux_backend: LinuxAudioBackend) -> StrResult<AudioDevicesList> {
    #[cfg(target_os = "linux")]
    let host = match linux_backend {
        LinuxAudioBackend::Alsa => cpal::host_from_id(cpal::HostId::Alsa).unwrap(),
        LinuxAudioBackend::Jack => cpal::host_from_id(cpal::HostId::Jack).unwrap(),
        LinuxAudioBackend::PipeWire => unreachable!(),
    };
    #[cfg(not(target_os = "linux"))]
    let host = cpal::default_host();

    let output = trace_err!(host.output_devices())?
        .filter_map(|d| device_name(&d).ok())
        .collect::<Vec<_>>();
    let input = trace_err!(host.input_devices())?
        .filter_map(|d| device_name(&d).ok())
        .collect::<Vec<_>>();

    Ok(AudioDevicesList { output, input })
}

pub struct CpalAudioDevice {
    inner: Device,

    device_type: AudioDeviceType,
}

#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
impl CpalAudioDevice {
    pub fn new(
        linux_backend: LinuxAudioBackend,
        id: AudioDeviceId,
        device_type: AudioDeviceType,
    ) -> StrResult<Self> {
        #[cfg(target_os = "linux")]
        let host = match linux_backend {
            LinuxAudioBackend::Alsa => cpal::host_from_id(cpal::HostId::Alsa).unwrap(),
            LinuxAudioBackend::Jack => cpal::host_from_id(cpal::HostId::Jack).unwrap(),
            LinuxAudioBackend::PipeWire => unreachable!(),
        };
        #[cfg(not(target_os = "linux"))]
        let host = cpal::default_host();

        let device = match &id {
            AudioDeviceId::Default => match &device_type {
                AudioDeviceType::Output => host
                    .default_output_device()
                    .ok_or_else(|| "No output audio device found".to_owned())?,
                AudioDeviceType::Input => host
                    .default_input_device()
                    .ok_or_else(|| "No input audio device found".to_owned())?,
                AudioDeviceType::VirtualMicrophoneInput => trace_err!(host.output_devices())?
                    .find(|d| {
                        if let Ok(name) = device_name(d) {
                            VIRTUAL_MICROPHONE_PAIRS
                                .iter()
                                .any(|(input_name, _)| name.contains(input_name))
                        } else {
                            false
                        }
                    })
                    .ok_or_else(|| {
                        "VB-CABLE or Voice Meeter not found. Please install or reinstall either one"
                            .to_owned()
                    })?,
                AudioDeviceType::VirtualMicrophoneOutput {
                    matching_input_device_name,
                } => {
                    let maybe_output_name = VIRTUAL_MICROPHONE_PAIRS
                        .iter()
                        .find(|(input_name, _)| matching_input_device_name.contains(input_name))
                        .map(|(_, output_name)| output_name);
                    if let Some(output_name) = maybe_output_name {
                        trace_err!(host.input_devices())?
                            .find(|d| {
                                if let Ok(name) = device_name(d) {
                                    name.contains(output_name)
                                } else {
                                    false
                                }
                            })
                            .ok_or_else(|| {
                                "Matching output microphone not found. Did you rename it?"
                                    .to_owned()
                            })?
                    } else {
                        return fmt_e!(
                            "Selected input microphone device is unknown. {}",
                            "Please manually select the matching output microphone device."
                        );
                    }
                }
            },
            AudioDeviceId::Name(name_substring) => trace_err!(host.devices())?
                .find(|d| {
                    if let Ok(name) = device_name(d) {
                        name.to_lowercase().contains(&name_substring.to_lowercase())
                    } else {
                        false
                    }
                })
                .ok_or_else(|| {
                    format!("Cannot find audio device which name contains \"{name_substring}\"")
                })?,
            AudioDeviceId::Index(index) => trace_err!(host.devices())?
                .nth(*index as usize - 1)
                .ok_or_else(|| format!("Cannot find audio device at index {index}"))?,
        };

        Ok(Self {
            inner: device,

            device_type,
        })
    }

    #[inline(always)]
    pub fn name(&self) -> StrResult<String> {
        trace_err!(device_name(&self.inner))
    }

    #[inline(always)]
    pub fn is_same_device(&self, other: &Self) -> bool {
        if let (Ok(name1), Ok(name2)) = (self.name(), other.name()) {
            name1 == name2
        } else {
            false
        }
    }
}

#[cfg(windows)]
/// # Safety
/// `key` must point to a valid `PROPERTYKEY` or a structurally compatible type
/// such as `DEVPROPKEY`.
unsafe fn get_device_property_string(
    property_store: &IPropertyStore,
    key: *const windows::Win32::Foundation::PROPERTYKEY,
) -> Option<String> {
    unsafe {
        let prop_variant = property_store.GetValue(key).ok()?;
        if prop_variant.vt() != VT_LPWSTR {
            return None;
        }
        prop_variant
            .Anonymous
            .Anonymous
            .Anonymous
            .pwszVal
            .to_string()
            .ok()
    }
}

#[cfg(windows)]
fn get_windows_device(device: &CpalAudioDevice) -> StrResult<IMMDevice> {
    let dev_name = trace_err!(device.name())?;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let mm_device_enumerator: IMMDeviceEnumerator =
            trace_err!(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL))?;

        let mm_device_collection: IMMDeviceCollection =
            trace_err!(mm_device_enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE))?;

        let count = trace_err!(mm_device_collection.GetCount())?;

        for i in 0..count {
            let mm_device: IMMDevice = trace_err!(mm_device_collection.Item(i))?;

            let property_store: IPropertyStore =
                trace_err!(mm_device.OpenPropertyStore(STGM_READ))?;

            // cpal 0.17 prefers DEVPKEY_Device_DeviceDesc, falling back to
            // PKEY_Device_FriendlyName. Match the same logic so the name
            // returned by cpal's description().name() can be found here.
            let mm_device_name = get_device_property_string(
                &property_store,
                &DEVPKEY_Device_DeviceDesc as *const _ as *const _,
            )
            .or_else(|| get_device_property_string(&property_store, &PKEY_Device_FriendlyName));

            if mm_device_name.as_ref() == Some(&dev_name) {
                return Ok(mm_device);
            }
        }

        fmt_e!("No device found with specified name")
    }
}

#[cfg(windows)]
pub fn get_windows_device_id(device: &CpalAudioDevice) -> StrResult<String> {
    unsafe {
        let mm_device = get_windows_device(device)?;

        let id_pwstr = trace_err!(mm_device.GetId())?;
        let id_str = trace_err!(id_pwstr.to_string())?;
        CoTaskMemFree(Some(id_pwstr.0 as _));

        Ok(id_str)
    }
}

// device must be an output device
#[cfg(windows)]
fn set_mute_windows_device(device: &CpalAudioDevice, mute: bool) -> StrResult {
    unsafe {
        let mm_device = get_windows_device(device)?;

        let endpoint_volume: IAudioEndpointVolume =
            trace_err!(mm_device.Activate(CLSCTX_ALL, None))?;

        trace_err!(endpoint_volume.SetMute(mute, std::ptr::null()))?;
    }

    Ok(())
}

fn get_stream_config(device: &CpalAudioDevice) -> StrResult<SupportedStreamConfig> {
    trace_err!(if device.device_type.is_output() {
        device
            .inner
            .default_output_config()
            .or_else(|_| device.inner.default_input_config())
    } else {
        device
            .inner
            .default_input_config()
            .or_else(|_| device.inner.default_output_config())
    })
}

pub fn get_sample_rate(device: &CpalAudioDevice) -> StrResult<u32> {
    let config = get_stream_config(device)?;
    Ok(config.sample_rate())
}

#[cfg(windows)]
struct MuteGuard<'a> {
    device: &'a CpalAudioDevice,
}

#[cfg(windows)]
impl<'a> Drop for MuteGuard<'a> {
    fn drop(&mut self) {
        set_mute_windows_device(self.device, false).ok();
    }
}

#[cfg_attr(not(windows), allow(unused_variables))]
pub async fn record_audio_loop(
    device: CpalAudioDevice,
    channels_count: u16,
    mute: bool,
    mut sender: StreamSender<()>,
) -> StrResult {
    let config = get_stream_config(&device)?;

    if config.channels() > 2 {
        return fmt_e!(
            "Audio devices with more than 2 channels are not supported. {}",
            "Please turn off surround audio."
        );
    }

    let stream_config = StreamConfig {
        channels: config.channels(),
        sample_rate: config.sample_rate(),
        buffer_size: BufferSize::Default,
    };

    // data_sender/receiver is the bridge between tokio and std thread
    let (data_sender, mut data_receiver) =
        tmpsc::unbounded_channel::<StrResult<SenderBuffer<()>>>();
    let (_shutdown_notifier, shutdown_receiver) = smpsc::channel::<()>();
    let (recycle_sender, recycle_receiver) = smpsc::channel::<SenderBuffer<()>>();

    let thread_callback = {
        let data_sender = data_sender.clone();
        move || {
            #[cfg(windows)]
            let _mute_guard = if mute && device.device_type.is_output() {
                set_mute_windows_device(&device, true).ok();
                Some(MuteGuard { device: &device })
            } else {
                None
            };

            let stream = trace_err!(device.inner.build_input_stream_raw(
                &stream_config,
                config.sample_format(),
                {
                    let data_sender = data_sender.clone();
                    move |data, _| {
                        // Get recycled buffer or create new one (grows organically like Vec)
                        let mut buffer = recycle_receiver
                            .try_recv()
                            .unwrap_or_else(|_| SenderBuffer::<()>::new(AUDIO, 0).unwrap());

                        // encode() clears buffer and returns lock to payload portion
                        let mut samples = buffer.encode(&()).unwrap();

                        let input_channels = config.channels();
                        let output_channels = channels_count;
                        let data_bytes = data.bytes();

                        if config.sample_format() == SampleFormat::F32 {
                            let frames = data_bytes.len() / (4 * input_channels as usize);
                            let required_capacity = frames * output_channels as usize * 2;
                            let current_len = samples.len();
                            if samples.capacity() < required_capacity {
                                samples.reserve(required_capacity - current_len);
                            }

                            #[inline(always)]
                            fn to_i16_bytes(b: &[u8]) -> [u8; 2] {
                                f32::from_ne_bytes([b[0], b[1], b[2], b[3]])
                                    .to_sample::<i16>()
                                    .to_ne_bytes()
                            }

                            if input_channels == 1 && output_channels == 2 {
                                for chunk in data_bytes.chunks_exact(4) {
                                    let s = to_i16_bytes(chunk);
                                    samples.extend_from_slice(&s);
                                    samples.extend_from_slice(&s);
                                }
                            } else if input_channels == 2 && output_channels == 1 {
                                // Average both channels for proper stereo-to-mono downmix
                                for chunk in data_bytes.chunks_exact(8) {
                                    let l = f32::from_ne_bytes([
                                        chunk[0], chunk[1], chunk[2], chunk[3],
                                    ]);
                                    let r = f32::from_ne_bytes([
                                        chunk[4], chunk[5], chunk[6], chunk[7],
                                    ]);
                                    let mixed = ((l + r) * 0.5).to_sample::<i16>();
                                    samples.extend_from_slice(&mixed.to_ne_bytes());
                                }
                            } else {
                                for chunk in data_bytes.chunks_exact(4) {
                                    let s = to_i16_bytes(chunk);
                                    samples.extend_from_slice(&s);
                                }
                            }
                        } else {
                            let frames = data_bytes.len() / (2 * input_channels as usize);
                            let required_capacity = frames * output_channels as usize * 2;
                            let current_len = samples.len();
                            if samples.capacity() < required_capacity {
                                samples.reserve(required_capacity - current_len);
                            }

                            if input_channels == 1 && output_channels == 2 {
                                for chunk in data_bytes.chunks_exact(2) {
                                    samples.extend_from_slice(chunk);
                                    samples.extend_from_slice(chunk);
                                }
                            } else if input_channels == 2 && output_channels == 1 {
                                // Average both channels for proper stereo-to-mono downmix
                                for chunk in data_bytes.chunks_exact(4) {
                                    let l = i16::from_ne_bytes([chunk[0], chunk[1]]);
                                    let r = i16::from_ne_bytes([chunk[2], chunk[3]]);
                                    // Use i32 to avoid overflow, then divide
                                    let mixed = ((l as i32 + r as i32) / 2) as i16;
                                    samples.extend_from_slice(&mixed.to_ne_bytes());
                                }
                            } else {
                                samples.extend_from_slice(data_bytes);
                            }
                        }

                        drop(samples); // Release lock before sending
                        data_sender.send(Ok(buffer)).ok();
                    }
                },
                {
                    let data_sender = data_sender.clone();
                    move |e| {
                        data_sender
                            .send(fmt_e!("Error while recording audio: {e}"))
                            .ok();
                    }
                },
                None
            ))?;

            trace_err!(stream.play())?;

            shutdown_receiver.recv().ok();

            Ok(())
        }
    };

    // use a std thread to store the stream object. The stream object must be destroyed on the same
    // thread of creation.
    thread::spawn(move || {
        if let Err(e) = thread_callback() {
            data_sender.send(Err(e)).ok();
        }
    });

    // Receive pre-filled buffers from callback, send over network, recycle
    while let Some(maybe_buffer) = data_receiver.recv().await {
        let mut buffer = maybe_buffer?;
        sender.send_buffer_ref(&mut buffer).await.ok();
        recycle_sender.send(buffer).ok();
    }

    Ok(())
}

struct StreamingSource {
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    current_batch: Vec<f32>,
    current_batch_cursor: usize,
    channels_count: usize,
    sample_rate: u32,
    batch_frames_count: usize,
}

impl Source for StreamingSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(self.channels_count as u16).unwrap()
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.sample_rate).unwrap()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Iterator for StreamingSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.current_batch_cursor == 0 {
            get_next_frame_batch(
                &mut self.sample_buffer.lock(),
                self.channels_count,
                self.batch_frames_count,
                &mut self.current_batch,
            );
        }

        let sample = self.current_batch[self.current_batch_cursor];

        self.current_batch_cursor =
            (self.current_batch_cursor + 1) % (self.batch_frames_count * self.channels_count);

        Some(sample)
    }
}

pub async fn play_audio_loop(
    device: CpalAudioDevice,
    channels_count: u16,
    sample_rate: u32,
    config: AudioConfig,
    receiver: StreamReceiver<()>,
) -> StrResult {
    // Size of a chunk of frames. It corresponds to the duration if a fade-in/out in frames.
    let batch_frames_count = sample_rate as usize * config.batch_ms as usize / 1000;

    // Average buffer size in frames
    let average_buffer_frames_count =
        sample_rate as usize * config.average_buffering_ms as usize / 1000;

    let sample_buffer = Arc::new(Mutex::new(VecDeque::new()));

    // Store the stream in a thread (because !Send)
    let (_shutdown_notifier, shutdown_receiver) = smpsc::channel::<()>();
    thread::spawn({
        let sample_buffer = Arc::clone(&sample_buffer);
        move || -> StrResult {
            let stream = trace_err!(
                rodio::DeviceSinkBuilder::from_device(device.inner.clone())
                    .and_then(|b| b.open_stream())
            )?;

            let source = StreamingSource {
                sample_buffer,
                current_batch: Vec::with_capacity(batch_frames_count * channels_count as usize),
                current_batch_cursor: 0,
                channels_count: channels_count as _,
                sample_rate,
                batch_frames_count,
            };
            stream.mixer().add(source);

            shutdown_receiver.recv().ok();
            Ok(())
        }
    });

    receive_samples_loop(
        receiver,
        sample_buffer,
        channels_count as _,
        batch_frames_count,
        average_buffer_frames_count,
    )
    .await
}
