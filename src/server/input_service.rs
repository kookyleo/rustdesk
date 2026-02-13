use super::*;
use crate::input::*;
use crate::whiteboard;
use dispatch::Queue;
use enigo::{Enigo, Key, KeyboardControllable, MouseButton, MouseControllable};
use hbb_common::{
    get_time,
    message_proto::{pointer_device_event::Union::TouchEvent, touch_event::Union::ScaleUpdate},
    protobuf::EnumOrUnknown,
};
use rdev::{self, EventType, Key as RdevKey, KeyCode, RawKey};
use rdev::{CGEventSourceStateID, CGEventTapLocation, VirtualInput};
use std::{
    convert::TryFrom,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{self, Duration, Instant},
};

const INVALID_CURSOR_POS: i32 = i32::MIN;
const INVALID_DISPLAY_IDX: i32 = -1;

#[derive(Default)]
struct StateCursor {
    hcursor: u64,
    cursor_data: Arc<Message>,
    cached_cursor_data: HashMap<u64, Arc<Message>>,
}

impl super::service::Reset for StateCursor {
    fn reset(&mut self) {
        *self = Default::default();
        crate::platform::reset_input_cache();
        fix_key_down_timeout(true);
    }
}

struct StatePos {
    cursor_pos: (i32, i32),
}

impl Default for StatePos {
    fn default() -> Self {
        Self {
            cursor_pos: (INVALID_CURSOR_POS, INVALID_CURSOR_POS),
        }
    }
}

impl super::service::Reset for StatePos {
    fn reset(&mut self) {
        self.cursor_pos = (INVALID_CURSOR_POS, INVALID_CURSOR_POS);
    }
}

impl StatePos {
    #[inline]
    fn is_valid(&self) -> bool {
        self.cursor_pos.0 != INVALID_CURSOR_POS
    }

    #[inline]
    fn is_moved(&self, x: i32, y: i32) -> bool {
        self.is_valid() && (self.cursor_pos.0 != x || self.cursor_pos.1 != y)
    }
}

#[derive(Default)]
struct StateWindowFocus {
    display_idx: i32,
}

impl super::service::Reset for StateWindowFocus {
    fn reset(&mut self) {
        self.display_idx = INVALID_DISPLAY_IDX;
    }
}

impl StateWindowFocus {
    #[inline]
    fn is_valid(&self) -> bool {
        self.display_idx != INVALID_DISPLAY_IDX
    }

    #[inline]
    fn is_changed(&self, disp_idx: i32) -> bool {
        self.is_valid() && self.display_idx != disp_idx
    }
}

#[derive(Default, Clone, Copy)]
struct Input {
    conn: i32,
    time: i64,
    x: i32,
    y: i32,
}

const KEY_CHAR_START: u64 = 9999;

#[derive(Clone, Default)]
pub struct MouseCursorSub {
    inner: ConnInner,
    cached: HashMap<u64, Arc<Message>>,
}

impl From<ConnInner> for MouseCursorSub {
    fn from(inner: ConnInner) -> Self {
        Self {
            inner,
            cached: HashMap::new(),
        }
    }
}

impl Subscriber for MouseCursorSub {
    #[inline]
    fn id(&self) -> i32 {
        self.inner.id()
    }

    #[inline]
    fn send(&mut self, msg: Arc<Message>) {
        if let Some(message::Union::CursorData(cd)) = &msg.union {
            if let Some(msg) = self.cached.get(&cd.id) {
                self.inner.send(msg.clone());
            } else {
                self.inner.send(msg.clone());
                let mut tmp = Message::new();
                // only send id out, require client side cache also
                tmp.set_cursor_id(cd.id);
                self.cached.insert(cd.id, Arc::new(tmp));
            }
        } else {
            self.inner.send(msg);
        }
    }
}

struct LockModesHandler;

impl LockModesHandler {
    #[inline]
    fn is_modifier_enabled(key_event: &KeyEvent, modifier: ControlKey) -> bool {
        key_event.modifiers.contains(&modifier.into())
    }

    #[inline]
    fn new_handler(key_event: &KeyEvent, _is_numpad_key: bool) -> Self {
        Self::new(key_event)
    }

    fn new(key_event: &KeyEvent) -> Self {
        let event_caps_enabled = Self::is_modifier_enabled(key_event, ControlKey::CapsLock);
        // Do not use the following code to detect `local_caps_enabled`.
        // Because the state of get_key_state will not affect simulation of `VIRTUAL_INPUT_STATE` in this file.
        //
        // let local_caps_enabled = VirtualInput::get_key_state(
        //     CGEventSourceStateID::CombinedSessionState,
        //     rdev::kVK_CapsLock,
        // );
        let local_caps_enabled = unsafe {
            let _lock = VIRTUAL_INPUT_MTX.lock();
            VIRTUAL_INPUT_STATE
                .as_ref()
                .map_or(false, |input| input.capslock_down)
        };
        if event_caps_enabled && !local_caps_enabled {
            press_capslock();
        } else if !event_caps_enabled && local_caps_enabled {
            release_capslock();
        }

        Self {}
    }
}


pub const NAME_CURSOR: &'static str = "mouse_cursor";
pub const NAME_POS: &'static str = "mouse_pos";
pub const NAME_WINDOW_FOCUS: &'static str = "window_focus";
#[derive(Clone)]
pub struct MouseCursorService {
    pub sp: ServiceTmpl<MouseCursorSub>,
}

impl Deref for MouseCursorService {
    type Target = ServiceTmpl<MouseCursorSub>;

    fn deref(&self) -> &Self::Target {
        &self.sp
    }
}

impl DerefMut for MouseCursorService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sp
    }
}

impl MouseCursorService {
    pub fn new(name: String, need_snapshot: bool) -> Self {
        Self {
            sp: ServiceTmpl::<MouseCursorSub>::new(name, need_snapshot),
        }
    }
}

pub fn new_cursor() -> ServiceTmpl<MouseCursorSub> {
    let svc = MouseCursorService::new(NAME_CURSOR.to_owned(), true);
    ServiceTmpl::<MouseCursorSub>::repeat::<StateCursor, _, _>(&svc.clone(), 33, run_cursor);
    svc.sp
}

