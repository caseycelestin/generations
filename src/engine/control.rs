use crate::engine::audio::AudioPool;
use crate::engine::inputs::Inputs;
use crate::engine::recording::Recorders;

/// One per renderer; an app shares it its own way. Everything done here or
/// on a handle takes effect at the renderer's next period.
#[derive(Debug)]
pub struct Control {
    inputs: Inputs,
    pool: AudioPool,
    recorders: Recorders,
}

impl Control {
    pub(crate) fn new(inputs: Inputs, pool: AudioPool, recorders: Recorders) -> Self {
        Self {
            inputs,
            pool,
            recorders,
        }
    }

    pub fn audio(&mut self) -> &mut AudioPool {
        &mut self.pool
    }

    pub fn inputs(&mut self) -> &mut Inputs {
        &mut self.inputs
    }

    pub fn recorders(&mut self) -> &mut Recorders {
        &mut self.recorders
    }
}
