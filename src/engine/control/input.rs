//! An input: audio from the capture thread, held by the renderer.

use super::{StatusError, TryRecvStatusError, TryWaitError};
use crate::engine::protocol::inputs::{InputChannels, InputEnd, InputRequest, InputStatus};
use crate::request::{self, RequestHandle, RequestPermit};

#[derive(Debug)]
pub struct Input {
    handle: RequestHandle<InputRequest>,
}

impl Input {
    pub(crate) fn open(permit: RequestPermit<'_, InputRequest>, channels: InputChannels) -> Self {
        let handle = permit.open(InputRequest { channels });
        Self { handle }
    }

    pub fn stop(&mut self) {
        self.handle.cancel();
    }

    /// Asks the renderer, which answers at its next period.
    pub fn request_status(&mut self) -> Result<PendingInputStatus, StatusError> {
        let pending = self.handle.request_status()?;
        Ok(PendingInputStatus(pending))
    }

    /// Why it ended, once it has.
    pub fn try_wait(&mut self) -> Result<InputEnd, TryWaitError> {
        Ok(self.handle.try_wait()?)
    }
}

/// Stops an input still open, since nothing could stop it once the handle
/// is gone.
impl Drop for Input {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}

/// The renderer's answer to one status question about an input.
#[derive(Debug)]
pub struct PendingInputStatus(request::PendingStatus<InputRequest>);

impl PendingInputStatus {
    pub fn try_recv(&mut self) -> Result<InputStatus, TryRecvStatusError> {
        let status = self.0.try_recv()?;
        Ok(status)
    }
}