pub fn new_pos() -> GenericService {
    let svc = EmptyExtraFieldService::new(NAME_POS.to_owned(), false);
    GenericService::repeat::<StatePos, _, _>(&svc.clone(), 33, run_pos);
    svc.sp
}

pub fn new_window_focus() -> GenericService {
    let svc = EmptyExtraFieldService::new(NAME_WINDOW_FOCUS.to_owned(), false);
    GenericService::repeat::<StateWindowFocus, _, _>(&svc.clone(), 33, run_window_focus);
    svc.sp
}

#[inline]
fn update_last_cursor_pos(x: i32, y: i32) {
    let mut lock = LATEST_SYS_CURSOR_POS.lock().unwrap();
    if lock.1 .0 != x || lock.1 .1 != y {
        (lock.0, lock.1) = (Some(Instant::now()), (x, y))
    }
}

fn run_pos(sp: EmptyExtraFieldService, state: &mut StatePos) -> ResultType<()> {
    let (_, (x, y)) = *LATEST_SYS_CURSOR_POS.lock().unwrap();
    if x == INVALID_CURSOR_POS || y == INVALID_CURSOR_POS {
        return Ok(());
    }

    if state.is_moved(x, y) {
        let mut msg_out = Message::new();
        msg_out.set_cursor_position(CursorPosition {
            x,
            y,
            ..Default::default()
        });
        let exclude = {
            let now = get_time();
            let lock = LATEST_PEER_INPUT_CURSOR.lock().unwrap();
            if now - lock.time < 300 {
                lock.conn
            } else {
                0
            }
        };
        sp.send_without(msg_out, exclude);
    }
    state.cursor_pos = (x, y);

    sp.snapshot(|sps| {
        let mut msg_out = Message::new();
        msg_out.set_cursor_position(CursorPosition {
            x: state.cursor_pos.0,
            y: state.cursor_pos.1,
            ..Default::default()
        });
        sps.send(msg_out);
        Ok(())
    })?;
    Ok(())
}

fn run_cursor(sp: MouseCursorService, state: &mut StateCursor) -> ResultType<()> {
    if let Some(hcursor) = crate::get_cursor()? {
        if hcursor != state.hcursor {
            let msg;
            if let Some(cached) = state.cached_cursor_data.get(&hcursor) {
                super::log::trace!("Cursor data cached, hcursor: {}", hcursor);
                msg = cached.clone();
            } else {
                let mut data = crate::get_cursor_data(hcursor)?;
                data.colors = hbb_common::compress::compress(&data.colors[..]).into();
                let mut tmp = Message::new();
                tmp.set_cursor_data(data);
                msg = Arc::new(tmp);
                state.cached_cursor_data.insert(hcursor, msg.clone());
                super::log::trace!("Cursor data updated, hcursor: {}", hcursor);
            }
            state.hcursor = hcursor;
            sp.send_shared(msg.clone());
            state.cursor_data = msg;
        }
    }
    sp.snapshot(|sps| {
        sps.send_shared(state.cursor_data.clone());
        Ok(())
    })?;
    Ok(())
}

fn run_window_focus(sp: EmptyExtraFieldService, state: &mut StateWindowFocus) -> ResultType<()> {
    let displays = super::display_service::get_sync_displays();
    if displays.len() <= 1 {
        return Ok(());
    }
    let disp_idx = crate::get_focused_display(displays);
    if let Some(disp_idx) = disp_idx.map(|id| id as i32) {
        if state.is_changed(disp_idx) {
            let mut misc = Misc::new();
            misc.set_follow_current_display(disp_idx as i32);
            let mut msg_out = Message::new();
            msg_out.set_misc(misc);
            sp.send(msg_out);
        }
        state.display_idx = disp_idx;
    }
    Ok(())
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum KeysDown {
    RdevKey(RawKey),
    EnigoKey(u64),
}

lazy_static::lazy_static! {
    static ref ENIGO: Arc<Mutex<Enigo>> = {
        Arc::new(Mutex::new(Enigo::new()))
    };
    static ref KEYS_DOWN: Arc<Mutex<HashMap<KeysDown, Instant>>> = Default::default();
    static ref LATEST_PEER_INPUT_CURSOR: Arc<Mutex<Input>> = Default::default();
    static ref LATEST_SYS_CURSOR_POS: Arc<Mutex<(Option<Instant>, (i32, i32))>> = Arc::new(Mutex::new((None, (INVALID_CURSOR_POS, INVALID_CURSOR_POS))));
    // Track connections that are currently using relative mouse movement.
    // Used to disable whiteboard/cursor display for all events while in relative mode.
    static ref RELATIVE_MOUSE_CONNS: Arc<Mutex<std::collections::HashSet<i32>>> = Default::default();
}

#[inline]
fn set_relative_mouse_active(conn: i32, active: bool) {
    let mut lock = RELATIVE_MOUSE_CONNS.lock().unwrap();
    if active {
        lock.insert(conn);
    } else {
        lock.remove(&conn);
    }
}

#[inline]
fn is_relative_mouse_active(conn: i32) -> bool {
    RELATIVE_MOUSE_CONNS.lock().unwrap().contains(&conn)
}

/// Clears the relative mouse mode state for a connection.
///
/// This must be called when an authenticated connection is dropped (during connection teardown)
/// to avoid leaking the connection id in `RELATIVE_MOUSE_CONNS` (a `Mutex<HashSet<i32>>`).
/// Callers are responsible for invoking this on disconnect.
#[inline]
pub(crate) fn clear_relative_mouse_active(conn: i32) {
    set_relative_mouse_active(conn, false);
}

static EXITING: AtomicBool = AtomicBool::new(false);

const MOUSE_MOVE_PROTECTION_TIMEOUT: Duration = Duration::from_millis(1_000);
// Actual diff of (x,y) is (1,1) here. But 5 may be tolerant.
const MOUSE_ACTIVE_DISTANCE: i32 = 5;

static RECORD_CURSOR_POS_RUNNING: AtomicBool = AtomicBool::new(false);

// https://github.com/rustdesk/rustdesk/issues/9729
// We need to do some special handling for macOS when using the legacy mode.
static LAST_KEY_LEGACY_MODE: AtomicBool = AtomicBool::new(true);
// We use enigo to
// 1. Simulate mouse events
// 2. Simulate the legacy mode key events
// 3. Simulate the functioin key events, like LockScreen
#[inline]
fn enigo_ignore_flags() -> bool {
    !LAST_KEY_LEGACY_MODE.load(Ordering::SeqCst)
}
#[inline]
fn set_last_legacy_mode(v: bool) {
    LAST_KEY_LEGACY_MODE.store(v, Ordering::SeqCst);
    ENIGO.lock().unwrap().set_ignore_flags(!v);
}

pub fn try_start_record_cursor_pos() -> Option<thread::JoinHandle<()>> {
    if RECORD_CURSOR_POS_RUNNING.load(Ordering::SeqCst) {
        return None;
    }

    RECORD_CURSOR_POS_RUNNING.store(true, Ordering::SeqCst);
    let handle = thread::spawn(|| {
        let interval = time::Duration::from_millis(33);
        loop {
            if !RECORD_CURSOR_POS_RUNNING.load(Ordering::SeqCst) {
                break;
            }

            let now = time::Instant::now();
            if let Some((x, y)) = crate::get_cursor_pos() {
                update_last_cursor_pos(x, y);
            }
            let elapsed = now.elapsed();
            if elapsed < interval {
                thread::sleep(interval - elapsed);
            }
        }
        update_last_cursor_pos(INVALID_CURSOR_POS, INVALID_CURSOR_POS);
    });
    Some(handle)
}

pub fn try_stop_record_cursor_pos() {
    let remote_count = AUTHED_CONNS
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.conn_type == AuthConnType::Remote)
        .count();
    if remote_count > 0 {
        return;
    }
    RECORD_CURSOR_POS_RUNNING.store(false, Ordering::SeqCst);
}

