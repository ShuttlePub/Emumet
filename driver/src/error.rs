use kernel::KernelError;

pub trait ConvertError {
    type Ok;
    fn convert_error(self) -> error_stack::Result<Self::Ok, KernelError>;
}
