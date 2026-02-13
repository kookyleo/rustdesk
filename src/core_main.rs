use crate::client::translate;
#[cfg(not(debug_assertions))]
use crate::platform::breakdown_callback;
#[cfg(not(debug_assertions))]
use hbb_common::platform::register_breakdown_handler;
use hbb_common::{config, log};

#[macro_export]
macro_rules! my_println{
    ($($arg:tt)*) => {
        println!("{}", format_args!($($arg)*));
    };
}

/// shared by flutter and sciter main function
///
/// [Note]
/// If it returns [`None`], then the process will terminate, and flutter gui will not be started.
/// If it returns [`Some`], then the process will continue, and flutter gui will be started.
pub fn core_main() -> Option<Vec<String>> {
    if !crate::common::global_init() {
        return None;
    }
    crate::load_custom_client();
    let mut args = Vec::new();
    let mut flutter_args = Vec::new();
    let mut i = 0;
    let mut _is_elevate = false;
    let mut _is_run_as_system = false;
    let mut _is_quick_support = false;
    let mut _is_flutter_invoke_new_connection = false;
    let mut no_server = false;
    for arg in std::env::args() {
        if i == 0 {
            let _ = arg;
        } else if i > 0 {
            #[cfg(feature = "flutter")]
            if [
                "--connect",
                "--play",
                "--file-transfer",
                "--view-camera",
                "--port-forward",
                "--terminal",
                "--rdp",
            ]
            .contains(&arg.as_str())
            {
                _is_flutter_invoke_new_connection = true;
            }
            if arg == "--elevate" {
                _is_elevate = true;
            } else if arg == "--run-as-system" {
                _is_run_as_system = true;
            } else if arg == "--quick_support" {
                _is_quick_support = true;
            } else if arg == "--no-server" {
                no_server = true;
            } else {
                args.push(arg);
            }
        }
        i += 1;
    }
    #[cfg(not(debug_assertions))]
    register_breakdown_handler(breakdown_callback);
    #[cfg(feature = "flutter")]
    if _is_flutter_invoke_new_connection {
        return core_main_invoke_new_connection(std::env::args());
    }
    let click_setup = false;
    if click_setup && !config::is_disable_installation() {
        args.push("--install".to_owned());
        flutter_args.push("--install".to_string());
    }
    if args.contains(&"--noinstall".to_string()) {
        args.clear();
    }
    if args.len() > 0 {
        if args[0] == "--version" {
            println!("{}", crate::VERSION);
            return None;
        } else if args[0] == "--build-date" {
            println!("{}", crate::BUILD_DATE);
            return None;
        }
    }
    let mut log_name = "".to_owned();
    if args.len() > 0 && args[0].starts_with("--") {
        let name = args[0].replace("--", "");
        if !name.is_empty() {
            log_name = name;
        }
    }
    hbb_common::init_log(false, &log_name);

    // linux uni (url) go here.
    #[cfg(all(feature = "flutter", feature = "plugin_framework"))]
    init_plugins(&args);
    if args.is_empty() || crate::common::is_empty_uni_link(&args[0]) {
        crate::platform::macos::try_remove_temp_update_dir(None);

        std::thread::spawn(move || crate::start_server(false, no_server));
    } else {
        use crate::platform;
        if args[0] == "--update" {
            if args.len() > 1 && args[1].ends_with(".dmg") {
                // Version check is unnecessary unless downgrading to an older version
                // that lacks "update dmg" support. This is a special case since we cannot
                // detect the version before extracting the DMG, so we skip the check.
                let dmg_path = &args[1];
                println!("Updating from DMG: {}", dmg_path);
                match platform::update_from_dmg(dmg_path) {
                    Ok(_) => {
                        println!("Update process from DMG started successfully.");
                        // The new process will handle the rest. We can exit.
                    }
                    Err(err) => {
                        eprintln!("Failed to start update from DMG: {}", err);
                    }
                }
            } else {
                println!("Starting update process...");
                log::info!("Starting update process...");
                let _text = match platform::update_me() {
                    Ok(_) => {
                        println!("{}", translate("Update successfully!".to_string()));
                        log::info!("Update successfully!");
                    }
                    Err(err) => {
                        eprintln!("Update failed with error: {}", err);
                        log::error!("Update failed with error: {err}");
                    }
                };
            }
            return None;
        }
        if args[0] == "--remove" {
            if args.len() == 2 {
                // sleep a while so that process of removed exe exit
                std::thread::sleep(std::time::Duration::from_secs(1));
                std::fs::remove_file(&args[1]).ok();
                return None;
            }
        } else if args[0] == "--tray" {
            if !crate::check_process("--tray", true) {
                crate::tray::start_tray();
            }
            return None;
        } else if args[0] == "--install-service" {
            log::info!("start --install-service");
            crate::platform::install_service();
            return None;
        } else if args[0] == "--uninstall-service" {
            log::info!("start --uninstall-service");
            crate::platform::uninstall_service(false, true);
            return None;
        } else if args[0] == "--service" {
            log::info!("start --service");
            crate::start_os_service();
            return None;
        } else if args[0] == "--server" {
            log::info!("start --server with user {}", crate::username());
            let handler = std::thread::spawn(move || crate::start_server(true, false));
            crate::tray::start_tray();
            // prevent server exit when encountering errors from tray
            hbb_common::allow_err!(handler.join());
            return None;
        } else if args[0] == "--import-config" {
            if args.len() == 2 {
                let filepath;
                let path = std::path::Path::new(&args[1]);
                if !path.is_absolute() {
                    let mut cur = std::env::current_dir().unwrap();
                    cur.push(path);
                    filepath = cur.to_str().unwrap().to_string();
                } else {
                    filepath = path.to_str().unwrap().to_string();
                }
                import_config(&filepath);
            }
            return None;
        } else if args[0] == "--password" {
            if config::is_disable_settings() {
                println!("Settings are disabled!");
                return None;
            }
            if config::Config::is_disable_change_permanent_password() {
                println!("Changing permanent password is disabled!");
                return None;
            }
            if args.len() == 2 {
                if crate::platform::is_installed() && is_root() {
                    if let Err(err) = crate::ipc::set_permanent_password(args[1].to_owned()) {
                        println!("{err}");
                    } else {
                        println!("Done!");
                    }
                } else {
                    println!("Installation and administrative privileges required!");
                }
            }
            return None;
        } else if args[0] == "--set-unlock-pin" {
            if config::Config::is_disable_unlock_pin() {
                println!("Unlock PIN is disabled!");
                return None;
            }
            #[cfg(feature = "flutter")]
            if args.len() == 2 {
                if crate::platform::is_installed() && is_root() {
                    if let Err(err) = crate::ipc::set_unlock_pin(args[1].to_owned(), false) {
                        println!("{err}");
                    } else {
                        println!("Done!");
                    }
                } else {
                    println!("Installation and administrative privileges required!");
                }
            }
            return None;
        } else if args[0] == "--get-id" {
            println!("{}", crate::ipc::get_id());
            return None;
        } else if args[0] == "--set-id" {
            if config::is_disable_settings() {
                println!("Settings are disabled!");
                return None;
            }
            if config::Config::is_disable_change_id() {
                println!("Changing ID is disabled!");
                return None;
            }
            if args.len() == 2 {
                if crate::platform::is_installed() && is_root() {
                    let old_id = crate::ipc::get_id();
                    let mut res = crate::ui_interface::change_id_shared(args[1].to_owned(), old_id);
                    if res.is_empty() {
                        res = "Done!".to_owned();
                    }
                    println!("{}", res);
                } else {
                    println!("Installation and administrative privileges required!");
                }
            }
            return None;
        } else if args[0] == "--config" {
            if args.len() == 2 && !args[0].contains("host=") {
                if crate::platform::is_installed() && is_root() {
                    // encrypted string used in renaming exe.
                    let name = if args[1].ends_with(".exe") {
                        args[1].to_owned()
                    } else {
                        format!("{}.exe", args[1])
                    };
                    if let Ok(lic) = crate::custom_server::get_custom_server_from_string(&name) {
                        if !lic.host.is_empty() {
                            crate::ui_interface::set_option("key".into(), lic.key);
                            crate::ui_interface::set_option(
                                "custom-rendezvous-server".into(),
                                lic.host,
                            );
                            crate::ui_interface::set_option("api-server".into(), lic.api);
                            crate::ui_interface::set_option("relay-server".into(), lic.relay);
                        }
                    }
                } else {
                    println!("Installation and administrative privileges required!");
                }
            }
            return None;
        } else if args[0] == "--option" {
            if config::is_disable_settings() {
                println!("Settings are disabled!");
                return None;
            }
            if crate::platform::is_installed() && is_root() {
                if args.len() == 2 {
                    let options = crate::ipc::get_options();
                    println!("{}", options.get(&args[1]).unwrap_or(&"".to_owned()));
                } else if args.len() == 3 {
                    crate::ipc::set_option(&args[1], &args[2]);
                }
            } else {
                println!("Installation and administrative privileges required!");
            }
            return None;
        } else if args[0] == "--assign" {
            if config::Config::no_register_device() {
                println!("Cannot assign an unregistrable device!");
            } else if crate::platform::is_installed() && is_root() {
                let max = args.len() - 1;
                let pos = args.iter().position(|x| x == "--token").unwrap_or(max);
                if pos < max {
                    let token = args[pos + 1].to_owned();
                    let id = crate::ipc::get_id();
                    let uuid = crate::encode64(hbb_common::get_uuid());
                    let get_value = |c: &str| {
                        let pos = args.iter().position(|x| x == c).unwrap_or(max);
                        if pos < max {
                            Some(args[pos + 1].to_owned())
                        } else {
                            None
                        }
                    };
                    let user_name = get_value("--user_name");
                    let strategy_name = get_value("--strategy_name");
                    let address_book_name = get_value("--address_book_name");
                    let address_book_tag = get_value("--address_book_tag");
                    let address_book_alias = get_value("--address_book_alias");
                    let address_book_password = get_value("--address_book_password");
                    let address_book_note = get_value("--address_book_note");
                    let device_group_name = get_value("--device_group_name");
                    let note = get_value("--note");
                    let device_username = get_value("--device_username");
                    let device_name = get_value("--device_name");
                    let mut body = serde_json::json!({
                        "id": id,
                        "uuid": uuid,
                    });
                    let header = "Authorization: Bearer ".to_owned() + &token;
                    if user_name.is_none()
                        && strategy_name.is_none()
                        && address_book_name.is_none()
                        && device_group_name.is_none()
                        && note.is_none()
                        && device_username.is_none()
                        && device_name.is_none()
                    {
                        println!(
                            r#"At least one of the following options is required:
  --user_name
  --strategy_name
  --address_book_name
  --device_group_name
  --note
  --device_username
  --device_name"#
                        );
                    } else {
                        if let Some(name) = user_name {
                            body["user_name"] = serde_json::json!(name);
                        }
                        if let Some(name) = strategy_name {
                            body["strategy_name"] = serde_json::json!(name);
                        }
                        if let Some(name) = address_book_name {
                            body["address_book_name"] = serde_json::json!(name);
                            if let Some(name) = address_book_tag {
                                body["address_book_tag"] = serde_json::json!(name);
                            }
                            if let Some(name) = address_book_alias {
                                body["address_book_alias"] = serde_json::json!(name);
                            }
                            if let Some(name) = address_book_password {
                                body["address_book_password"] = serde_json::json!(name);
                            }
                            if let Some(name) = address_book_note {
                                body["address_book_note"] = serde_json::json!(name);
                            }
                        }
                        if let Some(name) = device_group_name {
                            body["device_group_name"] = serde_json::json!(name);
                        }
                        if let Some(name) = note {
                            body["note"] = serde_json::json!(name);
                        }
                        if let Some(name) = device_username {
                            body["device_username"] = serde_json::json!(name);
                        }
                        if let Some(name) = device_name {
                            body["device_name"] = serde_json::json!(name);
                        }
                        let url = crate::ui_interface::get_api_server() + "/api/devices/cli";
                        match crate::post_request_sync(url, body.to_string(), &header) {
                            Err(err) => println!("{}", err),
                            Ok(text) => {
                                if text.is_empty() {
                                    println!("Done!");
                                } else {
                                    println!("{}", text);
                                }
                            }
                        }
                    }
                } else {
                    println!("--token is required!");
                }
            } else {
                println!("Installation and administrative privileges required!");
            }
            return None;
        } else if args[0] == "--check-hwcodec-config" {
            #[cfg(feature = "hwcodec")]
            crate::ipc::hwcodec_process();
            return None;
        } else if args[0] == "--terminal-helper" {
            // Terminal helper process - runs as user to create ConPTY
            // This is needed because ConPTY has compatibility issues with CreateProcessAsUserW
            return None;
        } else if args[0] == "--cm" {
            // call connection manager to establish connections
            // meanwhile, return true to call flutter window to show control panel
            crate::ui_interface::start_option_status_sync();
        } else if args[0] == "--cm-no-ui" {
            #[cfg(feature = "flutter")]
            {
                crate::ui_interface::start_option_status_sync();
                crate::flutter::connection_manager::start_cm_no_ui();
            }
            return None;
        } else if args[0] == "--whiteboard" {
            crate::whiteboard::run();
            return None;
        } else if args[0] == "-gtk-sudo" {
            return None;
        } else {
            #[cfg(all(feature = "flutter", feature = "plugin_framework"))]
            if args[0] == "--plugin-install" {
                if args.len() == 2 {
                    crate::plugin::change_uninstall_plugin(&args[1], false);
                } else if args.len() == 3 {
                    crate::plugin::install_plugin_with_url(&args[1], &args[2]);
                }
                return None;
            } else if args[0] == "--plugin-uninstall" {
                if args.len() == 2 {
                    crate::plugin::change_uninstall_plugin(&args[1], true);
                }
                return None;
            }
        }
    }
    //_async_logger_holder.map(|x| x.flush());
    #[cfg(feature = "flutter")]
    return Some(flutter_args);
    #[cfg(not(feature = "flutter"))]
    return Some(args);
}