// mac key input must be run in main thread, otherwise crash on >= osx 10.15
lazy_static::lazy_static! {
    static ref QUEUE: Queue = Queue::main();
}

struct VirtualInputState {
    virtual_input: VirtualInput,
    capslock_down: bool,
}

impl VirtualInputState {
    fn new() -> Option<Self> {
        VirtualInput::new(
            CGEventSourceStateID::CombinedSessionState,
            // Note: `CGEventTapLocation::Session` will be affected by the mouse events.
            // When we're simulating key events, then move the physical mouse, the key events will be affected.
            // It looks like https://github.com/rustdesk/rustdesk/issues/9729#issuecomment-2432306822
            // 1. Press "Command" key in RustDesk
            // 2. Move the physical mouse
            // 3. Press "V" key in RustDesk
            // Then the controlled side just prints "v" instead of pasting.
            //
            // Changing `CGEventTapLocation::Session` to `CGEventTapLocation::HID` fixes it.
            // But we do not consider this as a bug, because it's not a common case,
            // we consider only RustDesk operates the controlled side.
            //
            // https://developer.apple.com/documentation/coregraphics/cgeventtaplocation/
            CGEventTapLocation::Session,
        )
        .map(|virtual_input| Self {
            virtual_input,
            capslock_down: false,
        })
        .ok()
    }

    #[inline]
    fn simulate(&self, event_type: &EventType) -> ResultType<()> {
        Ok(self.virtual_input.simulate(&event_type)?)
    }
}

static mut VIRTUAL_INPUT_MTX: Mutex<()> = Mutex::new(());
static mut VIRTUAL_INPUT_STATE: Option<VirtualInputState> = None;

pub fn is_left_up(evt: &MouseEvent) -> bool {
    let buttons = evt.mask >> 3;
    let evt_type = evt.mask & MOUSE_TYPE_MASK;
    buttons == MOUSE_BUTTON_LEFT && evt_type == MOUSE_TYPE_UP
}

// Sleep for 8ms is enough in my tests, but we sleep 12ms to be safe.
// sleep 12ms In my test, the characters are already output in real time.
#[inline]
fn key_sleep() {
    // https://www.reddit.com/r/rustdesk/comments/1kn1w5x/typing_lags_when_connecting_to_macos_clients/
    //
    // There's a strange bug when running by `launchctl load -w /Library/LaunchAgents/abc.plist`
    // `std::thread::sleep(Duration::from_millis(20));` may sleep 90ms or more.
    // Though `/Applications/RustDesk.app/Contents/MacOS/rustdesk --server` in terminal is ok.
    let now = Instant::now();
    while now.elapsed() < Duration::from_millis(12) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[inline]
fn get_modifier_state(key: Key, en: &mut Enigo) -> bool {
    // https://github.com/rustdesk/rustdesk/issues/332
    // on Linux, if RightAlt is down, RightAlt status is false, Alt status is true
    // but on Windows, both are true
    let x = en.get_key_state(key.clone());
    match key {
        Key::Shift => x || en.get_key_state(Key::RightShift),
        Key::Control => x || en.get_key_state(Key::RightControl),
        Key::Alt => x || en.get_key_state(Key::RightAlt),
        Key::Meta => x || en.get_key_state(Key::RWin),
        Key::RightShift => x || en.get_key_state(Key::Shift),
        Key::RightControl => x || en.get_key_state(Key::Control),
        Key::RightAlt => x || en.get_key_state(Key::Alt),
        Key::RWin => x || en.get_key_state(Key::Meta),
        _ => x,
    }
}

pub fn handle_mouse(
    evt: &MouseEvent,
    conn: i32,
    username: String,
    argb: u32,
    simulate: bool,
    show_cursor: bool,
) {
    // having GUI (--server has tray, it is GUI too), run main GUI thread, otherwise crash
    let evt = evt.clone();
    QUEUE.exec_async(move || handle_mouse_(&evt, conn, username, argb, simulate, show_cursor));
}

// to-do: merge handle_mouse and handle_pointer
pub fn handle_pointer(evt: &PointerDeviceEvent, conn: i32) {
    // having GUI, run main GUI thread, otherwise crash
    let evt = evt.clone();
    QUEUE.exec_async(move || handle_pointer_(&evt, conn));
}

pub fn fix_key_down_timeout_loop() {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(10_000));
        fix_key_down_timeout(false);
    });
    if let Err(err) = ctrlc::set_handler(move || {
        fix_key_down_timeout_at_exit();
        std::process::exit(0); // will call atexit on posix, but not on Windows
    }) {
        log::error!("Failed to set Ctrl-C handler: {}", err);
    }
}

