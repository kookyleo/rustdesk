use hbb_common::ResultType;

#[no_mangle]
pub fn download_driver() -> ResultType<()> {
    // process download and report progress

    Ok(())
}

#[no_mangle]
pub fn install_update_driver(_reboot_required: &mut bool) -> ResultType<()> {
    Ok(())
}

#[no_mangle]
pub fn uninstall_driver(_reboot_required: &mut bool) -> ResultType<()> {
    Ok(())
}

#[no_mangle]
pub fn is_device_created() -> bool {
    false
}

#[no_mangle]
pub fn create_device() -> ResultType<()> {
    if is_device_created() {
        return Ok(());
    }
    Ok(())
}

#[no_mangle]
pub fn close_device() {
}

type PMonitorMode = *mut std::ffi::c_void;

#[no_mangle]
pub fn plug_in_monitor(_monitor_index: u32, _edid: u32, _retries: u32) -> ResultType<()> {
    Ok(())
}

#[no_mangle]
pub fn plug_out_monitor(_monitor_index: u32) -> ResultType<()> {
    Ok(())
}

#[no_mangle]
pub fn update_monitor_modes(
    _monitor_index: u32,
    _mode_count: u32,
    _modes: PMonitorMode,
) -> ResultType<()> {
    Ok(())
}
