//! An input: audio from the capture thread, held by the renderer.

use super::{StatusError, TryRecvStatusError, TryWaitError};
use crate::engine::protocol::inputs::{InputRequest, InputStatus};
use crate::request::{self, RequestHandle, RequestPermit};

#[derive(Debug)]
pub struct Input {
    handle: RequestHandle<InputRequest>,
}

impl Input {
    pub(crate) fn open(permit: RequestPermit<'_, InputRequest>) -> Self {
        let handle = permit.open(InputRequest {});
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

    /// `Ok` once it has ended.
    pub fn try_wait(&mut self) -> Result<(), TryWaitError> {
        self.handle.try_wait()?;
        Ok(())
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