pub fn fix_key_down_timeout_at_exit() {
    if EXITING.load(Ordering::SeqCst) {
        return;
    }
    EXITING.store(true, Ordering::SeqCst);
    fix_key_down_timeout(true);
    log::info!("fix_key_down_timeout_at_exit");
}

#[inline]
fn record_key_is_control_key(record_key: u64) -> bool {
    record_key < KEY_CHAR_START
}

#[inline]
fn record_key_is_chr(record_key: u64) -> bool {
    record_key < KEY_CHAR_START
}

#[inline]
fn record_key_to_key(record_key: u64) -> Option<Key> {
    if record_key_is_control_key(record_key) {
        control_key_value_to_key(record_key as _)
    } else if record_key_is_chr(record_key) {
        let chr: u32 = (record_key - KEY_CHAR_START) as _;
        Some(char_value_to_key(chr))
    } else {
        None
    }
}

pub fn release_device_modifiers() {
    let mut en = ENIGO.lock().unwrap();
    for modifier in [
        Key::Shift,
        Key::Control,
        Key::Alt,
        Key::Meta,
        Key::RightShift,
        Key::RightControl,
        Key::RightAlt,
        Key::RWin,
    ] {
        if get_modifier_state(modifier, &mut en) {
            en.key_up(modifier);
        }
    }
}

#[inline]
fn release_record_key(record_key: KeysDown) {
    let func = move || match record_key {
        KeysDown::RdevKey(raw_key) => {
            simulate_(&EventType::KeyRelease(RdevKey::RawKey(raw_key)));
        }
        KeysDown::EnigoKey(key) => {
            if let Some(key) = record_key_to_key(key) {
                ENIGO.lock().unwrap().key_up(key);
                log::debug!("Fixed {:?} timeout", key);
            }
        }
    };

    QUEUE.exec_async(func);
}

fn fix_key_down_timeout(force: bool) {
    let key_down = KEYS_DOWN.lock().unwrap();
    if key_down.is_empty() {
        return;
    }
    let cloned = (*key_down).clone();
    drop(key_down);

    for (record_key, time) in cloned.into_iter() {
        if force || time.elapsed().as_millis() >= 360_000 {
            record_pressed_key(record_key, false);
            release_record_key(record_key);
        }
    }
}

// e.g. current state of ctrl is down, but ctrl not in modifier, we should change ctrl to up, to make modifier state sync between remote and local
#[inline]
fn fix_modifier(
    modifiers: &[EnumOrUnknown<ControlKey>],
    key0: ControlKey,
    key1: Key,
    en: &mut Enigo,
) {
    if get_modifier_state(key1, en) && !modifiers.contains(&EnumOrUnknown::new(key0)) {
        en.key_up(key1);
        log::debug!("Fixed {:?}", key1);
    }
}

fn fix_modifiers(modifiers: &[EnumOrUnknown<ControlKey>], en: &mut Enigo, ck: i32) {
    if ck != ControlKey::Shift.value() {
        fix_modifier(modifiers, ControlKey::Shift, Key::Shift, en);
    }
    if ck != ControlKey::RShift.value() {
        fix_modifier(modifiers, ControlKey::Shift, Key::RightShift, en);
    }
    if ck != ControlKey::Alt.value() {
        fix_modifier(modifiers, ControlKey::Alt, Key::Alt, en);
    }
    if ck != ControlKey::RAlt.value() {
        fix_modifier(modifiers, ControlKey::Alt, Key::RightAlt, en);
    }
    if ck != ControlKey::Control.value() {
        fix_modifier(modifiers, ControlKey::Control, Key::Control, en);
    }
    if ck != ControlKey::RControl.value() {
        fix_modifier(modifiers, ControlKey::Control, Key::RightControl, en);
    }
    if ck != ControlKey::Meta.value() {
        fix_modifier(modifiers, ControlKey::Meta, Key::Meta, en);
    }
    if ck != ControlKey::RWin.value() {
        fix_modifier(modifiers, ControlKey::Meta, Key::RWin, en);
    }
}

// Update time to avoid send cursor position event to the peer.
// See `run_pos` --> `set_cursor_position` --> `exclude`
#[inline]
pub fn update_latest_input_cursor_time(conn: i32) {
    let mut lock = LATEST_PEER_INPUT_CURSOR.lock().unwrap();
    lock.conn = conn;
    lock.time = get_time();
}

#[inline]
fn get_last_input_cursor_pos() -> (i32, i32) {
    let lock = LATEST_PEER_INPUT_CURSOR.lock().unwrap();
    (lock.x, lock.y)
}

