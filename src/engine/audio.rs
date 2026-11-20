use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use thiserror::Error;

use crate::layout::ChannelLayout;

#[derive(Debug, Default)]
pub(crate) struct AudioState {
    /// `None` while its place is free.
    pub(crate) buffer: Option<AudioBuffer>,
    pub(crate) unload: AtomicBool,
}

/// Lives in the state, so the control side's pool holds it until the
/// place is reused and the renderer never frees it.
#[derive(Debug)]
pub(crate) struct AudioBuffer {
    pub(crate) samples: Vec<f32>,
    pub(crate) layout: ChannelLayout,
}

/// Nothing was loaded; the samples come back in it.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("the samples end part-way through a frame")]
    PartFrame(Vec<f32>),
    #[error("every place for loaded audio is held")]
    Full(Vec<f32>),
}

impl LoadError {
    pub fn into_samples(self) -> Vec<f32> {
        match self {
            Self::PartFrame(samples) | Self::Full(samples) => samples,
        }
    }
}

pub struct Audio {
    state: Arc<AudioState>,
}

impl Audio {
    pub(crate) fn load(state: Arc<AudioState>) -> Self {
        Self { state }
    }
}
