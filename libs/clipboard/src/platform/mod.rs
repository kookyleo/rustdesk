#[cfg(feature = "unix-file-copy-paste")]
pub mod unix;

#[cfg(feature = "unix-file-copy-paste")]
pub fn create_cliprdr_context(
    _enable_files: bool,
    _enable_others: bool,
    _response_wait_timeout_secs: u32,
) -> crate::ResultType<Box<dyn crate::CliprdrServiceContext>> {
    let boxed = unix::macos::pasteboard_context::create_pasteboard_context()? as Box<_>;
    Ok(boxed)
}
