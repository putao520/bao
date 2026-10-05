/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::VecDeque;
use std::sync::Arc;

use f32;
use log::error;
use malloc_size_of_derive::MallocSizeOf;
use parking_lot::RwLock;

use crate::audio_node::{
    AudioNodeEngine, AudioNodeType, BlockInfo, ChannelInfo, ChannelInterpretation,
};
use crate::block::{Block, Chunk};
use crate::param::{Param, ParamType};

mod delay_reader;
mod delay_writer;
pub(crate) use delay_reader::DelayReader;
pub(crate) use delay_writer::DelayWriter;

// Share with internal nodes. Use Arc because AudioNodeEngine requires Send
type DelayBuffer = Arc<RwLock<VecDeque<Block>>>;
type CachedUpmixedBlock = Arc<RwLock<Option<UpmixedBlock>>>;

#[derive(Copy, Clone, Debug, MallocSizeOf)]
pub struct DelayNodeOptions {
    pub max_delay_time: f64,
    pub delay_time: f64,
}

impl Default for DelayNodeOptions {
    fn default() -> Self {
        DelayNodeOptions {
            max_delay_time: 1.,
            delay_time: 0.,
        }
    }
}

#[derive(AudioNodeCommon)]
pub(crate) struct DelayNode {
    channel_info: ChannelInfo,
    delay_writer: Option<Box<DelayWriter>>,
    delay_reader: Option<Box<DelayReader>>,
}

/// UpmixedBlock is a Block that has been upmixed to the output channel count of the DelayReader
#[derive(Debug)]
struct UpmixedBlock {
    // The index of the upmixed block in the delay line
    index: usize,
    block: Block,
}

impl UpmixedBlock {
    fn new(
        index: usize,
        channel_count: u8,
        channel_interpretation: ChannelInterpretation,
        block: &Block,
    ) -> Self {
        let mut block = block.clone();
        block.mix(channel_count, channel_interpretation);
        UpmixedBlock { index, block }
    }

    fn index(&self) -> usize {
        self.index
    }

    fn block(&self) -> &Block {
        &self.block
    }

    fn increment_index(&mut self) {
        self.index += 1;
    }
}

impl DelayNode {
    pub fn new(options: DelayNodeOptions, channel_info: ChannelInfo) -> Self {
        let delay_line = Arc::new(RwLock::new(VecDeque::with_capacity(0)));
        let upmixed_block = Arc::new(RwLock::new(None));
        DelayNode {
            channel_info,
            delay_writer: Some(Box::new(DelayWriter::new(
                delay_line.clone(),
                upmixed_block.clone(),
                channel_info,
                options.max_delay_time,
            ))),
            delay_reader: Some(Box::new(DelayReader::new(
                delay_line,
                upmixed_block,
                Param::new(options.delay_time as f32),
                channel_info,
            ))),
        }
    }
}

impl AudioNodeEngine for DelayNode {
    fn node_type(&self) -> AudioNodeType {
        AudioNodeType::DelayNode
    }

    fn process(&mut self, inputs: Chunk, info: &BlockInfo) -> Chunk {
        let Some(delay_writer) = &mut self.delay_writer else {
            error!("No DelayWriter initialized!");
            return Chunk::explicit_silence();
        };
        delay_writer.process(inputs, info);

        // Read from the internal buffer
        let Some(delay_reader) = &mut self.delay_reader else {
            error!("No DelayReader initialized!");
            return Chunk::explicit_silence();
        };
        delay_reader.process(Chunk::default(), info)
    }

