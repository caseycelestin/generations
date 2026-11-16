//! An input on the render side, while it is open.

use rtrb::Consumer;

use super::{Frame, Renderer};
use crate::config::Pcm;
use crate::engine::protocol::inputs::InputRequest;
use crate::layout::ChannelLayout;
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Input {
    permit: ReplyPermit,
    reader: Consumer<Pcm>,
    layout: ChannelLayout,
}

impl Input {
    pub(crate) fn open(permit: ReplyPermit, reader: Consumer<Pcm>, layout: ChannelLayout) -> Self {
        Self {
            permit,
            reader,
            layout,
        }
    }

    /// As many frames as have arrived, up to `into`'s length; silence after
    /// them, which is a gap when the capture side runs late. Mono goes to
    /// both sides at full level.
    pub(crate) fn read(&mut self, into: &mut [Frame]) {
        let width = self.layout.count();
        let frames = into.len().min(self.reader.slots() / width);
        let Ok(chunk) = self.reader.read_chunk(frames * width) else {
            into.fill([0.0; Renderer::CHANNELS]);
            return;
        };
        let (sound, silence) = into.split_at_mut(frames);
        silence.fill([0.0; Renderer::CHANNELS]);
        // The ring holds whole frames, so its two slices split between
        // frames, never inside one.
        let (first, second) = chunk.as_slices();
        match self.layout {
            ChannelLayout::Mono => {
                for (frame, &sample) in sound.iter_mut().zip(first.iter().chain(second)) {
                    *frame = [sample, sample];
                }
            }
            ChannelLayout::Stereo => {
                let (first, _) = first.as_chunks::<2>();
                let (second, _) = second.as_chunks::<2>();
                for (frame, pair) in sound.iter_mut().zip(first.iter().chain(second)) {
                    *frame = *pair;
                }
            }
        }
        chunk.commit_all();
    }

    pub(crate) fn feeder_gone(&self) -> bool {
        self.reader.is_abandoned()
    }

    pub(crate) fn is(&self, id: RequestId) -> bool {
        self.permit.id() == id
    }

    /// The one way an input leaves: its reader goes back in the reply.
    pub(crate) fn end(self, requests: &mut RealtimeEnd<InputRequest>) {
        requests.close(self.permit, self.reader);
    }
}
