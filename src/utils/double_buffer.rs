/// Container for double buffered values.
#[derive(Debug)]
pub struct DoubleBuffered<T> {
    pending: T,
    current: Option<T>,
}

impl<T: DoubleBufferable> DoubleBuffered<T> {
    pub fn new() -> Self {
        Self {
            pending: T::default(),
            current: None,
        }
    }

    pub fn pending(&self) -> &T {
        &self.pending
    }

    pub fn pending_mut(&mut self) -> &mut T {
        &mut self.pending
    }

    pub fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }

    pub fn apply_pending(&mut self) -> &T {
        self.pending
            .merge_into(&mut self.current.get_or_insert_with(Default::default));
        self.current.as_ref().unwrap()
    }
}

pub trait DoubleBufferable: Default {
    fn merge_into(&self, into: &mut Self);
}