    fn get_param(&mut self, id: ParamType) -> &mut Param {
        // DelayReader should not be None when `get_param` is called.
        // The only time DelayNode gives up ownership of its DelayReader is within a render quantum
        // processing loop, when it gives ownership to the graph. In this case it has removed itself
        // from the graph so it will not be processed, and therefore never call `get_param` within the
        // processing loop.
        self.delay_reader
            .as_mut()
            .expect("Tried to get delay_time Param without an owned DelayReader.")
            .get_param(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_node::{AudioNodeEngine, BlockInfo, ChannelInfo};
    use crate::block::{Chunk, FRAMES_PER_BLOCK_USIZE, Tick};

    fn block_info() -> BlockInfo {
        BlockInfo {
            sample_rate: 44100.,
            frame: Tick(0),
            time: 0.,
        }
    }

    /// A single-channel block whose every frame holds `value`.
    fn dc_block(value: f32) -> Block {
        let mut block = Block::for_channels_explicit(1);
        block.data_chan_mut(0).fill(value);
        block
    }

    fn dc_chunk(value: f32) -> Chunk {
        Chunk::new(dc_block(value))
    }

    /// Value-level silence: `Block::is_silence` only reports the structural
    /// empty-buffer state, which silent chunks produced by the delay path do
    /// not have.
    fn is_all_zero(block: &Block) -> bool {
        (0..block.chan_count())
            .all(|chan| (0..FRAMES_PER_BLOCK_USIZE).all(|frame| block.data_chan_frame(frame, chan) == 0.))
    }

    #[test]
    fn default_options_are_spec_initial() {
        let options = DelayNodeOptions::default();
        assert_eq!(options.max_delay_time, 1.);
        assert_eq!(options.delay_time, 0.);
    }

    #[test]
    fn delay_time_param_is_reachable_from_the_node() {
        let mut node = DelayNode::new(
            DelayNodeOptions {
                max_delay_time: 1.,
                delay_time: 0.25,
            },
            ChannelInfo::default(),
        );
        assert_eq!(node.node_type(), AudioNodeType::DelayNode);
        assert_eq!(node.get_param(ParamType::DelayTime).value(), 0.25);
    }

    #[test]
    fn silent_input_yields_silent_output() {
        let mut node = DelayNode::new(DelayNodeOptions::default(), ChannelInfo::default());
        let output = node.process(Chunk::explicit_silence(), &block_info());
        assert_eq!(output.len(), 1);
        assert!(is_all_zero(&output.blocks[0]));
    }

    #[test]
    fn reader_yields_silence_while_the_delay_line_is_empty() {
        let buffer: DelayBuffer = Arc::new(RwLock::new(VecDeque::with_capacity(0)));
        let upmixed_block: CachedUpmixedBlock = Arc::new(RwLock::new(None));
        let mut reader = DelayReader::new(
            buffer,
            upmixed_block,
            Param::new(0.),
            ChannelInfo::default(),
        );
        let output = reader.read();
        assert_eq!(output.len(), 1);
        assert!(is_all_zero(&output.blocks[0]));
    }

    #[test]
    fn writer_keeps_the_delay_line_within_capacity_and_fifo_ordered() {
        // max_delay_time of 1s at 44.1kHz is exactly 344.53125 frames per
        // block, so the delay line holds ceil(344.53125) + 1 = 346 blocks.
        let buffer: DelayBuffer = Arc::new(RwLock::new(VecDeque::with_capacity(0)));
        let upmixed_block: CachedUpmixedBlock = Arc::new(RwLock::new(None));
        let mut writer =
            DelayWriter::new(buffer.clone(), upmixed_block, ChannelInfo::default(), 1.);
        let info = block_info();
        for i in 0..400u32 {
            let mut input = Chunk::default();
            input.blocks.push(dc_block(i as f32));
            writer.process(input, &info);
        }
        let delay_line = buffer.read();
        assert_eq!(delay_line.len(), 346);
        // Front holds the most recently written block, back the oldest kept.
        assert_eq!(delay_line.front().expect("front").data_chan_frame(0, 0), 399.);
        assert_eq!(delay_line.back().expect("back").data_chan_frame(0, 0), 54.);
    }

    #[test]
    fn delayed_input_only_reaches_the_output_once_the_delay_elapses() {
        // delay_time of 0.01s at 44.1kHz is exactly 441 frames, i.e. a bit
        // over three render blocks; output must stay silent until the first
        // delayed frames come back around.
        let mut node = DelayNode::new(
            DelayNodeOptions {
                max_delay_time: 1.,
                delay_time: 0.01,
            },
            ChannelInfo::default(),
        );
        let info = block_info();
        for _ in 0..3 {
            let output = node.process(dc_chunk(1.), &info);
            assert!(
                is_all_zero(&output.blocks[0]),
                "no delayed frame may surface before 441 frames have passed"
            );
        }
        let output = node.process(dc_chunk(1.), &info);
        // Frame 0 still reads from a block that is not in the line yet.
        assert_eq!(output.blocks[0].data_chan_frame(0, 0), 0.);
        // Frame 127 reads exactly 441 frames back, which is a full-amplitude
        // sample of the input written three blocks ago.
        assert!((output.blocks[0].data_chan_frame(127, 0) - 1.).abs() < 1e-5);
    }
}