// check if mouse is moved by the controlled side user to make controlled side has higher mouse priority than remote.
fn active_mouse_(_conn: i32) -> bool {
    true
    /* this method is buggy (not working on macOS, making fast moving mouse event discarded here) and added latency (this is blocking way, must do in async way), so we disable it for now
    // out of time protection
    if LATEST_SYS_CURSOR_POS
        .lock()
        .unwrap()
        .0
        .map(|t| t.elapsed() > MOUSE_MOVE_PROTECTION_TIMEOUT)
        .unwrap_or(true)
    {
        return true;
    }

    // last conn input may be protected
    if LATEST_PEER_INPUT_CURSOR.lock().unwrap().conn != conn {
        return false;
    }

    let in_active_dist = |a: i32, b: i32| -> bool { (a - b).abs() < MOUSE_ACTIVE_DISTANCE };

    // Check if input is in valid range
    match crate::get_cursor_pos() {
        Some((x, y)) => {
            let (last_in_x, last_in_y) = get_last_input_cursor_pos();
            let mut can_active = in_active_dist(last_in_x, x) && in_active_dist(last_in_y, y);
            // The cursor may not have been moved to last input position if system is busy now.
            // While this is not a common case, we check it again after some time later.
            if !can_active {
                // 100 micros may be enough for system to move cursor.
                // Mouse inputs on macOS are asynchronous. 1. Put in a queue to process in main thread. 2. Send event async.
                // More reties are needed on macOS.
                let retries = 100;
                let sleep_interval: u64 = 30;
                for _retry in 0..retries {
                    std::thread::sleep(std::time::Duration::from_micros(sleep_interval));
                    // Sleep here can also somehow suppress delay accumulation.
                    if let Some((x2, y2)) = crate::get_cursor_pos() {
                        let (last_in_x, last_in_y) = get_last_input_cursor_pos();
                        can_active = in_active_dist(last_in_x, x2) && in_active_dist(last_in_y, y2);
                        if can_active {
                            break;
                        }
                    }
                }
            }
            if !can_active {
                let mut lock = LATEST_PEER_INPUT_CURSOR.lock().unwrap();
                lock.x = INVALID_CURSOR_POS / 2;
                lock.y = INVALID_CURSOR_POS / 2;
            }
            can_active
        }
        None => true,
    }
    */
}

pub fn handle_pointer_(evt: &PointerDeviceEvent, conn: i32) {
    if !active_mouse_(conn) {
        return;
    }

    if EXITING.load(Ordering::SeqCst) {
        return;
    }

    match &evt.union {
        Some(TouchEvent(_evt)) => {}
        _ => {}
    }
}

pub fn handle_mouse_(
    evt: &MouseEvent,
    conn: i32,
    _username: String,
    _argb: u32,
    simulate: bool,
    _show_cursor: bool,
) {
    if simulate {
        handle_mouse_simulation_(evt, conn);
    }
    {
        let evt_type = evt.mask & MOUSE_TYPE_MASK;
        // Relative (delta) mouse events do not include absolute coordinates, so
        // whiteboard/cursor rendering must be disabled during relative mode to prevent
        // incorrect cursor/whiteboard updates. We check both is_relative_mouse_active(conn)
        // (connection already in relative mode from prior events) and evt_type (current
        // event is relative) to guard against the first relative event before the flag is set.
        if _show_cursor && !is_relative_mouse_active(conn) && evt_type != MOUSE_TYPE_MOVE_RELATIVE {
            handle_mouse_show_cursor_(evt, conn, _username, _argb);
        }
    }
}

pub fn handle_mouse_simulation_(evt: &MouseEvent, conn: i32) {
    if !active_mouse_(conn) {
        return;
    }

    if EXITING.load(Ordering::SeqCst) {
        return;
    }

    let buttons = evt.mask >> 3;
    let evt_type = evt.mask & MOUSE_TYPE_MASK;
    let mut en = ENIGO.lock().unwrap();
    en.set_ignore_flags(enigo_ignore_flags());
    if evt_type == MOUSE_TYPE_DOWN {
        fix_modifiers(&evt.modifiers[..], &mut en, 0);
        en.reset_flag();
        for ref ck in evt.modifiers.iter() {
            if let Some(key) = KEY_MAP.get(&ck.value()) {
                en.add_flag(key);
            }
        }
    }
    match evt_type {
        MOUSE_TYPE_MOVE => {
            // Switching back to absolute movement implicitly disables relative mouse mode.
            set_relative_mouse_active(conn, false);
            en.mouse_move_to(evt.x, evt.y);
            *LATEST_PEER_INPUT_CURSOR.lock().unwrap() = Input {
                conn,
                time: get_time(),
                x: evt.x,
                y: evt.y,
            };
        }
        // MOUSE_TYPE_MOVE_RELATIVE: Relative mouse movement for gaming/3D applications.
        // Each client independently decides whether to use relative mode.
        // Multiple clients can mix absolute and relative movements without conflict,
        // as the server simply applies the delta to the current cursor position.
        MOUSE_TYPE_MOVE_RELATIVE => {
            set_relative_mouse_active(conn, true);
            // Clamp delta to prevent extreme/malicious values from reaching OS APIs.
            // This matches the Flutter client's kMaxRelativeMouseDelta constant.
            const MAX_RELATIVE_MOUSE_DELTA: i32 = 10000;
            let dx = evt.x.clamp(-MAX_RELATIVE_MOUSE_DELTA, MAX_RELATIVE_MOUSE_DELTA);
            let dy = evt.y.clamp(-MAX_RELATIVE_MOUSE_DELTA, MAX_RELATIVE_MOUSE_DELTA);
            en.mouse_move_relative(dx, dy);
            // Get actual cursor position after relative movement for tracking
            if let Some((x, y)) = crate::get_cursor_pos() {
                *LATEST_PEER_INPUT_CURSOR.lock().unwrap() = Input {
                    conn,
                    time: get_time(),
                    x,
                    y,
                };
            }
        }
        MOUSE_TYPE_DOWN => match buttons {
            MOUSE_BUTTON_LEFT => {
                allow_err!(en.mouse_down(MouseButton::Left));
            }
            MOUSE_BUTTON_RIGHT => {
                allow_err!(en.mouse_down(MouseButton::Right));
            }
            MOUSE_BUTTON_WHEEL => {
                allow_err!(en.mouse_down(MouseButton::Middle));
            }
            MOUSE_BUTTON_BACK => {
                allow_err!(en.mouse_down(MouseButton::Back));
            }
            MOUSE_BUTTON_FORWARD => {
                allow_err!(en.mouse_down(MouseButton::Forward));
            }
            _ => {}
        },
        MOUSE_TYPE_UP => match buttons {
            MOUSE_BUTTON_LEFT => {
                en.mouse_up(MouseButton::Left);
            }
            MOUSE_BUTTON_RIGHT => {
                en.mouse_up(MouseButton::Right);
            }
            MOUSE_BUTTON_WHEEL => {
                en.mouse_up(MouseButton::Middle);
            }
            MOUSE_BUTTON_BACK => {
                en.mouse_up(MouseButton::Back);
            }
            MOUSE_BUTTON_FORWARD => {
                en.mouse_up(MouseButton::Forward);
            }
            _ => {}
        },
        MOUSE_TYPE_WHEEL | MOUSE_TYPE_TRACKPAD => {
            let mut x = -evt.x;
            let mut y = -evt.y;

            let is_track_pad = evt_type == MOUSE_TYPE_TRACKPAD;

            // fix shift + scroll(down/up)
            if !is_track_pad
                && evt
                    .modifiers
                    .contains(&EnumOrUnknown::new(ControlKey::Shift))
            {
                x = y;
                y = 0;
            }

            if x != 0 {
                en.mouse_scroll_x(x, is_track_pad);
            }
            if y != 0 {
                en.mouse_scroll_y(y, is_track_pad);
            }
        }
        _ => {}
    }
}

