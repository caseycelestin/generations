//! The control side: runs on UI or event threads. May lock and allocate.

mod rings;

use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer};

use crate::engine::protocol::{Command, Return};
use rings::Rings;

/// The control side's view of one renderer. Clones share it.
#[derive(Debug, Clone)]
pub struct Control(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    rings: Mutex<Rings>,
}

impl Control {
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self(Arc::new(Shared {
            rings: Mutex::new(Rings::new(to_renderer, from_renderer)),
        }))
    }
}
