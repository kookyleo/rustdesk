use super::*;
use crate::clipboard::clipboard_listener;
pub use crate::clipboard::{check_clipboard, ClipboardContext, ClipboardSide};
pub use crate::clipboard::{CLIPBOARD_INTERVAL as INTERVAL, CLIPBOARD_NAME as NAME};
#[cfg(feature = "unix-file-copy-paste")]
pub use crate::{
    clipboard::{check_clipboard_files, FILE_CLIPBOARD_NAME as FILE_NAME},
    clipboard_file::unix_file_clip,
};
use clipboard_master::CallbackResult;
use std::{
    io,
    sync::mpsc::{channel, RecvTimeoutError},
    time::Duration,
};

struct Handler {
    ctx: Option<ClipboardContext>,
}

pub fn new(name: String) -> GenericService {
    let svc = EmptyExtraFieldService::new(name, false);
    GenericService::run(&svc.clone(), run);
    svc.sp
}

fn run(sp: EmptyExtraFieldService) -> ResultType<()> {
    let (tx_cb_result, rx_cb_result) = channel();
    let ctx = Some(ClipboardContext::new().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?);
    clipboard_listener::subscribe(sp.name(), tx_cb_result)?;
    let mut handler = Handler {
        ctx,
    };

    while sp.ok() {
        match rx_cb_result.recv_timeout(Duration::from_millis(INTERVAL)) {
            Ok(CallbackResult::Next) => {
                #[cfg(feature = "unix-file-copy-paste")]
                if sp.name() == FILE_NAME {
                    handler.check_clipboard_file();
                    continue;
                }
                if let Some(msg) = handler.get_clipboard_msg() {
                    sp.send(msg);
                }
            }
            Ok(CallbackResult::Stop) => {
                log::debug!("Clipboard listener stopped");
                break;
            }
            Ok(CallbackResult::StopWithError(err)) => {
                bail!("Clipboard listener stopped with error: {}", err);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                log::error!("Clipboard listener disconnected");
                break;
            }
        }
    }

    clipboard_listener::unsubscribe(&sp.name());

    Ok(())
}

impl Handler {
    #[cfg(feature = "unix-file-copy-paste")]
    fn check_clipboard_file(&mut self) {
        if let Some(urls) = check_clipboard_files(&mut self.ctx, ClipboardSide::Host, false) {
            if !urls.is_empty() {
                if crate::clipboard::is_file_url_set_by_rustdesk(&urls) {
                    return;
                }
                match clipboard::platform::unix::serv_files::sync_files(&urls) {
                    Ok(()) => {
                        // Use `send_data()` here to reuse `handle_file_clip()` in `connection.rs`.
                        hbb_common::allow_err!(clipboard::send_data(
                            0,
                            unix_file_clip::get_format_list()
                        ));
                    }
                    Err(e) => {
                        log::error!("Failed to sync clipboard files: {}", e);
                    }
                }
            }
        }
    }

    fn get_clipboard_msg(&mut self) -> Option<Message> {
        check_clipboard(&mut self.ctx, ClipboardSide::Host, false)
    }
}
