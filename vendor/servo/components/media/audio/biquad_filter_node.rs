/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::f64::consts::{PI, SQRT_2};

use malloc_size_of_derive::MallocSizeOf;
use smallvec::SmallVec;

use crate::audio_node::{AudioNodeEngine, AudioNodeMessage, AudioNodeType, BlockInfo, ChannelInfo};
use crate::block::{Chunk, Tick};
use crate::param::{Param, ParamType};

#[derive(Copy, Clone, Debug, MallocSizeOf)]
pub struct BiquadFilterNodeOptions {
    pub filter: FilterType,
    pub frequency: f32,
    pub detune: f32,
    pub q: f32,
    pub gain: f32,
}

#[derive(Copy, Clone, Debug, MallocSizeOf)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    LowShelf,
    HighShelf,
    Peaking,
    Notch,
    AllPass,
}

impl Default for BiquadFilterNodeOptions {
    fn default() -> Self {
        BiquadFilterNodeOptions {
            filter: FilterType::LowPass,
            frequency: 350.,
            detune: 0.,
            q: 1.,
            gain: 0.,
        }
    }
}

#[derive(Copy, Clone, Debug, MallocSizeOf)]
pub enum BiquadFilterNodeMessage {
    SetFilterType(FilterType),
}

/// The last two input and output values, per-channel
// Default sets all fields to zero
#[derive(Default, Copy, Clone, PartialEq)]
struct BiquadState {
    /// The input value from last frame
    x1: f64,
    /// The input value from two frames ago
    x2: f64,
    /// The output value from last frame
    y1: f64,
    /// The output value from two frames ago
    y2: f64,
}

impl BiquadState {
    /// Update with new input/output values from this frame
    fn update(&mut self, x: f64, y: f64) {
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
    }
}

/// <https://webaudio.github.io/web-audio-api/#biquadfilternode>
#[derive(AudioNodeCommon)]
pub(crate) struct BiquadFilterNode {
    channel_info: ChannelInfo,
    filter: FilterType,
    frequency: Param,
    detune: Param,
    q: Param,
    gain: Param,
    /// The computed filter parameter b0
    /// This is actually b0 / a0, we pre-divide
    /// for efficiency
    b0: f64,
    /// The computed filter parameter b1
    /// This is actually b1 / a0, we pre-divide
    /// for efficiency
    b1: f64,
    /// The computed filter parameter b2
    /// This is actually b2 / a0, we pre-divide
    /// for efficiency
    b2: f64,
    /// The computed filter parameter a1
    /// This is actually a1 / a0, we pre-divide
    /// for efficiency
    a1: f64,
    /// The computed filter parameter a2
    /// This is actually a2 / a0, we pre-divide
    /// for efficiency
    a2: f64,
    /// Stored filter state, this contains the last two
    /// frames of input and output values for every
    /// channel
    state: SmallVec<[BiquadState; 2]>,
}

impl BiquadFilterNode {
    pub fn new(
        options: BiquadFilterNodeOptions,
        channel_info: ChannelInfo,
        sample_rate: f32,
    ) -> Self {
        let mut ret = Self {
            channel_info,
            filter: options.filter,
            frequency: Param::new(options.frequency),
            gain: Param::new(options.gain),
            q: Param::new(options.q),
            detune: Param::new(options.detune),
            b0: 0.,
            b1: 0.,
            b2: 0.,
            a1: 0.,
            a2: 0.,
            state: SmallVec::new(),
        };
        ret.update_coefficients(sample_rate);
        ret
    }

    pub fn update_parameters(&mut self, info: &BlockInfo, tick: Tick) -> bool {
        let mut changed = self.frequency.update(info, tick);
        changed |= self.detune.update(info, tick);
        changed |= self.q.update(info, tick);
        changed |= self.gain.update(info, tick);

        if changed {
            self.update_coefficients(info.sample_rate);
        }
        changed
    }

    /// Set to the constant z-transform y[n] = b0 * x[n]
    fn constant_z_transform(&mut self, b0: f64) {
        self.b0 = b0;
        self.b1 = 0.;
        self.b2 = 0.;
        self.a1 = 0.;
        self.a2 = 0.;
    }

