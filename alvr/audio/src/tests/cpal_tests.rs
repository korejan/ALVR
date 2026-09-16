use super::*;

#[inline]
fn legacy_capture_data(
    data: &cpal::Data,
    input_channels: u16,
    output_channels: u16,
    samples: &mut alvr_sockets::SendBufferLock<'_>,
) {
    let data_bytes = data.bytes();
    if data.sample_format() == SampleFormat::F32 {
        let frames = data_bytes.len() / (4 * input_channels as usize);
        let required_capacity = frames * output_channels as usize * 2;
        let current_len = samples.len();
        if samples.capacity() < required_capacity {
            samples.reserve(required_capacity - current_len);
        }

        #[inline(always)]
        fn to_i16_bytes(bytes: &[u8]) -> [u8; 2] {
            f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                .to_sample::<i16>()
                .to_ne_bytes()
        }

        if input_channels == 1 && output_channels == 2 {
            for chunk in data_bytes.chunks_exact(4) {
                let sample = to_i16_bytes(chunk);
                samples.extend_from_slice(&sample);
                samples.extend_from_slice(&sample);
            }
        } else if input_channels == 2 && output_channels == 1 {
            for chunk in data_bytes.chunks_exact(8) {
                let left = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let right = f32::from_ne_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                let mixed = ((left + right) * 0.5).to_sample::<i16>();
                samples.extend_from_slice(&mixed.to_ne_bytes());
            }
        } else {
            for chunk in data_bytes.chunks_exact(4) {
                samples.extend_from_slice(&to_i16_bytes(chunk));
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
            for chunk in data_bytes.chunks_exact(4) {
                let left = i16::from_ne_bytes([chunk[0], chunk[1]]);
                let right = i16::from_ne_bytes([chunk[2], chunk[3]]);
                let mixed = ((left as i32 + right as i32) / 2) as i16;
                samples.extend_from_slice(&mixed.to_ne_bytes());
            }
        } else {
            samples.extend_from_slice(data_bytes);
        }
    }
}

fn assert_matches_legacy<T: cpal::SizedSample>(mut data: Vec<T>) {
    let capture_data =
        unsafe { cpal::Data::from_parts(data.as_mut_ptr().cast(), data.len(), T::FORMAT) };
    let mut legacy_buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
    let mut unified_buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
    for (input_channels, output_channels) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
        let mut expected = legacy_buffer.encode(&()).unwrap();
        let mut actual = unified_buffer.encode(&()).unwrap();
        legacy_capture_data(
            &capture_data,
            input_channels,
            output_channels,
            &mut expected,
        );
        convert_capture_data(&capture_data, input_channels, output_channels, &mut actual);
        assert_eq!(
            actual.as_ref(),
            expected.as_ref(),
            "{input_channels}->{output_channels}"
        );
        let capacity = actual.capacity();
        let pointer = actual.as_ptr();
        actual.clear();
        convert_capture_data(&capture_data, input_channels, output_channels, &mut actual);
        assert_eq!(actual.as_ref(), expected.as_ref());
        assert_eq!(actual.capacity(), capacity);
        assert_eq!(actual.as_ptr(), pointer);
    }
}

#[test]
fn unified_i16_matches_legacy_for_every_value_and_downmix_sum() {
    let mut data = Vec::new();
    for sample in i16::MIN..=i16::MAX {
        for other in [i16::MIN, i16::MAX, -1, 0, 1, sample, sample.wrapping_neg()] {
            data.extend_from_slice(&[sample, other]);
        }
    }
    assert_matches_legacy(data);
}

#[test]
fn unified_f32_matches_legacy_at_quantization_boundaries() {
    let mut data = Vec::new();
    for sample in i16::MIN..=i16::MAX {
        let boundary = sample as f32 / 32768.0;
        for value in [boundary.next_down(), boundary, boundary.next_up()] {
            for other in [-1.0, 0.0, 1.0, value, -value] {
                data.extend_from_slice(&[value, other]);
            }
        }
    }
    assert_matches_legacy(data);
}

#[test]
fn unified_f32_matches_legacy_for_arbitrary_bits() {
    let mut data = vec![
        f32::NAN,
        0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MAX,
        f32::MIN,
        f32::MIN,
        -0.0,
        0.0,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
    ];
    let mut state = 0x12345678_u32;
    for _ in 0..65536 {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        data.push(f32::from_bits(state));
    }
    assert_matches_legacy(data);
}

