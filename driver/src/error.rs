use error_stack::Report;
use kernel::KernelError;

pub trait ConvertError {
    type Ok;
    fn convert_error(self) -> error_stack::Result<Self::Ok, KernelError>;
}

impl<T> ConvertError for Result<T, serde_json::Error> {
    type Ok = T;

    fn convert_error(self) -> error_stack::Result<T, KernelError> {
        self.map_err(|error| Report::from(error).change_context(KernelError::Internal))
    }
}