#[inline]
#[cfg(all(feature = "flutter", feature = "plugin_framework"))]
fn init_plugins(args: &Vec<String>) {
    if args.is_empty() || "--server" == (&args[0] as &str) {
        #[cfg(debug_assertions)]
        let load_plugins = true;
        #[cfg(not(debug_assertions))]
        let load_plugins = crate::platform::is_installed();
        if load_plugins {
            crate::plugin::init();
        }
    } else if "--service" == (&args[0] as &str) {
        hbb_common::allow_err!(crate::plugin::remove_uninstalled());
    }
}

fn import_config(path: &str) {
    use hbb_common::{config::*, get_exe_time, get_modified_time};
    let path2 = path.replace(".toml", "2.toml");
    let path2 = std::path::Path::new(&path2);
    let path = std::path::Path::new(path);
    log::info!("import config from {:?} and {:?}", path, path2);
    let config: Config = load_path(path.into());
    if config.is_empty() {
        log::info!("Empty source config, skipped");
        return;
    }
    if get_modified_time(&path) > get_modified_time(&Config::file())
        && get_modified_time(&path) < get_exe_time()
    {
        if store_path(Config::file(), config).is_err() {
            log::info!("config written");
        }
    }
    let config2: Config2 = load_path(path2.into());
    if get_modified_time(&path2) > get_modified_time(&Config2::file()) {
        if store_path(Config2::file(), config2).is_err() {
            log::info!("config2 written");
        }
    }
}