fn encode_native_samples<T: cpal::SizedSample>(
    data: &[T],
    input_channels: u16,
    output_channels: u16,
    buffer: &mut SenderBuffer<()>,
) -> Vec<i16> {
    let mut data = data.to_vec();
    let capture_data =
        unsafe { cpal::Data::from_parts(data.as_mut_ptr().cast(), data.len(), T::FORMAT) };
    let mut bytes = buffer.encode(&()).unwrap();
    convert_capture_data(&capture_data, input_channels, output_channels, &mut bytes);
    bytes
        .chunks_exact(2)
        .map(|sample| i16::from_ne_bytes([sample[0], sample[1]]))
        .collect()
}

fn converted_samples<T: cpal::SizedSample>(
    data: &[T],
    input_channels: u16,
    output_channels: u16,
) -> Vec<i16> {
    let mut buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
    encode_native_samples(data, input_channels, output_channels, &mut buffer)
}

#[test]
fn capture_preserves_native_format() {
    for format in [
        SampleFormat::F32,
        SampleFormat::I16,
        SampleFormat::I24,
        SampleFormat::I32,
        SampleFormat::F64,
    ] {
        let config =
            SupportedStreamConfig::new(2, 48000, cpal::SupportedBufferSize::Unknown, format);
        assert_eq!(get_capture_sample_format(&config).unwrap(), format);
    }
}

#[test]
fn capture_rejects_unsupported_format() {
    for format in [
        SampleFormat::I8,
        SampleFormat::I64,
        SampleFormat::U8,
        SampleFormat::U16,
        SampleFormat::U24,
        SampleFormat::U32,
        SampleFormat::U64,
        SampleFormat::DsdU8,
        SampleFormat::DsdU16,
        SampleFormat::DsdU32,
    ] {
        let config =
            SupportedStreamConfig::new(2, 48000, cpal::SupportedBufferSize::Unknown, format);
        assert!(get_capture_sample_format(&config).is_err(), "{format}");
    }
}

#[test]
fn capture_converts_i24_in_four_byte_containers() {
    let data =
        [-8388608, -4194304, 0, 4194304, 8388607].map(|sample| cpal::I24::new(sample).unwrap());
    assert_eq!(std::mem::size_of::<cpal::I24>(), 4);
    assert_eq!(
        converted_samples(&data, 1, 1),
        [-32768, -16384, 0, 16384, 32767]
    );
}

#[test]
fn capture_downmixes_i24_from_low_24_bits() {
    for left in (-8_388_608..=8_388_607).step_by(1021) {
        for right in [-8_388_608, -1, 0, 1, 8_388_607, left, left / 2] {
            let (left, right) = (
                cpal::I24::new(left).unwrap(),
                cpal::I24::new(right).unwrap(),
            );
            assert_eq!(
                downmix_i24_capture_samples(left, right),
                downmix_capture_samples(left, right)
            );
        }
    }

    // The same two stereo frames, without and with a sign-extended top byte
    let mut raw = [0x00c0_0000_i32, 0x00e0_0000, -4_194_304, -2_097_152];
    let data =
        unsafe { cpal::Data::from_parts(raw.as_mut_ptr().cast(), raw.len(), SampleFormat::I24) };
    let mut buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
    for (input, output, expected) in [
        (2, 1, &[-12288_i16, -12288][..]),
        (2, 2, &[-16384, -8192, -16384, -8192][..]),
    ] {
        let mut samples = buffer.encode(&()).unwrap();
        convert_capture_data(&data, input, output, &mut samples);
        let actual: Vec<i16> = samples
            .chunks_exact(2)
            .map(|bytes| i16::from_ne_bytes([bytes[0], bytes[1]]))
            .collect();
        assert_eq!(actual, expected, "{input}->{output}");
    }
}

#[test]
fn capture_converts_i32_and_duplicates_mono() {
    assert_eq!(
        converted_samples(&[i32::MIN, 0, i32::MAX], 1, 2),
        [-32768, -32768, 0, 0, 32767, 32767],
    );
}

#[test]
fn capture_converts_f64_and_preserves_stereo_order() {
    assert_eq!(
        converted_samples(&[-1.0_f64, 0.5, -0.5, 0.0], 2, 2),
        [-32768, 16384, -16384, 0],
    );
}

#[test]
fn capture_downmixes_before_reducing_precision() {
    assert_eq!(
        converted_samples(&[0.5_f64, -0.5, 0.5, 0.0], 2, 1),
        [0, 8192]
    );
    assert_eq!(converted_samples(&[i32::MAX, i32::MAX], 2, 1), [32767]);
    assert_eq!(converted_samples(&[i32::MIN, i32::MIN], 2, 1), [-32768]);
    assert_eq!(converted_samples(&[65535_i32, 1], 2, 1), [0]);
}

