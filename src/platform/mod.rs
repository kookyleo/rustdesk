pub use macos::*;
pub mod macos;
pub mod delegate;
use hbb_common::{
    message_proto::CursorData,
    sysinfo::{Pid, System},
    ResultType,
};
use std::sync::{Arc, Mutex};

pub const SERVICE_INTERVAL: u64 = 300;

lazy_static::lazy_static! {
    static ref INSTALLING_SERVICE: Arc<Mutex<bool>>= Default::default();
}

pub fn installing_service() -> bool {
    INSTALLING_SERVICE.lock().unwrap().clone()
}

pub fn is_xfce() -> bool {
    false
}

pub fn breakdown_callback() {
    crate::input_service::release_device_modifiers();
}

pub fn change_resolution(name: &str, width: usize, height: usize) -> ResultType<()> {
    let cur_resolution = current_resolution(name)?;
    // For MacOS
    // to-do: Make sure the following comparison works.
    // For Linux
    // Just run "xrandr", dpi may not be taken into consideration.
    // For Windows
    // dmPelsWidth and dmPelsHeight is the same to width and height
    // Because this process is running in dpi awareness mode.
    if cur_resolution.width as usize == width && cur_resolution.height as usize == height {
        return Ok(());
    }
    hbb_common::log::warn!("Change resolution of '{}' to ({},{})", name, width, height);
    change_resolution_directly(name, width, height)
}

pub fn get_wakelock(_display: bool) -> WakeLock {
    hbb_common::log::info!("new wakelock, require display on: {_display}");
    // display: keep screen on
    // idle: keep cpu on
    // sleep: prevent system from sleeping, even manually
    crate::platform::WakeLock::new(_display, true, false)
}

pub(crate) struct InstallingService; // please use new

impl InstallingService {
    pub fn new() -> Self {
        *INSTALLING_SERVICE.lock().unwrap() = true;
        Self
    }
}

impl Drop for InstallingService {
    fn drop(&mut self) {
        *INSTALLING_SERVICE.lock().unwrap() = false;
    }
}

// Note: This method is inefficient. It will get all the processes.
// It should only be called when performance is not critical.
#[allow(dead_code)]
fn get_pids_of_process_with_args<S1: AsRef<str>, S2: AsRef<str>>(
    name: S1,
    args: &[S2],
) -> Vec<Pid> {
    let name = name.as_ref().to_lowercase();
    let system = System::new_all();
    system
        .processes()
        .iter()
        .filter(|(_, process)| {
            process.name().to_lowercase() == name
                && process.cmd().len() == args.len() + 1
                && args.iter().enumerate().all(|(i, arg)| {
                    process.cmd()[i + 1].to_lowercase() == arg.as_ref().to_lowercase()
                })
        })
        .map(|(&pid, _)| pid)
        .collect()
}

// Note: This method is inefficient. It will get all the processes.
// It should only be called when performance is not critical.
pub fn get_pids_of_process_with_first_arg<S1: AsRef<str>, S2: AsRef<str>>(
    name: S1,
    arg: S2,
) -> Vec<Pid> {
    let name = name.as_ref().to_lowercase();
    let system = System::new_all();
    system
        .processes()
        .iter()
        .filter(|(_, process)| {
            process.name().to_lowercase() == name
                && process.cmd().len() >= 2
                && process.cmd()[1].to_lowercase() == arg.as_ref().to_lowercase()
        })
        .map(|(&pid, _)| pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_cursor_data() {
        for _ in 0..30 {
            if let Some(hc) = get_cursor().unwrap() {
                let cd = get_cursor_data(hc).unwrap();
                repng::encode(
                    std::fs::File::create("cursor.png").unwrap(),
                    cd.width as _,
                    cd.height as _,
                    &cd.colors[..],
                )
                .unwrap();
            }
            #[cfg(target_os = "macos")]
            macos::is_process_trusted(false);
        }
    }
    #[test]
    fn test_get_cursor_pos() {
        for _ in 0..30 {
            assert!(!get_cursor_pos().is_none());
        }
    }

    #[test]
    fn test_resolution() {
        let name = r"\\.\DISPLAY1";
        println!("current:{:?}", current_resolution(name));
        println!("change:{:?}", change_resolution(name, 2880, 1800));
        println!("resolutions:{:?}", resolutions(name));
    }
}