/// invoke a new connection
///
/// [Note]
/// this is for invoke new connection from dbus.
/// If it returns [`None`], then the process will terminate, and flutter gui will not be started.
/// If it returns [`Some`], then the process will continue, and flutter gui will be started.
#[cfg(feature = "flutter")]
fn core_main_invoke_new_connection(mut args: std::env::Args) -> Option<Vec<String>> {
    let mut authority = None;
    let mut id = None;
    let mut param_array = vec![];
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connect" | "--play" | "--file-transfer" | "--view-camera" | "--port-forward"
            | "--terminal" | "--rdp" => {
                authority = Some((&arg.to_string()[2..]).to_owned());
                id = args.next();
            }
            "--password" => {
                if let Some(password) = args.next() {
                    param_array.push(format!("password={password}"));
                }
            }
            "--relay" => {
                param_array.push(format!("relay=true"));
            }
            // inner
            "--switch_uuid" => {
                if let Some(switch_uuid) = args.next() {
                    param_array.push(format!("switch_uuid={switch_uuid}"));
                }
            }
            _ => {}
        }
    }
    let mut uni_links = Default::default();
    if let Some(authority) = authority {
        if let Some(mut id) = id {
            let app_name = crate::get_app_name();
            let ext = format!(".{}", app_name.to_lowercase());
            if id.ends_with(&ext) {
                id = id.replace(&ext, "");
            }
            let params = param_array.join("&");
            let params_flag = if params.is_empty() { "" } else { "?" };
            uni_links = format!(
                "{}{}/{}{}{}",
                crate::get_uri_prefix(),
                authority,
                id,
                params_flag,
                params
            );
        }
    }
    if uni_links.is_empty() {
        return None;
    }

    return if let Err(_) = crate::ipc::send_url_scheme(uni_links) {
        Some(Vec::new())
    } else {
        None
    };
}

fn is_root() -> bool {
    crate::platform::is_root()
}
