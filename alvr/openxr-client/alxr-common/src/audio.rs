#[cfg(target_os = "android")]
mod android {
    use alvr_common::prelude::*;
    use alvr_session::AudioConfig;
    use alvr_sockets::{StreamReceiver, StreamSender};
    use oboe::{
        AudioInputCallback, AudioInputStreamSafe, AudioOutputCallback, AudioOutputStreamSafe,
        AudioStream, AudioStreamBase, AudioStreamBuilder, DataCallbackResult, InputPreset, Mono,
        PerformanceMode, SampleRateConversionQuality, Stereo, Usage,
    };
    use parking_lot::Mutex;
    use std::{
        collections::VecDeque,
        mem,
        sync::{Arc, mpsc as smpsc},
        thread,
    };
    use tokio::sync::mpsc as tmpsc;

    // Batch duration in milliseconds for client-side microphone capture.
    // 10ms at 48kHz = 480 frames = 960 bytes, which fits well under the network MTU.
    const MIC_BATCH_MS: u32 = 10;

    struct RecorderCallback {
        sender: tmpsc::UnboundedSender<Vec<u8>>,
        recycle_receiver: smpsc::Receiver<Vec<u8>>,
    }

    impl AudioInputCallback for RecorderCallback {
        type FrameType = (i16, Mono);

        fn on_audio_ready(
            &mut self,
            _: &mut dyn AudioInputStreamSafe,
            frames: &[i16],
        ) -> DataCallbackResult {
            let mut sample_buffer = self.recycle_receiver.try_recv().unwrap_or_default();
            sample_buffer.clear();
            sample_buffer.reserve(frames.len() * mem::size_of::<i16>());

            for frame in frames {
                sample_buffer.extend(&frame.to_ne_bytes());
            }

            self.sender.send(sample_buffer).ok();

            DataCallbackResult::Continue
        }
    }

    #[inline(always)]
    fn get_input_audio_stream_builder() -> AudioStreamBuilder<oboe::Input, oboe::Mono, i16> {
        AudioStreamBuilder::default()
            .set_shared()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sample_rate_conversion_quality(SampleRateConversionQuality::Fastest)
            .set_mono()
            .set_i16()
            .set_input()
            .set_usage(Usage::VoiceCommunication)
            .set_input_preset(InputPreset::VoiceCommunication)
    }

    #[inline(always)]
    pub fn get_input_sample_rate() -> StrResult<u32> {
        let stream = trace_err!(get_input_audio_stream_builder().open_stream())?;
        Ok(stream.get_sample_rate() as u32)
    }

    pub async fn record_audio_loop(mut sender: StreamSender<()>) -> StrResult {
        let (_shutdown_notifier, shutdown_receiver) = smpsc::channel::<()>();
        let (data_sender, mut data_receiver) = tmpsc::unbounded_channel();
        let (recycle_sender, recycle_receiver) = smpsc::channel();

        // Query the actual sample rate from the device
        let actual_sample_rate = get_input_sample_rate()?;

        // Calculate batch size in frames based on the actual sample rate
        let batch_frames_count = (actual_sample_rate * MIC_BATCH_MS / 1000) as i32;

        thread::spawn(move || -> StrResult {
            let mut stream = trace_err!(
                get_input_audio_stream_builder()
                    .set_frames_per_callback(batch_frames_count)
                    .set_callback(RecorderCallback {
                        sender: data_sender,
                        recycle_receiver,
                    })
                    .open_stream()
            )?;

            trace_err!(stream.start())?;

            shutdown_receiver.recv().ok();

            // This call gets stuck if the headset goes to sleep, but finishes when the headset wakes up
            stream.stop_with_timeout(0).ok();

            Ok(())
        });

        while let Some(data) = data_receiver.recv().await {
            let mut buffer = sender.new_buffer(&(), data.len())?;
            buffer.get_mut().extend(&data);
            sender.send_buffer(buffer).await.ok();
            recycle_sender.send(data).ok();
        }

        Ok(())
    }

    struct PlayerCallback {
        sample_buffer: Arc<Mutex<VecDeque<f32>>>,
        batch_frames_count: usize,
        temp_buffer: Vec<f32>,
    }

    impl AudioOutputCallback for PlayerCallback {
        type FrameType = (f32, Stereo);

