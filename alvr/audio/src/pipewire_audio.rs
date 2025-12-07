//! PipeWire audio backend for Linux.
//!
//! This module provides audio capture and playback functionality using the PipeWire
//! multimedia framework. It integrates with the existing audio infrastructure
//! and provides native support for modern Linux audio stacks.
//!
//! # Architecture
//!
//! Both capture and playback spawn a dedicated thread running a PipeWire main loop.
//! Communication between the async runtime and PipeWire threads uses:
//! - `pw::channel` for shutdown signaling (async -> PipeWire)
//! - `tokio::sync::mpsc` for audio data (PipeWire -> async, capture only)
//! - `Arc<Mutex<VecDeque<f32>>>` for shared sample buffer (playback only)

use std::{cell::RefCell, collections::VecDeque, io::Cursor, mem, rc::Rc, sync::Arc, thread};

use alvr_common::prelude::*;
use alvr_session::AudioConfig;
use alvr_sockets::{StreamReceiver, StreamSender};
use parking_lot::Mutex;
use pipewire::{
    self as pw,
    context::ContextRc,
    main_loop::MainLoopRc,
    spa::{
        param::audio::{AudioFormat, AudioInfoRaw},
        pod::{Object, Pod, Property, Value, serialize::PodSerializer},
        sys::{SPA_PARAM_EnumFormat, SPA_TYPE_OBJECT_Format},
        utils::Direction,
    },
    stream::{StreamFlags, StreamRc, StreamState},
};
use tokio::sync::mpsc as tmpsc;

use crate::{AudioDeviceType, AudioDevicesList, get_next_frame_batch, receive_samples_loop};

/// Zero-sized shutdown signal sent to PipeWire threads.
struct Shutdown;

/// RAII guard that sends a shutdown signal on drop.
///
/// This ensures the PipeWire thread exits cleanly even if the async task
/// is cancelled or panics. The `Option` allows taking ownership in `Drop`.
struct ShutdownSender(Option<pw::channel::Sender<Shutdown>>);

impl Drop for ShutdownSender {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(Shutdown);
        }
    }
}

/// Represents a PipeWire audio device.
///
/// Note: PipeWire handles actual device routing at the session manager level
/// (e.g., WirePlumber). This struct stores the requested device identifier,
/// but actual routing is configured externally via tools like pavucontrol or qpwgraph.
pub struct PipeWireAudioDevice {
    name: String,
    #[allow(dead_code)]
    device_type: AudioDeviceType,
}

impl PipeWireAudioDevice {
    /// Creates a new PipeWire audio device wrapper.
    ///
    /// The device ID is stored for reference, but actual device routing is handled
    /// by PipeWire's session manager. Streams connect to the default device and
    /// users can reroute via external tools.
    pub fn new(id: alvr_session::AudioDeviceId, device_type: AudioDeviceType) -> StrResult<Self> {
        let name = match id {
            alvr_session::AudioDeviceId::Default => "Default".to_string(),
            alvr_session::AudioDeviceId::Name(name) => name,
            alvr_session::AudioDeviceId::Index(idx) => format!("Device {idx}"),
        };

        Ok(Self { name, device_type })
    }

    /// Returns the device name.
    pub fn name(&self) -> StrResult<String> {
        Ok(self.name.clone())
    }

