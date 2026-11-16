//! An input on the render side, while it is open.

use rtrb::Consumer;

use crate::config::Pcm;
use crate::engine::protocol::inputs::InputRequest;
use crate::request::{RealtimeEnd, ReplyPermit, RequestId};

#[derive(Debug)]
pub(crate) struct Input {
    permit: ReplyPermit,
    reader: Consumer<Pcm>,
}

impl Input {
    pub(crate) fn open(permit: ReplyPermit, reader: Consumer<Pcm>) -> Self {
        Self { permit, reader }
    }

    pub(crate) fn is(&self, id: RequestId) -> bool {
        self.permit.id() == id
    }

    /// The one way an input leaves: its reader goes back in the reply.
    pub(crate) fn end(self, requests: &mut RealtimeEnd<InputRequest>) {
        requests.close(self.permit, self.reader);
    }
}