    /// Update the coefficients a1, a2, b0, b1, b2, given the sample_rate
    ///
    /// See <https://webaudio.github.io/web-audio-api/#filters-characteristics>
    fn update_coefficients(&mut self, fs: f32) {
        let g: f64 = self.gain.value().into();
        let q: f64 = self.q.value().into();
        let freq: f64 = self.frequency.value().into();
        let f0: f64 = freq * (2.0_f64).powf(self.detune.value() as f64 / 1200.);
        let fs: f64 = fs.into();
        let normalized = f0 / fs;

        // (BAO, e168/REQ-BRW-002) Out-of-band frequency takes the per-type
        // boundary z-transform limit, mirroring Chromium `Biquad::Set*Params`
        // (cutoff clamped into [0, 1] — bandpass clamps only below — so the
        // boundary branches emit b0 = <limit>, a0 = 1, a1 = a2 = 0, killing
        // the filter state). The upstream form clamped f0 to fs/2 and
        // computed at omega0 = pi, which keeps the live (1 + z^-1)^2
        // denominator pole ringing forever once the center frequency leaves
        // the band (biquad-automation "automate-detune": bandpass swept past
        // Nyquist rings to amplitude 14.5 where the reference expects
        // silence).
        let at_nyquist_or_above = !normalized.is_finite() || normalized >= 0.5;
        let at_zero_or_below = normalized <= 0.;

        let a = 10.0_f64.powf(g / 40.);

        // the boundary values sometimes need limits to
        // be taken
        match self.filter {
            FilterType::LowPass => {
                if at_nyquist_or_above {
                    self.constant_z_transform(1.);
                    return;
                } else if at_zero_or_below {
                    self.constant_z_transform(0.);
                    return;
                }
            },
            FilterType::HighPass => {
                if at_nyquist_or_above {
                    self.constant_z_transform(0.);
                    return;
                } else if at_zero_or_below {
                    self.constant_z_transform(1.);
                    return;
                }
            },
            FilterType::LowShelf => {
                if at_nyquist_or_above {
                    self.constant_z_transform(a * a);
                    return;
                } else if at_zero_or_below {
                    self.constant_z_transform(1.);
                    return;
                }
            },
            FilterType::HighShelf => {
                if at_nyquist_or_above {
                    self.constant_z_transform(1.);
                    return;
                } else if at_zero_or_below {
                    self.constant_z_transform(a * a);
                    return;
                }
            },
            FilterType::Peaking => {
                if at_zero_or_below || at_nyquist_or_above {
                    self.constant_z_transform(1.);
                    return;
                } else if q <= 0. {
                    self.constant_z_transform(a * a);
                    return;
                }
            },
            FilterType::AllPass => {
                if at_zero_or_below || at_nyquist_or_above {
                    self.constant_z_transform(1.);
                    return;
                } else if q <= 0. {
                    self.constant_z_transform(-1.);
                    return;
                }
            },
            FilterType::Notch => {
                if at_zero_or_below || at_nyquist_or_above {
                    self.constant_z_transform(1.);
                    return;
                } else if q <= 0. {
                    self.constant_z_transform(0.);
                    return;
                }
            },
            FilterType::BandPass => {
                if at_zero_or_below || at_nyquist_or_above {
                    self.constant_z_transform(0.);
                    return;
                } else if q <= 0. {
                    self.constant_z_transform(1.);
                    return;
                }
            },
        }

        let omega0 = 2. * PI * normalized;
        let sin_omega = omega0.sin();
        let cos_omega = omega0.cos();
        let alpha_q = sin_omega / (2. * q);
        let alpha_q_db = sin_omega / (2. * 10.0_f64.powf(q / 20.));
        let alpha_s = sin_omega / SQRT_2;

        // we predivide by a0
        let a0;

        match self.filter {
            FilterType::LowPass => {
                self.b0 = (1. - cos_omega) / 2.;
                self.b1 = 1. - cos_omega;
                self.b2 = self.b1 / 2.;
                a0 = 1. + alpha_q_db;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q_db;
            },
            FilterType::HighPass => {
                self.b0 = (1. + cos_omega) / 2.;
                self.b1 = -(1. + cos_omega);
                self.b2 = -self.b1 / 2.;
                a0 = 1. + alpha_q_db;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q_db;
            },
            FilterType::BandPass => {
                self.b0 = alpha_q;
                self.b1 = 0.;
                self.b2 = -alpha_q;
                a0 = 1. + alpha_q;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q;
            },
            FilterType::Notch => {
                self.b0 = 1.;
                self.b1 = -2. * cos_omega;
                self.b2 = 1.;
                a0 = 1. + alpha_q;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q;
            },
            FilterType::AllPass => {
                self.b0 = 1. - alpha_q;
                self.b1 = -2. * cos_omega;
                self.b2 = 1. + alpha_q;
                a0 = 1. + alpha_q;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q;
            },
            FilterType::Peaking => {
                self.b0 = 1. + alpha_q * a;
                self.b1 = -2. * cos_omega;
                self.b2 = 1. - alpha_q * a;
                a0 = 1. + alpha_q / a;
                self.a1 = -2. * cos_omega;
                self.a2 = 1. - alpha_q / a;
            },
            FilterType::LowShelf => {
                let alpha_rt_a = 2. * alpha_s * a.sqrt();
                self.b0 = a * ((a + 1.) - (a - 1.) * cos_omega + alpha_rt_a);
                self.b1 = 2. * a * ((a - 1.) - (a + 1.) * cos_omega);
                self.b2 = a * ((a + 1.) - (a - 1.) * cos_omega - alpha_rt_a);
                a0 = (a + 1.) + (a - 1.) * cos_omega + alpha_rt_a;
                self.a1 = -2. * ((a - 1.) + (a + 1.) * cos_omega);
                self.a2 = (a + 1.) + (a - 1.) * cos_omega - alpha_rt_a;
            },
            FilterType::HighShelf => {
                let alpha_rt_a = 2. * alpha_s * a.sqrt();
                self.b0 = a * ((a + 1.) + (a - 1.) * cos_omega + alpha_rt_a);
                self.b1 = -2. * a * ((a - 1.) + (a + 1.) * cos_omega);
                self.b2 = a * ((a + 1.) + (a - 1.) * cos_omega - alpha_rt_a);
                a0 = (a + 1.) - (a - 1.) * cos_omega + alpha_rt_a;
                self.a1 = 2. * ((a - 1.) - (a + 1.) * cos_omega);
                self.a2 = (a + 1.) - (a - 1.) * cos_omega - alpha_rt_a;
            },
        }
        self.b0 /= a0;
        self.b1 /= a0;
        self.b2 /= a0;
        self.a1 /= a0;
        self.a2 /= a0;
    }
}

