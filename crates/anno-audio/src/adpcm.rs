//! IMA ADPCM codec (stereo, interleaved nibbles).
//!
//! Ported from Maxsound.dll's adpcm_coder/adpcm_decoder.
//! Each byte encodes two samples (one per channel):
//!   - High nibble: left channel
//!   - Low nibble: right channel

/// Standard IMA ADPCM index adjustment table (16 entries).
const INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Standard IMA ADPCM step size table (89 entries).
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408,
    449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// Per-channel ADPCM state.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdpcmChannelState {
    pub predicted: i16,
    pub step_index: i8,
}

/// Stereo ADPCM state (left + right channels).
#[derive(Debug, Clone, Copy, Default)]
pub struct AdpcmState {
    pub left: AdpcmChannelState,
    pub right: AdpcmChannelState,
}

/// Decode a single ADPCM nibble, updating the channel state.
fn decode_nibble(nibble: u8, state: &mut AdpcmChannelState) -> i16 {
    let step = STEP_TABLE[state.step_index as usize];

    let mut diff = 0i32;
    if nibble & 4 != 0 {
        diff += step << 2;
    }
    if nibble & 2 != 0 {
        diff += step << 1;
    }
    if nibble & 1 != 0 {
        diff += step;
    }
    diff >>= 2;

    if nibble & 8 != 0 {
        diff = -diff;
    }

    let predicted = (state.predicted as i32 + diff).clamp(-32768, 32767);
    state.predicted = predicted as i16;

    state.step_index = (state.step_index as i32 + INDEX_TABLE[nibble as usize]).clamp(0, 88) as i8;

    predicted as i16
}

/// Encode a single sample to an ADPCM nibble, updating the channel state.
/// Uses the same reconstruction formula as the decoder to stay in sync.
fn encode_nibble(sample: i16, state: &mut AdpcmChannelState) -> u8 {
    let step = STEP_TABLE[state.step_index as usize];
    let mut diff = sample as i32 - state.predicted as i32;

    let sign = if diff < 0 {
        diff = -diff;
        8u8
    } else {
        0u8
    };

    let mut nibble = 0u8;

    if diff >= step {
        nibble |= 4;
        diff -= step;
    }
    let half_step = step >> 1;
    if diff >= half_step {
        nibble |= 2;
        diff -= half_step;
    }
    if diff >= step >> 2 {
        nibble |= 1;
    }

    // Reconstruct using the SAME formula as the decoder
    let mut reconstructed = 0i32;
    if nibble & 4 != 0 {
        reconstructed += step << 2;
    }
    if nibble & 2 != 0 {
        reconstructed += step << 1;
    }
    if nibble & 1 != 0 {
        reconstructed += step;
    }
    reconstructed >>= 2;

    if sign != 0 {
        reconstructed = -reconstructed;
    }

    let predicted = (state.predicted as i32 + reconstructed).clamp(-32768, 32767);
    state.predicted = predicted as i16;

    state.step_index =
        (state.step_index as i32 + INDEX_TABLE[(nibble | sign) as usize]).clamp(0, 88) as i8;

    nibble | sign
}

/// Decode stereo IMA ADPCM data to interleaved 16-bit PCM.
///
/// Each input byte produces 2 output samples (left, right).
/// Output buffer must have room for `input.len() * 2` samples.
pub fn decode_stereo(input: &[u8], output: &mut [i16], state: &mut AdpcmState) {
    let mut out_idx = 0;
    for &byte in input {
        let left_nibble = (byte >> 4) & 0x0F;
        let right_nibble = byte & 0x0F;

        if out_idx < output.len() {
            output[out_idx] = decode_nibble(left_nibble, &mut state.left);
            out_idx += 1;
        }
        if out_idx < output.len() {
            output[out_idx] = decode_nibble(right_nibble, &mut state.right);
            out_idx += 1;
        }
    }
}

/// Encode interleaved 16-bit PCM to stereo IMA ADPCM.
///
/// Input must have an even number of samples (left, right pairs).
/// Each pair produces one output byte.
pub fn encode_stereo(input: &[i16], output: &mut [u8], state: &mut AdpcmState) {
    let mut in_idx = 0;
    let mut out_idx = 0;

    while in_idx + 1 < input.len() && out_idx < output.len() {
        let left_nibble = encode_nibble(input[in_idx], &mut state.left);
        let right_nibble = encode_nibble(input[in_idx + 1], &mut state.right);
        output[out_idx] = (left_nibble << 4) | right_nibble;
        in_idx += 2;
        out_idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_stereo() {
        // Generate a slowly varying stereo signal that ADPCM can track
        let input: Vec<i16> = (0..256)
            .map(|i| ((i as f32 * 0.05).sin() * 1000.0) as i16)
            .collect();

        let mut encoded = vec![0u8; input.len() / 2];
        let mut encode_state = AdpcmState::default();
        encode_stereo(&input, &mut encoded, &mut encode_state);

        let mut decoded = vec![0i16; input.len()];
        let mut decode_state = AdpcmState::default();
        decode_stereo(&encoded, &mut decoded, &mut decode_state);

        // ADPCM is lossy, check that the second half (after step adapts) is close
        for (orig, dec) in input[64..].iter().zip(decoded[64..].iter()) {
            assert!(
                (*orig as i32 - *dec as i32).abs() < 500,
                "orig={orig}, decoded={dec}"
            );
        }
    }

    #[test]
    fn encode_decode_consistency() {
        // Verify encoder and decoder stay in sync (both produce same state)
        let mut enc_state = AdpcmChannelState::default();
        let mut dec_state = AdpcmChannelState::default();

        let samples = [0i16, 10, 20, 30, 25, 15, 0, -15, -30, -20, -10, 0];
        for &sample in &samples {
            let nibble = encode_nibble(sample, &mut enc_state);
            let _ = decode_nibble(nibble, &mut dec_state);
            assert_eq!(
                enc_state.predicted, dec_state.predicted,
                "encoder and decoder predicted values diverged"
            );
            assert_eq!(
                enc_state.step_index, dec_state.step_index,
                "encoder and decoder step indices diverged"
            );
        }
    }
}
