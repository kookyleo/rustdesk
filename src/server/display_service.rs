use super::*;
use crate::common::SimpleCallOnReturn;
use hbb_common::protobuf::MessageField;
use scrap::Display;
use std::sync::atomic::{AtomicBool, Ordering};

// https://github.com/rustdesk/rustdesk/discussions/6042, avoiding dbus call

pub const NAME: &'static str = "display";

struct ChangedResolution {
    original: (i32, i32),
    changed: (i32, i32),
}

lazy_static::lazy_static! {
    static ref CHANGED_RESOLUTIONS: Arc<RwLock<HashMap<String, ChangedResolution>>> = Default::default();
    // Initial primary display index.
    // It should not be updated when displays changed.
    pub static ref PRIMARY_DISPLAY_IDX: usize = get_primary();
    static ref SYNC_DISPLAYS: Arc<Mutex<SyncDisplaysInfo>> = Default::default();
}

// https://github.com/rustdesk/rustdesk/pull/8537
static TEMP_IGNORE_DISPLAYS_CHANGED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct SyncDisplaysInfo {
    displays: Vec<DisplayInfo>,
    is_synced: bool,
}

impl SyncDisplaysInfo {
    fn check_changed(&mut self, displays: Vec<DisplayInfo>) {
        if self.displays.len() != displays.len() {
            self.displays = displays;
            if !TEMP_IGNORE_DISPLAYS_CHANGED.load(Ordering::Relaxed) {
                self.is_synced = false;
            }
            return;
        }
        for (i, d) in displays.iter().enumerate() {
            if d != &self.displays[i] {
                self.displays = displays;
                if !TEMP_IGNORE_DISPLAYS_CHANGED.load(Ordering::Relaxed) {
                    self.is_synced = false;
                }
                return;
            }
        }
    }

    fn get_update_sync_displays(&mut self) -> Option<Vec<DisplayInfo>> {
        if self.is_synced {
            return None;
        }
        self.is_synced = true;
        Some(self.displays.clone())
    }
}

pub fn temp_ignore_displays_changed() -> SimpleCallOnReturn {
    TEMP_IGNORE_DISPLAYS_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
    SimpleCallOnReturn {
        b: true,
        f: Box::new(move || {
            // Wait for a while to make sure check_display_changed() is called
            // after video service has sending its `SwitchDisplay` message(`try_broadcast_display_changed()`).
            std::thread::sleep(Duration::from_millis(1000));
            TEMP_IGNORE_DISPLAYS_CHANGED.store(false, Ordering::Relaxed);
            // Trigger the display changed message.
            SYNC_DISPLAYS.lock().unwrap().is_synced = false;
        }),
    }
}

// This function is really useful, though a duplicate check if display changed.
// The video server will then send the following messages to the client:
//  1. the supported resolutions of the {idx} display
//  2. the switch resolution message, so that the client can record the custom resolution.
pub(super) fn check_display_changed(
    ndisplay: usize,
    idx: usize,
    (x, y, w, h): (i32, i32, usize, usize),
) -> Option<DisplayInfo> {
    let lock = SYNC_DISPLAYS.lock().unwrap();
    // If plugging out a monitor && lock.displays.get(idx) is None.
    //  1. The client version < 1.2.4. The client side has to reconnect.
    //  2. The client version > 1.2.4, The client side can handle the case because sync peer info message will be sent.
    // But it is acceptable to for the user to reconnect manually, because the monitor is unplugged.
    let d = lock.displays.get(idx)?;
    if ndisplay != lock.displays.len() {
        return Some(d.clone());
    }
    if !(d.x == x && d.y == y && d.width == w as i32 && d.height == h as i32) {
        Some(d.clone())
    } else {
        None
    }
}

#[inline]
pub fn set_last_changed_resolution(display_name: &str, original: (i32, i32), changed: (i32, i32)) {
    let mut lock = CHANGED_RESOLUTIONS.write().unwrap();
    match lock.get_mut(display_name) {
        Some(res) => res.changed = changed,
        None => {
            lock.insert(
                display_name.to_owned(),
                ChangedResolution { original, changed },
            );
        }
    }
}

#[inline]
pub fn restore_resolutions() {
    for (name, res) in CHANGED_RESOLUTIONS.read().unwrap().iter() {
        let (w, h) = res.original;
        log::info!("Restore resolution of display '{}' to ({}, {})", name, w, h);
        if let Err(e) = crate::platform::change_resolution(name, w as _, h as _) {
            log::error!(
                "Failed to restore resolution of display '{}' to ({},{}): {}",
                name,
                w,
                h,
                e
            );
        }
    }
    // Can be cleared because restore resolutions is called when there is no client connected.
    CHANGED_RESOLUTIONS.write().unwrap().clear();
}

