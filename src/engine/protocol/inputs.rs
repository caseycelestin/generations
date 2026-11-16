//! What crosses for inputs.

use crate::request::Request;

#[derive(Debug)]
pub(crate) struct InputRequest {}

impl Request for InputRequest {
    type Reply = ();
    type Status = InputStatus;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStatus {}