impl AudioNodeEngine for BiquadFilterNode {
    fn node_type(&self) -> AudioNodeType {
        AudioNodeType::BiquadFilterNode
    }

    fn process(&mut self, mut inputs: Chunk, info: &BlockInfo) -> Chunk {
        debug_assert!(inputs.len() == 1);
        self.state
            .resize(inputs.blocks[0].chan_count() as usize, Default::default());
        self.update_parameters(info, Tick(0));

        // XXXManishearth this node has tail time, so even if the block is silence
        // we must still compute things on it. However, it is possible to become
        // a dumb passthrough as long as we reach a quiescent state
        //
        // see https://dxr.mozilla.org/mozilla-central/rev/87a95e1b7ec691bef7b938e722fe1b01cce68664/dom/media/webaudio/blink/Biquad.cpp#81-91

        let repeat_or_silence = inputs.blocks[0].is_silence() || inputs.blocks[0].is_repeat();

        if repeat_or_silence && !self.state.iter().all(|s| *s == self.state[0]) {
            // In case our input is repeat/silence but our states are not identical, we must
            // explicitly duplicate, since mutate_with will otherwise only operate
            // on the first channel, ignoring the states of the later ones
            inputs.blocks[0].explicit_repeat();
        } else {
            // In case the states are identical, just make any silence explicit,
            // since mutate_with can't handle silent blocks
            inputs.blocks[0].explicit_silence();
        }

        {
            let mut iter = inputs.blocks[0].iter();
            while let Some(mut frame) = iter.next() {
                self.update_parameters(info, frame.tick());
                frame.mutate_with(|sample, chan| {
                    let state = &mut self.state[chan as usize];
                    let x0 = *sample as f64;
                    let y0 = self.b0 * x0 + self.b1 * state.x1 + self.b2 * state.x2 -
                        self.a1 * state.y1 -
                        self.a2 * state.y2;
                    *sample = y0 as f32;
                    state.update(x0, y0);
                });
            }
        }

        if inputs.blocks[0].is_repeat() {
            let state = self.state[0];
            self.state.iter_mut().for_each(|s| *s = state);
        }

        inputs
    }

