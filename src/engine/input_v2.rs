use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

#[derive(Debug)]
pub(crate) struct InputState {
    id: AtomicUsize,
}

#[derive(Debug)]
pub(crate) struct Inputs {
    states: Vec<Arc<InputState>>,
}

pub(crate) fn create(count: usize) -> (Inputs, realtime::Inputs) {
    let states: Vec<Arc<InputState>> = (0..count)
        .map(|id| {
            Arc::new(InputState {
                id: AtomicUsize::new(id),
            })
        })
        .collect();
    let realtime = realtime::Inputs::new(states.clone());
    (Inputs { states }, realtime)
}

pub(crate) mod realtime {
    use std::sync::Arc;

    use super::InputState;

    #[derive(Debug)]
    pub(crate) struct Inputs {
        states: Vec<Arc<InputState>>,
    }

    impl Inputs {
        pub(crate) fn new(states: Vec<Arc<InputState>>) -> Self {
            Self { states }
        }
    }
}