pub fn handle_mouse_show_cursor_(evt: &MouseEvent, conn: i32, username: String, argb: u32) {
    let buttons = evt.mask >> 3;
    let evt_type = evt.mask & MOUSE_TYPE_MASK;
    match evt_type {
        MOUSE_TYPE_MOVE => {
            whiteboard::update_whiteboard(
                whiteboard::get_key_cursor(conn),
                whiteboard::CustomEvent::Cursor(whiteboard::Cursor {
                    x: evt.x as _,
                    y: evt.y as _,
                    argb,
                    btns: 0,
                    text: username,
                }),
            );
        }
        MOUSE_TYPE_UP => {
            if buttons == MOUSE_BUTTON_LEFT {
                // Some clients intentionally send button events without coordinates.
                // Fall back to the last known cursor position to avoid jumping to (0, 0).
                // TODO(protocol): (0, 0) is a valid screen coordinate. Consider using a dedicated
                // sentinel value (e.g. INVALID_CURSOR_POS) or a protocol-level flag to distinguish
                // "coordinates not provided" from "coordinates are (0, 0)". Impact is minor since
                // this only affects whiteboard rendering and clicking exactly at (0, 0) is rare.
                let (x, y) = if evt.x == 0 && evt.y == 0 {
                    get_last_input_cursor_pos()
                } else {
                    (evt.x, evt.y)
                };
                whiteboard::update_whiteboard(
                    whiteboard::get_key_cursor(conn),
                    whiteboard::CustomEvent::Cursor(whiteboard::Cursor {
                        x: x as _,
                        y: y as _,
                        argb,
                        btns: buttons,
                        text: username,
                    }),
                );
            }
        }
        _ => {}
    }
}

pub fn is_enter(evt: &KeyEvent) -> bool {
    if let Some(key_event::Union::ControlKey(ck)) = evt.union {
        if ck.value() == ControlKey::Return.value() || ck.value() == ControlKey::NumpadEnter.value()
        {
            return true;
        }
    }
    return false;
}

pub async fn lock_screen() {
    // CGSession -suspend not real lock screen, it is user switch
    std::thread::spawn(|| {
        let mut key_event = KeyEvent::new();

        key_event.set_chr('q' as _);
        key_event.modifiers.push(ControlKey::Meta.into());
        key_event.modifiers.push(ControlKey::Control.into());
        key_event.mode = KeyboardMode::Legacy.into();

        key_event.down = true;
        handle_key(&key_event);
        key_event.down = false;
        handle_key(&key_event);
    });
}

#[inline]
pub fn handle_key(evt: &KeyEvent) {
    // having GUI, run main GUI thread, otherwise crash
    let evt = evt.clone();
    QUEUE.exec_async(move || handle_key_(&evt));
    // Key sleep is required for macOS.
    // If we don't sleep, the key press/release events may not take effect.
    //
    // For example, the controlled side osx `12.7.6` or `15.1.1`
    // If we input characters quickly and continuously, and press or release "Shift" for a short period of time,
    // it is possible that after releasing "Shift", the controlled side will still print uppercase characters.
    // Though it is not very easy to reproduce.
    key_sleep();
}

#[inline]
fn reset_input() {
    unsafe {
        let _lock = VIRTUAL_INPUT_MTX.lock();
        VIRTUAL_INPUT_STATE = VirtualInputState::new();
    }
}

pub fn reset_input_ondisconn() {
    QUEUE.exec_async(reset_input);
}

fn sim_rdev_rawkey_position(code: KeyCode, keydown: bool) {
    let rawkey = RawKey::MacVirtualKeycode(code);

    // map mode(1): Send keycode according to the peer platform.
    record_pressed_key(KeysDown::RdevKey(rawkey), keydown);

    let event_type = if keydown {
        EventType::KeyPress(RdevKey::RawKey(rawkey))
    } else {
        EventType::KeyRelease(RdevKey::RawKey(rawkey))
    };
    simulate_(&event_type);
}

#[inline]
fn simulate_(event_type: &EventType) {
    unsafe {
        let _lock = VIRTUAL_INPUT_MTX.lock();
        if let Some(input) = VIRTUAL_INPUT_STATE.as_ref() {
            let _ = input.simulate(&event_type);
        }
    }
}

#[inline]
fn press_capslock() {
    let caps_key = RdevKey::RawKey(rdev::RawKey::MacVirtualKeycode(rdev::kVK_CapsLock));
    unsafe {
        let _lock = VIRTUAL_INPUT_MTX.lock();
        if let Some(input) = VIRTUAL_INPUT_STATE.as_mut() {
            if input.simulate(&EventType::KeyPress(caps_key)).is_ok() {
                input.capslock_down = true;
                key_sleep();
            }
        }
    }
}

#[inline]
fn release_capslock() {
    let caps_key = RdevKey::RawKey(rdev::RawKey::MacVirtualKeycode(rdev::kVK_CapsLock));
    unsafe {
        let _lock = VIRTUAL_INPUT_MTX.lock();
        if let Some(input) = VIRTUAL_INPUT_STATE.as_mut() {
            if input.simulate(&EventType::KeyRelease(caps_key)).is_ok() {
                input.capslock_down = false;
                key_sleep();
            }
        }
    }
}