        fn on_audio_ready(
            &mut self,
            _: &mut dyn AudioOutputStreamSafe,
            out_frames: &mut [(f32, f32)],
        ) -> DataCallbackResult {
            debug_assert_eq!(
                out_frames.len(),
                self.batch_frames_count,
                "Oboe callback buffer size mismatch"
            );
            alvr_audio::get_next_frame_batch(
                &mut *self.sample_buffer.lock(),
                2,
                self.batch_frames_count,
                &mut self.temp_buffer,
            );

            for f in 0..out_frames.len() {
                out_frames[f] = (self.temp_buffer[f * 2], self.temp_buffer[f * 2 + 1]);
            }

            DataCallbackResult::Continue
        }
    }
    pub async fn play_audio_loop(
        sample_rate: u32,
        config: AudioConfig,
        receiver: StreamReceiver<()>,
    ) -> StrResult {
        let batch_frames_count = sample_rate as usize * config.batch_ms as usize / 1000;
        let average_buffer_frames_count =
            sample_rate as usize * config.average_buffering_ms as usize / 1000;

        let sample_buffer = Arc::new(Mutex::new(VecDeque::new()));

        // store the stream in a thread (because !Send) and extract the playback handle
        let (_shutdown_notifier, shutdown_receiver) = smpsc::channel::<()>();
        thread::spawn({
            let sample_buffer = Arc::clone(&sample_buffer);
            move || -> StrResult {
                let mut stream = trace_err!(
                    AudioStreamBuilder::default()
                        .set_exclusive()
                        .set_performance_mode(PerformanceMode::LowLatency)
                        .set_sample_rate(sample_rate as _)
                        .set_sample_rate_conversion_quality(SampleRateConversionQuality::Fastest)
                        .set_stereo()
                        .set_f32()
                        .set_frames_per_callback(batch_frames_count as _)
                        .set_output()
                        .set_usage(Usage::Game)
                        .set_callback(PlayerCallback {
                            sample_buffer,
                            batch_frames_count,
                            temp_buffer: Vec::with_capacity(batch_frames_count * 2),
                        })
                        .open_stream()
                )?;

                trace_err!(stream.start())?;

                shutdown_receiver.recv().ok();

                // Note: Oboe crahes if stream.stop() is NOT called on AudioPlayer
                stream.stop_with_timeout(0).ok();

                Ok(())
            }
        });

        alvr_audio::receive_samples_loop(
            receiver,
            sample_buffer,
            2,
            batch_frames_count,
            average_buffer_frames_count,
        )
        .await
    }
}
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(target_os = "android"))]
mod non_android {
    use alvr_audio::{AudioDevice, AudioDeviceType};
    use alvr_common::prelude::*;
    use alvr_session::{AudioConfig, AudioDeviceId, LinuxAudioBackend};
    use alvr_sockets::{StreamReceiver, StreamSender};

    #[inline(always)]
    fn get_audio_backend() -> LinuxAudioBackend {
        #[cfg(target_os = "linux")]
        return crate::APP_CONFIG.audio_backend;
        #[cfg(not(target_os = "linux"))]
        return LinuxAudioBackend::PipeWire;
    }

    #[inline(always)]
    fn get_input_audio_device() -> StrResult<AudioDevice> {
        AudioDevice::new(
            get_audio_backend(),
            AudioDeviceId::Default,
            AudioDeviceType::Input,
        )
    }

    #[inline(always)]
    pub fn get_input_sample_rate() -> StrResult<u32> {
        let device = get_input_audio_device()?;
        Ok(alvr_audio::get_sample_rate(&device)? as u32)
    }

    #[inline(always)]
    pub async fn record_audio_loop(sender: StreamSender<()>) -> StrResult {
        let device = get_input_audio_device()?;
        alvr_audio::record_audio_loop(device, 1, false, sender).await
    }

    #[inline(always)]
    pub async fn play_audio_loop(
        sample_rate: u32,
        config: AudioConfig,
        receiver: StreamReceiver<()>,
    ) -> StrResult {
        let device = AudioDevice::new(
            get_audio_backend(),
            AudioDeviceId::Default,
            AudioDeviceType::Output,
        )?;
        alvr_audio::play_audio_loop(device, 2, sample_rate, config, receiver).await
    }
}
#[cfg(not(target_os = "android"))]
pub use non_android::*;
