use crate::{errors::OperationStatus, result::FfiResult};

/// Executes a C ABI entry point without allowing a Rust panic to cross the
/// boundary. Panics are converted into a regular runtime error so callers can
/// handle them using the same status protocol as other failures.
pub fn ffi_guard<Return, Function>(function: Function) -> Return
where
    Function: FnOnce() -> Return,
    Return: FfiReturn,
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(function)).unwrap_or_else(|_| {
        Return::from_operation_status(OperationStatus::error(
            crate::errors::OperationStatusCode::RuntimeError,
            "A panic occurred while executing the C API operation.",
        ))
    })
}

/// Checks if a pointer is null and returns from the calling function with a
/// null-pointer error status.
///
/// Works with any return type that implements [`FfiReturn`], including
/// [`FfiResult`], [`OperationStatus`], and `()`.
///
/// # Arguments
///
/// - `$pointer`: The pointer expression to check.
#[macro_export]
macro_rules! return_error_if_null_pointer {
    ($pointer:expr) => {
        if $pointer.is_null() {
            return <_ as $crate::macros::FfiReturn>::from_operation_status(
                $crate::errors::OperationStatus::error(
                    $crate::errors::OperationStatusCode::NullPointer,
                    format!("Received a null `{}` pointer.", stringify!($pointer)),
                ),
            );
        }
    };
}

/// Unwraps a [`Result`], returning the [`Ok`] value, or converts the error
/// into the function's return type and returning early.
///
/// Works with any return type that implements [`FfiReturn`], including
/// [`FfiResult`], [`OperationStatus`], and `()`.
///
/// # Arguments
///
/// - `$result`: The `Result<T, OperationStatus>` expression to unwrap.
#[macro_export]
macro_rules! unwrap_or_return_error {
    ($result:expr) => {
        $crate::unwrap_or_return_error!($result, |_| {})
    };
    ($result:expr, $on_err:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                $on_err(&error);
                return <_ as $crate::macros::FfiReturn>::from_operation_status(error);
            }
        }
    };
}

/// Implemented by FFI return types that can be constructed from an
/// [`OperationStatus`] error, enabling the `return_error_if_null_pointer!` and
/// `unwrap_or_return_error!` macros to work across all return types.
pub trait FfiReturn {
    fn from_operation_status(status: OperationStatus) -> Self;
}

impl<Type: Default> FfiReturn for FfiResult<Type, OperationStatus> {
    fn from_operation_status(status: OperationStatus) -> Self {
        Self::err(status)
    }
}

impl FfiReturn for OperationStatus {
    fn from_operation_status(status: OperationStatus) -> Self {
        status
    }
}

impl FfiReturn for () {
    fn from_operation_status(_status: OperationStatus) -> Self {}
}

#[cfg(test)]
mod test {
    use super::ffi_guard;
    use crate::{OperationStatus, errors::OperationStatusCode};

    #[test]
    fn converts_panics_to_runtime_errors() {
        let status: OperationStatus = ffi_guard(|| panic!("test panic"));

        assert_eq!(status.code, OperationStatusCode::RuntimeError);
    }
}
