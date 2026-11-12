//! The control side: code that runs on the threads driving the engine,
//! a UI or event loop. May lock and allocate; never runs on a real-time
//! thread.

mod rings;

use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer};

use crate::engine::protocol::{Command, Return};
use rings::Rings;

/// One renderer as the control side sees it. Cheap to clone: every
/// clone is the same renderer.
#[derive(Debug, Clone)]
pub struct Control(Arc<Shared>);

/// What every clone of a [`Control`] shares.
#[derive(Debug)]
struct Shared {
    /// The rings to and from the renderer, under one lock so every send
    /// collects first.
    rings: Mutex<Rings>,
}

impl Control {
    /// A control sending on `to_renderer` and collecting from
    /// `from_renderer`.
    pub(crate) fn new(to_renderer: Producer<Command>, from_renderer: Consumer<Return>) -> Self {
        Self(Arc::new(Shared {
            rings: Mutex::new(Rings::new(to_renderer, from_renderer)),
        }))
    }
}