#[inline]
pub fn capture_cursor_embedded() -> bool {
    scrap::is_cursor_embedded()
}

pub fn new() -> GenericService {
    let svc = EmptyExtraFieldService::new(NAME.to_owned(), true);
    GenericService::run(&svc.clone(), run);
    svc.sp
}

fn displays_to_msg(displays: Vec<DisplayInfo>) -> Message {
    let mut pi = PeerInfo {
        ..Default::default()
    };
    pi.displays = displays.clone();

    // current_display should not be used in server.
    // It is set to 0 for compatibility with old clients.
    pi.current_display = 0;
    let mut msg_out = Message::new();
    msg_out.set_peer_info(pi);
    msg_out
}

fn check_get_displays_changed_msg() -> Option<Message> {
    check_update_displays(&try_get_displays().ok()?);
    get_displays_msg()
}

pub fn check_displays_changed() -> ResultType<()> {
    check_update_displays(&try_get_displays()?);
    Ok(())
}

fn get_displays_msg() -> Option<Message> {
    let displays = SYNC_DISPLAYS.lock().unwrap().get_update_sync_displays()?;
    Some(displays_to_msg(displays))
}

fn run(sp: EmptyExtraFieldService) -> ResultType<()> {
    while sp.ok() {
        sp.snapshot(|sps| {
            if !TEMP_IGNORE_DISPLAYS_CHANGED.load(Ordering::Relaxed) {
                if sps.has_subscribes() {
                    SYNC_DISPLAYS.lock().unwrap().is_synced = false;
                    bail!("new subscriber");
                }
            }
            Ok(())
        })?;

        if let Some(msg_out) = check_get_displays_changed_msg() {
            sp.send(msg_out);
            log::info!("Displays changed");
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    Ok(())
}

#[inline]
pub(super) fn get_original_resolution(
    display_name: &str,
    w: usize,
    h: usize,
) -> MessageField<Resolution> {
    let changed_resolutions = CHANGED_RESOLUTIONS.write().unwrap();
    let (width, height) = match changed_resolutions.get(display_name) {
        Some(res) => {
            res.original
            /*
            The resolution change may not happen immediately, `changed` has been updated,
            but the actual resolution is old, it will be mistaken for a third-party change.
            if res.changed.0 != w as i32 || res.changed.1 != h as i32 {
                // If the resolution is changed by third process, remove the record in changed_resolutions.
                changed_resolutions.remove(display_name);
                (w as _, h as _)
            } else {
                res.original
            }
            */
        }
        None => (w as _, h as _),
    };
    Some(Resolution {
        width,
        height,
        ..Default::default()
    })
    .into()
}

pub(super) fn get_sync_displays() -> Vec<DisplayInfo> {
    SYNC_DISPLAYS.lock().unwrap().displays.clone()
}

pub(super) fn get_display_info(idx: usize) -> Option<DisplayInfo> {
    SYNC_DISPLAYS.lock().unwrap().displays.get(idx).cloned()
}

// Display to DisplayInfo
// The DisplayInfo is be sent to the peer.
pub(super) fn check_update_displays(all: &Vec<Display>) {
    let displays = all
        .iter()
        .map(|d| {
            let display_name = d.name();
            let scale = d.scale();
            let original_resolution = get_original_resolution(
                &display_name,
                ((d.width() as f64) / scale).round() as usize,
                (d.height() as f64 / scale).round() as usize,
            );
            DisplayInfo {
                x: d.origin().0 as _,
                y: d.origin().1 as _,
                width: d.width() as _,
                height: d.height() as _,
                name: display_name,
                online: d.is_online(),
                cursor_embedded: false,
                original_resolution,
                scale,
                ..Default::default()
            }
        })
        .collect::<Vec<DisplayInfo>>();
    SYNC_DISPLAYS.lock().unwrap().check_changed(displays);
}

pub fn is_inited_msg() -> Option<Message> {
    None
}

pub async fn update_get_sync_displays_on_login() -> ResultType<Vec<DisplayInfo>> {
    let displays = display_service::try_get_displays();
    check_update_displays(&displays?);
    Ok(SYNC_DISPLAYS.lock().unwrap().displays.clone())
}

#[inline]
pub fn get_primary() -> usize {
    try_get_displays().map(|d| get_primary_2(&d)).unwrap_or(0)
}

#[inline]
pub fn get_primary_2(all: &Vec<Display>) -> usize {
    all.iter().position(|d| d.is_primary()).unwrap_or(0)
}

#[inline]
pub fn try_get_displays() -> ResultType<Vec<Display>> {
    Ok(Display::all()?)
}