#[test]
fn native_capture_covers_all_mono_stereo_combinations() {
    fn check_channels<T: cpal::SizedSample>(data: &[T]) {
        for (input, output) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
            assert!(converted_samples(&data[..0], input, output).is_empty());
        }
        assert_eq!(converted_samples(&data[..1], 1, 1), [-16384]);
        assert_eq!(converted_samples(&data[..1], 1, 2), [-16384, -16384]);
        assert_eq!(converted_samples(&data[..2], 2, 1), [0]);
        assert_eq!(converted_samples(&data[..2], 2, 2), [-16384, 16384]);
        assert_eq!(converted_samples(data, 1, 1), [-16384, 16384, 0, 16384]);
        assert_eq!(converted_samples(data, 2, 2), [-16384, 16384, 0, 16384]);
        assert_eq!(
            converted_samples(data, 1, 2),
            [-16384, -16384, 16384, 16384, 0, 0, 16384, 16384],
        );
        assert_eq!(converted_samples(data, 2, 1), [0, 8192]);
    }

    check_channels(&[-0.5_f32, 0.5, 0.0, 0.5]);
    check_channels(&[-0.5_f64, 0.5, 0.0, 0.5]);
    check_channels(&[-16384_i16, 16384, 0, 16384]);
    check_channels(&[-1073741824_i32, 1073741824, 0, 1073741824]);
    check_channels(&[-4194304, 4194304, 0, 4194304].map(|sample| cpal::I24::new(sample).unwrap()));
}

#[test]
fn native_capture_handles_full_scale_f64() {
    assert_eq!(
        converted_samples(&[-1.0_f64, 0.0, 1.0], 1, 1),
        [-32768, 0, 32767]
    );
    assert_eq!(
        converted_samples(&[1.0_f64, 1.0, -1.0, -1.0], 2, 1),
        [32767, -32768]
    );
}

#[test]
fn native_capture_reuses_warmed_sender_buffer_for_equal_batches() {
    fn check_reuse<T: cpal::SizedSample>(mut data: Vec<T>) {
        let capture_data =
            unsafe { cpal::Data::from_parts(data.as_mut_ptr().cast(), data.len(), T::FORMAT) };
        for (input, output) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
            let mut buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
            {
                let mut samples = buffer.encode(&()).unwrap();
                convert_capture_data(&capture_data, input, output, &mut samples);
            }
            let (pointer, capacity) = {
                let mut samples = buffer.encode(&()).unwrap();
                convert_capture_data(&capture_data, input, output, &mut samples);
                (samples.as_ptr(), samples.capacity())
            };
            for _ in 0..3 {
                let mut samples = buffer.encode(&()).unwrap();
                assert!(samples.is_empty());
                convert_capture_data(&capture_data, input, output, &mut samples);
                assert_eq!(
                    samples.len(),
                    data.len() / input as usize * output as usize * 2
                );
                assert!(
                    samples
                        .chunks_exact(2)
                        .all(|sample| sample == 16384_i16.to_ne_bytes())
                );
                assert_eq!(samples.as_ptr(), pointer);
                assert_eq!(samples.capacity(), capacity);
            }
        }
    }

    check_reuse(vec![0.5_f32; 4096]);
    check_reuse(vec![0.5_f64; 4096]);
    check_reuse(vec![16384_i16; 4096]);
    check_reuse(vec![cpal::I24::new(4194304).unwrap(); 4096]);
    check_reuse(vec![1073741824_i32; 4096]);
}

#[test]
fn native_capture_reuses_sender_buffer_without_stale_samples() {
    let mut buffer = SenderBuffer::<()>::new(AUDIO, 0).unwrap();
    let large_batch = vec![0.5_f64; 4096];
    assert_eq!(
        encode_native_samples(&large_batch, 1, 2, &mut buffer),
        vec![16384; 8192],
    );
    assert_eq!(
        encode_native_samples(&[-0.5_f64], 1, 1, &mut buffer),
        [-16384]
    );
    assert_eq!(
        encode_native_samples(&[0.0_f64; 4], 2, 1, &mut buffer),
        [0, 0],
    );
    assert!(encode_native_samples(&[] as &[f64], 1, 1, &mut buffer).is_empty());
    assert_eq!(
        encode_native_samples(&large_batch, 1, 2, &mut buffer),
        vec![16384; 8192],
    );
}