#[inline]
fn control_key_value_to_key(value: i32) -> Option<Key> {
    KEY_MAP.get(&value).and_then(|k| Some(*k))
}

#[inline]
fn char_value_to_key(value: u32) -> Key {
    Key::Layout(std::char::from_u32(value).unwrap_or('\0'))
}

fn map_keyboard_mode(evt: &KeyEvent) {
    sim_rdev_rawkey_position(evt.chr() as _, evt.down);
}

fn add_flags_to_enigo(en: &mut Enigo, key_event: &KeyEvent) {
    // When long-pressed the command key, then press and release
    // the Tab key, there should be CGEventFlagCommand in the flag.
    en.reset_flag();
    for ck in key_event.modifiers.iter() {
        if let Some(key) = KEY_MAP.get(&ck.value()) {
            en.add_flag(key);
        }
    }
}

fn get_control_key_value(key_event: &KeyEvent) -> i32 {
    if let Some(key_event::Union::ControlKey(ck)) = key_event.union {
        ck.value()
    } else {
        -1
    }
}

fn release_unpressed_modifiers(en: &mut Enigo, key_event: &KeyEvent) {
    let ck_value = get_control_key_value(key_event);
    fix_modifiers(&key_event.modifiers[..], en, ck_value);
}

fn sync_modifiers(en: &mut Enigo, key_event: &KeyEvent, _to_release: &mut Vec<Key>) {
    add_flags_to_enigo(en, key_event);

    if key_event.down {
        release_unpressed_modifiers(en, key_event);
    }
}

fn process_control_key(en: &mut Enigo, ck: &EnumOrUnknown<ControlKey>, down: bool) {
    if let Some(key) = control_key_value_to_key(ck.value()) {
        if down {
            en.key_down(key).ok();
        } else {
            en.key_up(key);
        }
    }
}

#[inline]
fn need_to_uppercase(en: &mut Enigo) -> bool {
    get_modifier_state(Key::Shift, en) || get_modifier_state(Key::CapsLock, en)
}

fn process_chr(en: &mut Enigo, chr: u32, down: bool) {
    let key = char_value_to_key(chr);

    if down {
        if en.key_down(key).is_ok() {
        } else {
            if let Ok(chr) = char::try_from(chr) {
                let mut s = chr.to_string();
                if need_to_uppercase(en) {
                    s = s.to_uppercase();
                }
                en.key_sequence(&s);
            };
        }
    } else {
        en.key_up(key);
    }
}

fn process_unicode(en: &mut Enigo, chr: u32) {
    if let Ok(chr) = char::try_from(chr) {
        en.key_sequence(&chr.to_string());
    }
}

fn process_seq(en: &mut Enigo, sequence: &str) {
    en.key_sequence(&sequence);
}

fn record_pressed_key(record_key: KeysDown, down: bool) {
    let mut key_down = KEYS_DOWN.lock().unwrap();
    if down {
        key_down.insert(record_key, Instant::now());
    } else {
        key_down.remove(&record_key);
    }
}

fn is_function_key(ck: &EnumOrUnknown<ControlKey>) -> bool {
    let mut res = false;
    if ck.value() == ControlKey::CtrlAltDel.value() {
        res = true;
    } else if ck.value() == ControlKey::LockScreen.value() {
        std::thread::spawn(|| {
            lock_screen_2();
        });
        res = true;
    }
    return res;
}

fn legacy_keyboard_mode(evt: &KeyEvent) {
    let mut to_release: Vec<Key> = Vec::new();

    let mut en = ENIGO.lock().unwrap();
    sync_modifiers(&mut en, &evt, &mut to_release);

    let down = evt.down;
    match evt.union {
        Some(key_event::Union::ControlKey(ck)) => {
            if is_function_key(&ck) {
                return;
            }
            let record_key = ck.value() as u64;
            record_pressed_key(KeysDown::EnigoKey(record_key), down);
            process_control_key(&mut en, &ck, down)
        }
        Some(key_event::Union::Chr(chr)) => {
            let record_key = chr as u64 + KEY_CHAR_START;
            record_pressed_key(KeysDown::EnigoKey(record_key), down);
            process_chr(&mut en, chr, down)
        }
        Some(key_event::Union::Unicode(chr)) => process_unicode(&mut en, chr),
        Some(key_event::Union::Seq(ref seq)) => process_seq(&mut en, seq),
        _ => {}
    }

}

fn translate_keyboard_mode(evt: &KeyEvent) {
    match &evt.union {
        Some(key_event::Union::Seq(seq)) => {
            let mut en = ENIGO.lock().unwrap();
            en.key_sequence(seq);
        }
        Some(key_event::Union::Chr(..)) => {
            sim_rdev_rawkey_position(evt.chr() as _, evt.down);
        }
        Some(key_event::Union::Unicode(..)) => {
            // Do not handle unicode for now.
        }
        _ => {
            log::debug!("Unreachable. Unexpected key event {:?}", &evt);
        }
    }
}

fn skip_led_sync_control_key(_key: &ControlKey) -> bool {
    false
}

fn skip_led_sync_rdev_key(_key: &RdevKey) -> bool {
    false
}

#[inline]
fn is_legacy_mode(evt: &KeyEvent) -> bool {
    evt.mode.enum_value_or(KeyboardMode::Legacy) == KeyboardMode::Legacy
}

