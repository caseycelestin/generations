//! An input: audio from the capture thread, held by the renderer.

use crate::engine::protocol::inputs::InputRequest;
use crate::request::{RequestHandle, RequestPermit};

#[derive(Debug)]
pub struct Input {
    handle: RequestHandle<InputRequest>,
}

impl Input {
    pub(crate) fn open(permit: RequestPermit<'_, InputRequest>) -> Self {
        let handle = permit.open(InputRequest {});
        Self { handle }
    }
}
