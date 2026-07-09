use std::{any::type_name, borrow::Cow};

/// A failure while decoding a value from its wire bytes.
///
/// Concrete and non-generic (unlike `nom`'s input-borrowing `error::Error`), so
/// it is `Clone + Eq + 'static` and does not infect every composite `?`. The
/// structured variants cover the common cases; [`DecodeError::Custom`] is the
/// escape hatch for anything else. `#[non_exhaustive]` so variants can grow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    #[error("unexpected end of input while decoding {type_name}: needed {needed} more byte(s)")]
    UnexpectedEnd {
        type_name: &'static str,
        needed: usize,
    },

    #[error("invalid encoding for {type_name}: {message}")]
    InvalidValue {
        type_name: &'static str,
        message: Cow<'static, str>,
    },

    #[error("unknown discriminant {discriminant:#x} for {type_name}")]
    UnknownDiscriminant {
        type_name: &'static str,
        discriminant: u64,
    },

    #[error("length {len} out of bounds [{min}, {max}] for {type_name}")]
    LengthOutOfBounds {
        type_name: &'static str,
        len: usize,
        min: usize,
        max: usize,
    },

    #[error("{0}")]
    Custom(Cow<'static, str>),
}

impl DecodeError {
    /// The input ran out `needed` bytes short while decoding a `T`.
    #[must_use]
    pub fn end_of_input<T: ?Sized>(needed: usize) -> Self {
        Self::UnexpectedEnd {
            type_name: type_name::<T>(),
            needed,
        }
    }

    /// The bytes for a `T` were well-sized but semantically invalid.
    #[must_use]
    pub fn invalid_value<T: ?Sized>(message: impl Into<Cow<'static, str>>) -> Self {
        Self::InvalidValue {
            type_name: type_name::<T>(),
            message: message.into(),
        }
    }

    /// A `T` tag/discriminant did not match any known variant.
    #[must_use]
    pub fn unknown_discriminant<T: ?Sized>(discriminant: u64) -> Self {
        Self::UnknownDiscriminant {
            type_name: type_name::<T>(),
            discriminant,
        }
    }

    /// A `T`'s decoded length fell outside its `[min, max]` bound.
    #[must_use]
    pub fn length_out_of_bounds<T: ?Sized>(len: usize, min: usize, max: usize) -> Self {
        Self::LengthOutOfBounds {
            type_name: type_name::<T>(),
            len,
            min,
            max,
        }
    }

    /// An arbitrary decode failure that the structured variants do not capture.
    #[must_use]
    pub fn custom(message: impl Into<Cow<'static, str>>) -> Self {
        Self::Custom(message.into())
    }
}