    /// Checks if two devices refer to the same audio endpoint.
    pub fn is_same_device(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// Returns the list of available audio devices.
///
/// Currently returns only "Default" since PipeWire handles device routing
/// at the session manager level. Users configure routing externally.
#[inline(always)]
pub fn get_devices_list() -> StrResult<AudioDevicesList> {
    Ok(AudioDevicesList {
        output: vec!["Default".to_string()],
        input: vec!["Default".to_string()],
    })
}

/// Returns the sample rate for the given device.
///
/// Returns 48000 Hz as the default, which is widely supported and matches
/// PipeWire's typical default configuration.
#[inline(always)]
pub fn get_sample_rate(_device: &PipeWireAudioDevice) -> StrResult<u32> {
    Ok(48000)
}

/// Build an audio format POD for PipeWire stream negotiation.
///
/// Creates a serialized POD object containing audio format parameters
/// that PipeWire uses to negotiate the stream format.
fn build_audio_format_pod(
    buffer: &mut [u8],
    format: AudioFormat,
    sample_rate: u32,
    channels: u32,
) -> Result<usize, String> {
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(format);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(channels);

    let properties: Vec<Property> = audio_info.into();
    let object = Object {
        type_: SPA_TYPE_OBJECT_Format,
        id: SPA_PARAM_EnumFormat,
        properties,
    };

    let (cursor, _) =
        PodSerializer::serialize(Cursor::new(&mut buffer[..]), &Value::Object(object))
            .map_err(|e| format!("Failed to serialize audio format POD: {e:?}"))?;

    Ok(cursor.position() as usize)
}

/// Record audio using PipeWire.
///
/// Captures audio from the default input device and sends it through the provided sender.
pub async fn record_audio_loop(
    device: PipeWireAudioDevice,
    channels_count: u16,
    _mute: bool,
    mut sender: StreamSender<()>,
) -> StrResult {
    let sample_rate = get_sample_rate(&device)?;

    let (data_tx, mut data_rx) = tmpsc::unbounded_channel::<StrResult<Vec<u8>>>();
    let (shutdown_tx, shutdown_rx) = pw::channel::channel::<Shutdown>();

    let handle = thread::spawn(move || {
        if let Err(e) = run_capture_loop(channels_count, sample_rate, data_tx, shutdown_rx) {
            error!("PipeWire capture error: {e}");
        }
    });

    // Guard ensures shutdown is sent even if this async task is cancelled
    let shutdown_tx = ShutdownSender(Some(shutdown_tx));

    while let Some(result) = data_rx.recv().await {
        let data = result?;
        let mut buffer = sender.new_buffer(&(), data.len())?;
        buffer.get_mut().extend(&data);
        sender.send_buffer(buffer).await.ok();
    }

    drop(shutdown_tx);
    handle.join().ok();
    Ok(())
}

/// Runs the PipeWire capture loop on a dedicated thread.
///
/// This function blocks until shutdown is signaled or an error occurs.
/// Audio data is sent through `data_tx` as raw bytes in S16LE format.
fn run_capture_loop(
    channels_count: u16,
    sample_rate: u32,
    data_tx: tmpsc::UnboundedSender<StrResult<Vec<u8>>>,
    shutdown_rx: pw::channel::Receiver<Shutdown>,
) -> StrResult {
    // Initialize PipeWire library for this thread
    pw::init();

    let mainloop =
        MainLoopRc::new(None).map_err(|e| format!("Failed to create PipeWire main loop: {e}"))?;
    let context = ContextRc::new(&mainloop, None)
        .map_err(|e| format!("Failed to create PipeWire context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("Failed to connect to PipeWire: {e}"))?;

    // Stream properties for session manager routing and identification
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => "ALXR Audio Capture",
        *pw::keys::APP_NAME => "ALXR",
    };

    let stream = StreamRc::new(core, "alxr-audio-capture", props)
        .map_err(|e| format!("Failed to create PipeWire stream: {e}"))?;

    // Attach shutdown receiver to quit main loop when signaled
    let _shutdown = shutdown_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let data_tx = Rc::new(data_tx);

    let _listener = stream
        .add_local_listener::<()>()
        .state_changed({
            let mainloop = mainloop.clone();
            move |_, _, old, new| {
                debug!("PipeWire capture: {old:?} -> {new:?}");
                if matches!(new, StreamState::Error(_)) {
                    error!("PipeWire capture stream entered error state");
                    mainloop.quit();
                }
            }
        })
        .process({
            let data_tx = Rc::clone(&data_tx);
            move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                // Read chunk metadata before accessing mutable data
                let data = &mut datas[0];
                let size = data.chunk().size() as usize;
                let offset = data.chunk().offset() as usize;

                if let Some(audio_data) = data.data() {
                    if size > 0 && offset + size <= audio_data.len() {
                        // Note: Allocation here is unavoidable since we need to send
                        // owned data across the channel to the async task.
                        // The Vec is sized exactly to the audio chunk size.
                        let mut samples = Vec::with_capacity(size);
                        samples.extend_from_slice(&audio_data[offset..offset + size]);
                        let _ = data_tx.send(Ok(samples));
                    }
                }
            }
        })
        .register()
        .map_err(|e| format!("Failed to register stream listener: {e}"))?;

    // Build and connect with audio format
    let mut pod_buffer = [0u8; 1024];
    let pod_size = build_audio_format_pod(
        &mut pod_buffer,
        AudioFormat::S16LE,
        sample_rate,
        channels_count.into(),
    )?;
    let pod = Pod::from_bytes(&pod_buffer[..pod_size]).ok_or("Failed to create Pod from bytes")?;

    // AUTOCONNECT: Let session manager route to default device
    // MAP_BUFFERS: Map buffer memory for direct access
    // RT_PROCESS: Enable real-time processing in the audio thread
    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;
    stream
        .connect(Direction::Input, None, flags, &mut [pod])
        .map_err(|e| format!("Failed to connect PipeWire capture stream: {e}"))?;

    info!("PipeWire capture stream connected");
    mainloop.run();
    info!("PipeWire capture loop exited");

    stream.disconnect().ok();
    Ok(())
}

/// Play audio using PipeWire.
///
/// Receives audio samples and plays them through the default output device.
pub async fn play_audio_loop(
    _device: PipeWireAudioDevice,
    channels_count: u16,
    sample_rate: u32,
    config: AudioConfig,
    receiver: StreamReceiver<()>,
) -> StrResult {
    let batch_frames_count = sample_rate as usize * config.batch_ms as usize / 1000;
    let average_buffer_frames_count =
        sample_rate as usize * config.average_buffering_ms as usize / 1000;

    let sample_buffer = Arc::new(Mutex::new(VecDeque::new()));
    let sample_buffer_clone = Arc::clone(&sample_buffer);

    let channels = channels_count as usize;

    let (shutdown_tx, shutdown_rx) = pw::channel::channel::<Shutdown>();

    let handle = thread::spawn(move || {
        if let Err(e) = run_playback_loop(
            channels,
            sample_rate,
            batch_frames_count,
            sample_buffer_clone,
            shutdown_rx,
        ) {
            error!("PipeWire playback error: {e}");
        }
    });

    // Guard ensures shutdown is sent even if this async task is cancelled
    let shutdown_tx = ShutdownSender(Some(shutdown_tx));

    let result = receive_samples_loop(
        receiver,
        sample_buffer,
        channels_count as _,
        batch_frames_count,
        average_buffer_frames_count,
    )
    .await;

    drop(shutdown_tx);
    handle.join().ok();

    result
}

/// Runs the PipeWire playback loop on a dedicated thread.
///
/// This function blocks until shutdown is signaled or an error occurs.
/// Audio samples are read from the shared `sample_buffer` and written
/// to PipeWire in F32LE format.
fn run_playback_loop(
    channels: usize,
    sample_rate: u32,
    batch_frames_count: usize,
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
    shutdown_rx: pw::channel::Receiver<Shutdown>,
) -> StrResult {
    // Initialize PipeWire library for this thread
    pw::init();

    let mainloop =
        MainLoopRc::new(None).map_err(|e| format!("Failed to create PipeWire main loop: {e}"))?;
    let context = ContextRc::new(&mainloop, None)
        .map_err(|e| format!("Failed to create PipeWire context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("Failed to connect to PipeWire: {e}"))?;

    // Stream properties for session manager routing and identification
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Game",
        *pw::keys::NODE_NAME => "ALXR Audio Playback",
        *pw::keys::APP_NAME => "ALXR",
    };

    let stream = StreamRc::new(core, "alxr-audio-playback", props)
        .map_err(|e| format!("Failed to create PipeWire stream: {e}"))?;

    // Attach shutdown receiver to quit main loop when signaled
    let _shutdown = shutdown_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    // Use Rc to share Arc with the callback
    let sample_buffer_rc = Rc::new(sample_buffer);
    // Pre-allocate temp buffer with expected capacity to avoid reallocations in RT callback
    let initial_capacity = batch_frames_count * channels;
    let temp_buffer = Rc::new(RefCell::new(Vec::<f32>::with_capacity(initial_capacity)));
    let bytes_per_sample = mem::size_of::<f32>();
    let bytes_per_frame = channels * bytes_per_sample;

    let _listener = stream
        .add_local_listener::<()>()
        .state_changed({
            let mainloop = mainloop.clone();
            move |_, _, old, new| {
                debug!("PipeWire playback: {old:?} -> {new:?}");
                if matches!(new, StreamState::Error(_)) {
                    error!("PipeWire playback stream entered error state");
                    mainloop.quit();
                }
            }
        })
        .process({
            let sample_buffer = Rc::clone(&sample_buffer_rc);
            let temp_buffer = Rc::clone(&temp_buffer);
            move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let data = &mut datas[0];

                // For output streams, we write to the data buffer
                let Some(audio_data) = data.data() else {
                    return;
                };

                // Calculate how many frames we can write
                let max_frames = audio_data.len() / bytes_per_frame;
                if max_frames == 0 {
                    return;
                }

                // Request frames from our sample buffer (use batch size or max available)
                let frames_to_write = batch_frames_count.min(max_frames);

                // Get frames from our sample buffer
                let mut temp = temp_buffer.borrow_mut();
                get_next_frame_batch(
                    &mut *sample_buffer.lock(),
                    channels,
                    frames_to_write,
                    &mut temp,
                );

                // Write f32 samples directly to the buffer as bytes
                let samples_to_write = temp.len();
                let bytes_to_write = samples_to_write * bytes_per_sample;

                // SAFETY: temp is a Vec<f32> with valid alignment. We reinterpret the
                // underlying memory as bytes for a memcpy. The bytes_to_write is correctly
                // calculated as samples_to_write * size_of::<f32>().
                let sample_bytes: &[u8] = unsafe {
                    std::slice::from_raw_parts(temp.as_ptr() as *const u8, bytes_to_write)
                };

                // Copy to output buffer
                let copy_len = bytes_to_write.min(audio_data.len());
                audio_data[..copy_len].copy_from_slice(&sample_bytes[..copy_len]);

                // Update chunk to indicate how much data we wrote
                let chunk = data.chunk_mut();
                *chunk.size_mut() = copy_len as u32;
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = bytes_per_frame as i32;
            }
        })
        .register()
        .map_err(|e| format!("Failed to register stream listener: {e}"))?;

    // Build and connect with audio format
    let mut pod_buffer = [0u8; 1024];
    let pod_size = build_audio_format_pod(
        &mut pod_buffer,
        AudioFormat::F32LE,
        sample_rate,
        channels as u32,
    )?;
    let pod = Pod::from_bytes(&pod_buffer[..pod_size]).ok_or("Failed to create Pod from bytes")?;

    // AUTOCONNECT: Let session manager route to default device
    // MAP_BUFFERS: Map buffer memory for direct access
    // RT_PROCESS: Enable real-time processing in the audio thread
    let flags = StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;
    stream
        .connect(Direction::Output, None, flags, &mut [pod])
        .map_err(|e| format!("Failed to connect PipeWire playback stream: {e}"))?;

    info!("PipeWire playback stream connected");
    mainloop.run();
    info!("PipeWire playback loop exited");

    stream.disconnect().ok();
    Ok(())
}