pub fn handle_key_(evt: &KeyEvent) {
    if EXITING.load(Ordering::SeqCst) {
        return;
    }

    let mut _lock_mode_handler = None;
    match &evt.union {
        Some(key_event::Union::Unicode(..)) | Some(key_event::Union::Seq(..)) => {
            _lock_mode_handler = Some(LockModesHandler::new_handler(&evt, false));
        }
        Some(key_event::Union::ControlKey(ck)) => {
            let key = ck.enum_value_or(ControlKey::Unknown);
            if !skip_led_sync_control_key(&key) {
                let is_numpad_key = false;
                _lock_mode_handler = Some(LockModesHandler::new_handler(&evt, is_numpad_key));
            }
        }
        Some(key_event::Union::Chr(code)) => {
            if is_legacy_mode(&evt) {
                _lock_mode_handler = Some(LockModesHandler::new_handler(evt, false));
            } else {
                let key = crate::keyboard::keycode_to_rdev_key(*code);
                if !skip_led_sync_rdev_key(&key) {
                    let is_numpad_key = false;
                    _lock_mode_handler = Some(LockModesHandler::new_handler(evt, is_numpad_key));
                }
            }
        }
        _ => {}
    }

    match evt.mode.enum_value() {
        Ok(KeyboardMode::Map) => {
            set_last_legacy_mode(false);
            map_keyboard_mode(evt);
        }
        Ok(KeyboardMode::Translate) => {
            set_last_legacy_mode(false);
            translate_keyboard_mode(evt);
        }
        _ => {
            set_last_legacy_mode(true);
            legacy_keyboard_mode(evt);
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn lock_screen_2() {
    lock_screen().await;
}

lazy_static::lazy_static! {
    static ref MODIFIER_MAP: HashMap<i32, Key> = [
        (ControlKey::Alt, Key::Alt),
        (ControlKey::RAlt, Key::RightAlt),
        (ControlKey::Control, Key::Control),
        (ControlKey::RControl, Key::RightControl),
        (ControlKey::Shift, Key::Shift),
        (ControlKey::RShift, Key::RightShift),
        (ControlKey::Meta, Key::Meta),
        (ControlKey::RWin, Key::RWin),
    ].iter().map(|(a, b)| (a.value(), b.clone())).collect();
    static ref KEY_MAP: HashMap<i32, Key> =
    [
        (ControlKey::Alt, Key::Alt),
        (ControlKey::Backspace, Key::Backspace),
        (ControlKey::CapsLock, Key::CapsLock),
        (ControlKey::Control, Key::Control),
        (ControlKey::Delete, Key::Delete),
        (ControlKey::DownArrow, Key::DownArrow),
        (ControlKey::End, Key::End),
        (ControlKey::Escape, Key::Escape),
        (ControlKey::F1, Key::F1),
        (ControlKey::F10, Key::F10),
        (ControlKey::F11, Key::F11),
        (ControlKey::F12, Key::F12),
        (ControlKey::F2, Key::F2),
        (ControlKey::F3, Key::F3),
        (ControlKey::F4, Key::F4),
        (ControlKey::F5, Key::F5),
        (ControlKey::F6, Key::F6),
        (ControlKey::F7, Key::F7),
        (ControlKey::F8, Key::F8),
        (ControlKey::F9, Key::F9),
        (ControlKey::Home, Key::Home),
        (ControlKey::LeftArrow, Key::LeftArrow),
        (ControlKey::Meta, Key::Meta),
        (ControlKey::Option, Key::Option),
        (ControlKey::PageDown, Key::PageDown),
        (ControlKey::PageUp, Key::PageUp),
        (ControlKey::Return, Key::Return),
        (ControlKey::RightArrow, Key::RightArrow),
        (ControlKey::Shift, Key::Shift),
        (ControlKey::Space, Key::Space),
        (ControlKey::Tab, Key::Tab),
        (ControlKey::UpArrow, Key::UpArrow),
        (ControlKey::Numpad0, Key::Numpad0),
        (ControlKey::Numpad1, Key::Numpad1),
        (ControlKey::Numpad2, Key::Numpad2),
        (ControlKey::Numpad3, Key::Numpad3),
        (ControlKey::Numpad4, Key::Numpad4),
        (ControlKey::Numpad5, Key::Numpad5),
        (ControlKey::Numpad6, Key::Numpad6),
        (ControlKey::Numpad7, Key::Numpad7),
        (ControlKey::Numpad8, Key::Numpad8),
        (ControlKey::Numpad9, Key::Numpad9),
        (ControlKey::Cancel, Key::Cancel),
        (ControlKey::Clear, Key::Clear),
        (ControlKey::Menu, Key::Alt),
        (ControlKey::Pause, Key::Pause),
        (ControlKey::Kana, Key::Kana),
        (ControlKey::Hangul, Key::Hangul),
        (ControlKey::Junja, Key::Junja),
        (ControlKey::Final, Key::Final),
        (ControlKey::Hanja, Key::Hanja),
        (ControlKey::Kanji, Key::Kanji),
        (ControlKey::Convert, Key::Convert),
        (ControlKey::Select, Key::Select),
        (ControlKey::Print, Key::Print),
        (ControlKey::Execute, Key::Execute),
        (ControlKey::Snapshot, Key::Snapshot),
        (ControlKey::Insert, Key::Insert),
        (ControlKey::Help, Key::Help),
        (ControlKey::Sleep, Key::Sleep),
        (ControlKey::Separator, Key::Separator),
        (ControlKey::Scroll, Key::Scroll),
        (ControlKey::NumLock, Key::NumLock),
        (ControlKey::RWin, Key::RWin),
        (ControlKey::Apps, Key::Apps),
        (ControlKey::Multiply, Key::Multiply),
        (ControlKey::Add, Key::Add),
        (ControlKey::Subtract, Key::Subtract),
        (ControlKey::Decimal, Key::Decimal),
        (ControlKey::Divide, Key::Divide),
        (ControlKey::Equals, Key::Equals),
        (ControlKey::NumpadEnter, Key::NumpadEnter),
        (ControlKey::RAlt, Key::RightAlt),
        (ControlKey::RControl, Key::RightControl),
        (ControlKey::RShift, Key::RightShift),
    ].iter().map(|(a, b)| (a.value(), b.clone())).collect();
    static ref NUMPAD_KEY_MAP: HashMap<i32, bool> =
    [
        (ControlKey::Home, true),
        (ControlKey::UpArrow, true),
        (ControlKey::PageUp, true),
        (ControlKey::LeftArrow, true),
        (ControlKey::RightArrow, true),
        (ControlKey::End, true),
        (ControlKey::DownArrow, true),
        (ControlKey::PageDown, true),
        (ControlKey::Insert, true),
        (ControlKey::Delete, true),
    ].iter().map(|(a, b)| (a.value(), b.clone())).collect();
}