    fn get_param(&mut self, id: ParamType) -> &mut Param {
        match id {
            ParamType::Frequency => &mut self.frequency,
            ParamType::Detune => &mut self.detune,
            ParamType::Q => &mut self.q,
            ParamType::Gain => &mut self.gain,
            _ => panic!("Unknown param {:?} for BiquadFilterNode", id),
        }
    }

    fn message_specific(&mut self, message: AudioNodeMessage, sample_rate: f32) {
        if let AudioNodeMessage::BiquadFilterNode(m) = message {
            match m {
                BiquadFilterNodeMessage::SetFilterType(f) => {
                    self.filter = f;
                    self.update_coefficients(sample_rate);
                },
            }
        }
    }
}

#[cfg(test)]
mod boundary_tests {
    // (BAO, e168/REQ-BRW-002) WPT biquad-automation "automate-detune" shape
    // pin: a bandpass whose frequency*2^(detune/1200) sweeps past Nyquist
    // must output exact zeros from the crossing on — the per-type boundary
    // z-transform limit (b0 = 0, a1 = a2 = 0) kills the filter state.
    // The old form clamped f0 to fs/2 and computed at omega0 = pi, leaving
    // the live (1 + z^-1)^2 denominator pole ringing forever after the
    // crossing (WPT failure form: amplitude 14.5 at the end of the render).
    use super::*;
    use crate::audio_node::{AudioNodeEngine, BlockInfo, ChannelInfo};
    use crate::block::{Block, Chunk, Tick, FRAMES_PER_BLOCK_USIZE};
    use crate::param::{ParamType, RampKind, UserAutomationEvent};

    const FS: f32 = 16000.;

    fn block_info(block: u64) -> BlockInfo {
        BlockInfo {
            sample_rate: FS,
            frame: Tick(block * FRAMES_PER_BLOCK_USIZE as u64),
            time: (block * FRAMES_PER_BLOCK_USIZE as u64) as f64 / FS as f64,
        }
    }

    #[test]
    fn bandpass_is_silent_once_frequency_passes_nyquist() {
        // 4400 Hz center swept by a -12000..12000 cent detune ramp over
        // 0.125s (2000 frames): f0 = 4400*2^(detune/1200) crosses 8000 Hz
        // between frames 1086 (7987.9 Hz) and 1087 (8043.4 Hz).
        let mut node = BiquadFilterNode::new(
            BiquadFilterNodeOptions {
                filter: FilterType::BandPass,
                frequency: 4400.,
                detune: 0.,
                q: 1.,
                gain: 0.,
            },
            ChannelInfo::default(),
            FS,
        );
        let detune = node.get_param(ParamType::Detune);
        detune.insert_event(
            UserAutomationEvent::SetValueAtTime(-12000., 0.).convert_to_event(FS),
        );
        detune.insert_event(
            UserAutomationEvent::RampToValueAtTime(RampKind::Linear, 12000., 0.125)
                .convert_to_event(FS),
        );

        // 4400 Hz test tone, same shape as the WPT task.
        let omega = 2. * PI * 4400. / FS as f64;

        let mut charged = false;
        let mut first_ring = None;
        for block in 0..10u64 {
            let info = block_info(block);
            let tone: Vec<f32> = (0..FRAMES_PER_BLOCK_USIZE)
                .map(|i| {
                    let n = block as usize * FRAMES_PER_BLOCK_USIZE + i;
                    (omega * n as f64).sin() as f32
                })
                .collect();
            let out = node.process(Chunk::new(Block::for_vec(tone)), &info);
            for frame in 0..FRAMES_PER_BLOCK_USIZE {
                let n = block as usize * FRAMES_PER_BLOCK_USIZE + frame;
                let sample = out.blocks[0].data_chan_frame(frame, 0);
                if n < 1087 {
                    charged |= sample != 0.;
                } else if sample != 0. && first_ring.is_none() {
                    first_ring = Some((n, sample));
                }
            }
        }
        assert!(charged, "filter must pass signal while in band (test setup)");
        assert!(
            first_ring.is_none(),
            "output must be exactly zero once f0 passes Nyquist, rang at {first_ring:?}"
        );
    }
}
