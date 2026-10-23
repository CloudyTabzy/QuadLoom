//! Fallible reservations for the initial large mesh buffers.

use crate::{Error, Stage};

pub(crate) fn reserve<T>(buffer: &mut Vec<T>, additional: usize, stage: Stage) -> Result<(), Error> {
    buffer.try_reserve_exact(additional).map_err(|_| {
        Error::internal(stage, "not enough memory to allocate mesh buffers")
            .with_hint("free memory or reduce the input mesh size")
    })
}

#[cfg(test)]
mod tests {
    use crate::{ErrorKind, Stage};

    #[test]
    fn capacity_overflow_is_an_error_without_allocating_or_panicking() {
        let mut buffer = vec![7_u32];
        let error = super::reserve(&mut buffer, usize::MAX, Stage::Validate).unwrap_err();
        assert_eq!(buffer, [7]);
        assert_eq!(error.kind, ErrorKind::Internal);
        assert_eq!(error.stage, Stage::Validate);
        assert!(error.message.contains("memory"));
        assert!(error.hint.is_some());
    }
}
