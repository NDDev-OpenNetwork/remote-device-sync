use bytes::Bytes;
use rds_audio::{AudioFormat, AudioPacket, JitterBuffer, PopOutcome};

fn packet(seq: u64, data: &[u8]) -> AudioPacket {
    AudioPacket {
        seq,
        timestamp_us: 0,
        samples: 960,
        data: Bytes::copy_from_slice(data),
    }
}

fn silence(seq: u64) -> AudioPacket {
    // RFC 6716 permits zero-byte frames; this TOC declares one 20 ms CELT frame.
    packet(seq, &[0xf8])
}

#[test]
fn older_arrival_cannot_evict_a_newer_retained_packet() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 2, 0).unwrap();
    jitter.push(silence(2)).unwrap();
    jitter.push(silence(3)).unwrap();
    jitter.push(silence(1)).unwrap();
    for expected in [2, 3] {
        assert!(matches!(jitter.pop(), Some((PopOutcome::Packet, Some(p))) if p.seq == expected));
    }
}

#[test]
fn setting_the_playout_floor_discards_earlier_queued_packets() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
    jitter.push(silence(1)).unwrap();
    jitter.push(silence(5)).unwrap();
    jitter.start_at(5).unwrap();
    assert_eq!(jitter.len(), 1);
    assert!(matches!(jitter.pop(), Some((PopOutcome::Packet, Some(p))) if p.seq == 5));
    assert!(jitter.pop().is_none());
}

#[test]
fn consecutive_losses_each_have_a_concealment_interval() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
    jitter.start_at(1).unwrap();
    jitter.push(silence(3)).unwrap();
    for expected in [1, 2] {
        assert!(
            matches!(jitter.pop(), Some((PopOutcome::Gap { sequence }, None)) if sequence == expected)
        );
    }
    assert_eq!(jitter.gaps, 2);
    assert!(matches!(jitter.pop(), Some((PopOutcome::Packet, Some(p))) if p.seq == 3));
}

#[test]
fn exhausted_sequence_cannot_reopen_the_playout_epoch() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
    assert!(jitter.start_at(u64::MAX).is_err());
    assert!(jitter.push(silence(u64::MAX)).is_err());
    jitter.start_at(u64::MAX - 1).unwrap();
    jitter.push(silence(u64::MAX - 1)).unwrap();
    assert!(matches!(jitter.pop(), Some((PopOutcome::Packet, Some(p))) if p.seq == u64::MAX - 1));
    assert!(jitter.pop().is_none());
    assert_eq!(
        jitter.push(silence(0)).unwrap(),
        rds_audio::PushOutcome::Late
    );
}

#[test]
fn an_arbitrary_forward_jump_is_one_explicit_discontinuity() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
    jitter.start_at(0).unwrap();
    jitter.push(silence(u64::MAX - 1)).unwrap();
    assert_eq!(
        jitter.pop(),
        Some((
            PopOutcome::Discontinuity {
                from: 0,
                to: u64::MAX - 1
            },
            None
        ))
    );
    assert_eq!(jitter.gaps, u64::MAX - 1);
    assert!(matches!(jitter.pop(), Some((PopOutcome::Packet, Some(p))) if p.seq == u64::MAX - 1));
    assert!(jitter.pop().is_none());
}

#[test]
fn a_small_hole_waits_for_successors_or_the_missing_packet() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 1).unwrap();
    jitter.start_at(1).unwrap();
    jitter.push(silence(2)).unwrap();
    assert!(jitter.pop().is_none());
    jitter.push(silence(1)).unwrap();
    assert_eq!(jitter.pop().unwrap().1.unwrap().seq, 1);
    assert_eq!(jitter.pop().unwrap().1.unwrap().seq, 2);
    assert_eq!(jitter.gaps, 0);
}

#[test]
fn every_small_arrival_permutation_preserves_newest_bounded_audio() {
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    let mut arrivals = vec![a, b, c, d];
                    arrivals.sort_unstable();
                    arrivals.dedup();
                    if arrivals.len() != 4 {
                        continue;
                    }
                    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 2, 0).unwrap();
                    for seq in [a, b, c, d] {
                        jitter.push(silence(seq)).unwrap();
                        assert!(jitter.len() <= 2);
                    }
                    for expected in [2, 3] {
                        assert_eq!(jitter.pop().unwrap().1.unwrap().seq, expected);
                    }
                    assert_eq!(jitter.overflow, 2);
                    assert!(jitter.is_empty());
                }
            }
        }
    }
}

#[test]
fn diagnostics_saturate_instead_of_panicking_or_wrapping() {
    let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 1, 0).unwrap();
    jitter.accepted = u64::MAX;
    jitter.duplicates = u64::MAX;
    jitter.overflow = u64::MAX;
    jitter.late = u64::MAX;
    jitter.gaps = u64::MAX;
    jitter.start_at(0).unwrap();
    jitter.push(silence(0)).unwrap();
    jitter.push(silence(0)).unwrap();
    jitter.push(silence(1)).unwrap();
    assert!(matches!(
        jitter.pop(),
        Some((PopOutcome::Gap { sequence: 0 }, None))
    ));
    jitter.pop().unwrap();
    jitter.push(silence(0)).unwrap();
    assert_eq!(
        [
            jitter.accepted,
            jitter.duplicates,
            jitter.overflow,
            jitter.late,
            jitter.gaps
        ],
        [u64::MAX; 5]
    );
}
