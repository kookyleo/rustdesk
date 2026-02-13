use librustdesk::*;

fn main() {
    crate::common::load_custom_client();
    hbb_common::init_log(false, "service");
    crate::start_os_service();
}
