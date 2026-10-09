use bytes::Bytes;
use rds_audio::{AudioFormat, AudioPacket, JitterBuffer, OpusDecoder, OpusEncoder};

fn packet(seq: u64, data: &[u8]) -> AudioPacket {
    AudioPacket {
        seq,
        timestamp_us: 0,
        samples: 960,
        data: Bytes::copy_from_slice(data),
    }
}

#[test]
fn admission_parses_the_packet_instead_of_trusting_its_sample_count() {
    for data in [
        &[0xf9, 0][..], // Two CBR frames cannot split one payload byte.
        &[0xf0][..],    // One 10 ms frame falsely labeled as 20 ms.
        &[0x01][..],    // Two valid 10 ms frames exceed the one-frame profile.
    ] {
        let p = packet(0, data);
        assert!(
            AudioPacket::from_wire(p.to_wire(), AudioFormat::MONO_48_KHZ, 960).is_err(),
            "wire admission accepted {data:?}"
        );
        let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
        assert!(jitter.push(p).is_err(), "jitter accepted {data:?}");
        assert!(jitter.is_empty());
    }
}

#[test]
fn maximum_single_frame_includes_its_toc_byte() {
    let mut data = vec![0; 1276];
    data[0] = 0xf8;
    assert_eq!(opus::packet::parse(&data).unwrap().frames[0].len(), 1275);
    let p = packet(0, &data);
    assert!(AudioPacket::from_wire(p.to_wire(), AudioFormat::MONO_48_KHZ, 960).is_ok());
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
    assert!(jitter.push(p).is_ok());
}

#[test]
fn incorrect_duration_is_rejected_before_stateful_decode() {
    let format = AudioFormat::MONO_48_KHZ;
    let mut encoder =
        opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Audio).unwrap();
    let pcm: Vec<f32> = (0..480).map(|i| (i as f32 * 0.04).sin() * 0.5).collect();
    let mut encoded = [0; 1276];
    let n = encoder.encode_float(&pcm, &mut encoded).unwrap();
    let mut tested = OpusDecoder::new(format).unwrap();
    let mut untouched = OpusDecoder::new(format).unwrap();
    assert!(tested.decode(&packet(0, &encoded[..n])).is_err());
    assert!(
        tested.decode_plc().unwrap() == untouched.decode_plc().unwrap(),
        "refused packet changed decoder state"
    );
}

#[test]
fn real_packets_roundtrip_at_every_supported_pcm_format() {
    for sample_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        for channels in [1, 2] {
            let format = AudioFormat {
                sample_rate,
                channels,
            };
            let mut encoder = OpusEncoder::new(format).unwrap();
            let mut decoder = OpusDecoder::new(format).unwrap();
            let pcm = vec![0.0; encoder.frame_samples() * channels as usize];
            for seq in 0..8 {
                let encoded = encoder.encode(&pcm, seq * 20_000).unwrap();
                let received =
                    AudioPacket::from_wire(encoded.to_wire(), format, encoder.frame_samples())
                        .unwrap();
                assert_eq!(received.seq, seq);
                assert_eq!(decoder.decode(&received).unwrap().len(), pcm.len());
            }
        }
    }
}

#[test]
fn output_channels_do_not_reject_valid_mono_coded_stereo_audio() {
    let mut encoder = OpusEncoder::new(AudioFormat::MONO_48_KHZ).unwrap();
    let encoded = encoder.encode(&vec![0.0; 960], 0).unwrap();
    let stereo = AudioFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let received = AudioPacket::from_wire(encoded.to_wire(), stereo, 960).unwrap();
    let mut decoder = OpusDecoder::new(stereo).unwrap();
    assert_eq!(decoder.decode(&received).unwrap().len(), 1920);
}

#[test]
fn malformed_admission_does_not_evict_buffered_audio() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 1, 0).unwrap();
    jitter.push(packet(4, &[0xf8])).unwrap();
    for data in [vec![], vec![0xf8; 1277], vec![0xfb], vec![0xfb, 0]] {
        assert!(jitter.push(packet(5, &data)).is_err());
        assert_eq!(jitter.len(), 1);
        assert_eq!(jitter.accepted, 1);
        assert_eq!(jitter.overflow, 0);
    }
    assert_eq!(jitter.pop().unwrap().1.unwrap().seq, 4);
}

#[test]
fn wire_admission_refuses_invalid_negotiation_and_timestamp_overflow() {
    for samples in [0, 1, 959, 961, usize::MAX] {
        let mut frame = packet(0, &[0xf8]).to_wire();
        frame.samples = samples as u32;
        assert!(AudioPacket::from_wire(frame, AudioFormat::MONO_48_KHZ, samples).is_err());
    }
    let mut frame = packet(0, &[0xf8]).to_wire();
    frame.capture_ts_ms = u64::MAX;
    assert!(AudioPacket::from_wire(frame, AudioFormat::MONO_48_KHZ, 960).is_err());
}
