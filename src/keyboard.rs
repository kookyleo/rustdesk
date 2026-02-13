#[cfg(feature = "flutter")]
use crate::flutter;
#[cfg(not(any(feature = "flutter", feature = "cli")))]
use crate::ui::CUR_SESSION;
use crate::ui_session_interface::{InvokeUiSession, Session};
use crate::{client::get_key_state, common::GrabState};
use hbb_common::log;
use hbb_common::message_proto::*;
use rdev::KeyCode;
use rdev::{Event, EventType, Key};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[allow(dead_code)]
const OS_LOWER_WINDOWS: &str = "windows";
#[allow(dead_code)]
const OS_LOWER_LINUX: &str = "linux";
#[allow(dead_code)]
const OS_LOWER_MACOS: &str = "macos";
#[allow(dead_code)]
const OS_LOWER_ANDROID: &str = "android";

static KEYBOARD_HOOKED: AtomicBool = AtomicBool::new(false);

// Track key down state for relative mouse mode exit shortcut.
// macOS: Cmd+G (track G key)
// This prevents the exit from retriggering on OS key-repeat.
#[cfg(feature = "flutter")]
static EXIT_SHORTCUT_KEY_DOWN: AtomicBool = AtomicBool::new(false);

// Track whether relative mouse mode is currently active.
// This is set by Flutter via set_relative_mouse_mode_state() and checked
// by the rdev grab loop to determine if exit shortcuts should be processed.
#[cfg(feature = "flutter")]
static RELATIVE_MOUSE_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the relative mouse mode state from Flutter.
/// This is called when entering or exiting relative mouse mode.
#[cfg(feature = "flutter")]
pub fn set_relative_mouse_mode_state(active: bool) {
    RELATIVE_MOUSE_MODE_ACTIVE.store(active, Ordering::SeqCst);
    // Reset exit shortcut state when mode changes to avoid stale state
    if !active {
        EXIT_SHORTCUT_KEY_DOWN.store(false, Ordering::SeqCst);
    }
}

#[cfg(feature = "flutter")]
static IS_RDEV_ENABLED: AtomicBool = AtomicBool::new(false);

lazy_static::lazy_static! {
    static ref TO_RELEASE: Arc<Mutex<HashMap<Key, Event>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref MODIFIERS_STATE: Mutex<HashMap<Key, bool>> = {
        let mut m = HashMap::new();
        m.insert(Key::ShiftLeft, false);
        m.insert(Key::ShiftRight, false);
        m.insert(Key::ControlLeft, false);
        m.insert(Key::ControlRight, false);
        m.insert(Key::Alt, false);
        m.insert(Key::AltGr, false);
        m.insert(Key::MetaLeft, false);
        m.insert(Key::MetaRight, false);
        Mutex::new(m)
    };
}

pub mod client {
    use super::*;

    lazy_static::lazy_static! {
        static ref IS_GRAB_STARTED: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    }

    pub fn start_grab_loop() {
        let mut lock = IS_GRAB_STARTED.lock().unwrap();
        if *lock {
            return;
        }
        super::start_grab_loop();
        *lock = true;
    }

    pub fn change_grab_status(state: GrabState, keyboard_mode: &str) {
        #[cfg(feature = "flutter")]
        if !IS_RDEV_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        match state {
            GrabState::Ready => {}
            GrabState::Run => {
                KEYBOARD_HOOKED.swap(true, Ordering::SeqCst);
            }
            GrabState::Wait => {
                release_remote_keys(keyboard_mode);

                KEYBOARD_HOOKED.swap(false, Ordering::SeqCst);
            }
            GrabState::Exit => {}
        }
    }

    pub fn process_event(keyboard_mode: &str, event: &Event, lock_modes: Option<i32>) {
        let keyboard_mode = get_keyboard_mode_enum(keyboard_mode);
        if is_long_press(&event) {
            return;
        }
        let peer = get_peer_platform().to_lowercase();
        for key_event in event_to_key_events(peer, &event, keyboard_mode, lock_modes) {
            send_key_event(&key_event);
        }
    }

    pub fn process_event_with_session<T: InvokeUiSession>(
        keyboard_mode: &str,
        event: &Event,
        lock_modes: Option<i32>,
        session: &Session<T>,
    ) {
        let keyboard_mode = get_keyboard_mode_enum(keyboard_mode);
        if is_long_press(&event) {
            return;
        }
        let peer = session.peer_platform().to_lowercase();
        for key_event in event_to_key_events(peer, &event, keyboard_mode, lock_modes) {
            session.send_key_event(&key_event);
        }
    }

    pub fn get_modifiers_state(
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    ) -> (bool, bool, bool, bool) {
        let modifiers_lock = MODIFIERS_STATE.lock().unwrap();
        let ctrl = *modifiers_lock.get(&Key::ControlLeft).unwrap()
            || *modifiers_lock.get(&Key::ControlRight).unwrap()
            || ctrl;
        let shift = *modifiers_lock.get(&Key::ShiftLeft).unwrap()
            || *modifiers_lock.get(&Key::ShiftRight).unwrap()
            || shift;
        let command = *modifiers_lock.get(&Key::MetaLeft).unwrap()
            || *modifiers_lock.get(&Key::MetaRight).unwrap()
            || command;
        let alt = *modifiers_lock.get(&Key::Alt).unwrap()
            || *modifiers_lock.get(&Key::AltGr).unwrap()
            || alt;

        (alt, ctrl, shift, command)
    }

    pub fn legacy_modifiers(
        key_event: &mut KeyEvent,
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    ) {
        if alt
            && !crate::is_control_key(&key_event, &ControlKey::Alt)
            && !crate::is_control_key(&key_event, &ControlKey::RAlt)
        {
            key_event.modifiers.push(ControlKey::Alt.into());
        }
        if shift
            && !crate::is_control_key(&key_event, &ControlKey::Shift)
            && !crate::is_control_key(&key_event, &ControlKey::RShift)
        {
            key_event.modifiers.push(ControlKey::Shift.into());
        }
        if ctrl
            && !crate::is_control_key(&key_event, &ControlKey::Control)
            && !crate::is_control_key(&key_event, &ControlKey::RControl)
        {
            key_event.modifiers.push(ControlKey::Control.into());
        }
        if command
            && !crate::is_control_key(&key_event, &ControlKey::Meta)
            && !crate::is_control_key(&key_event, &ControlKey::RWin)
        {
            key_event.modifiers.push(ControlKey::Meta.into());
        }
    }

    pub fn event_lock_screen() -> KeyEvent {
        let mut key_event = KeyEvent::new();
        key_event.set_control_key(ControlKey::LockScreen);
        key_event.down = true;
        key_event.mode = KeyboardMode::Legacy.into();
        key_event
    }

    #[inline]
    pub fn lock_screen() {
        send_key_event(&event_lock_screen());
    }

    pub fn event_ctrl_alt_del() -> KeyEvent {
        let mut key_event = KeyEvent::new();
        if get_peer_platform() == "Windows" {
            key_event.set_control_key(ControlKey::CtrlAltDel);
            key_event.down = true;
        } else {
            key_event.set_control_key(ControlKey::Delete);
            legacy_modifiers(&mut key_event, true, true, false, false);
            key_event.press = true;
        }
        key_event.mode = KeyboardMode::Legacy.into();
        key_event
    }

    #[inline]
    pub fn ctrl_alt_del() {
        send_key_event(&event_ctrl_alt_del());
    }
}

static mut IS_LEFT_OPTION_DOWN: bool = false;

fn get_keyboard_mode() -> String {
    #[cfg(not(any(feature = "flutter", feature = "cli")))]
    if let Some(session) = CUR_SESSION.lock().unwrap().as_ref() {
        return session.get_keyboard_mode();
    }
    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        return session.get_keyboard_mode();
    }
    "legacy".to_string()
}

/// Check if exit shortcut for relative mouse mode is active.
/// Exit shortcuts (only exits, not toggles):
/// - macOS: Cmd+G
/// Note: This shortcut is only available in Flutter client. Sciter client does not support relative mouse mode.
#[cfg(feature = "flutter")]
fn is_exit_relative_mouse_shortcut(key: Key) -> bool {
    let modifiers = MODIFIERS_STATE.lock().unwrap();

    // macOS: Cmd+G to exit
    if key != Key::KeyG {
        return false;
    }
    let meta = *modifiers.get(&Key::MetaLeft).unwrap_or(&false)
        || *modifiers.get(&Key::MetaRight).unwrap_or(&false);
    return meta;
}

/// Notify Flutter to exit relative mouse mode.
/// Note: This is Flutter-only. Sciter client does not support relative mouse mode.
#[cfg(feature = "flutter")]
fn notify_exit_relative_mouse_mode() {
    let session_id = flutter::get_cur_session_id();
    flutter::push_session_event(&session_id, "exit_relative_mouse_mode", vec![]);
}


/// Handle relative mouse mode shortcuts in the rdev grab loop.
/// Returns true if the event should be blocked from being sent to the peer.
#[cfg(feature = "flutter")]
#[inline]
fn can_exit_relative_mouse_mode_from_grab_loop() -> bool {
    // Only process exit shortcuts when relative mouse mode is actually active.
    // This prevents blocking Cmd+G when not in relative mouse mode.
    if !RELATIVE_MOUSE_MODE_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }

    let Some(session) = flutter::get_cur_session() else {
        return false;
    };

    // Only for remote desktop sessions.
    if !session.is_default() {
        return false;
    }

    // Must have keyboard permission and not be in view-only mode.
    if !*session.server_keyboard_enabled.read().unwrap() {
        return false;
    }
    let lc = session.lc.read().unwrap();
    if lc.view_only.v {
        return false;
    }

    // Peer must support relative mouse mode.
    crate::common::is_support_relative_mouse_mode_num(lc.version)
}

#[cfg(feature = "flutter")]
#[inline]
fn should_block_relative_mouse_shortcut(key: Key, is_press: bool) -> bool {
    if !KEYBOARD_HOOKED.load(Ordering::SeqCst) {
        return false;
    }

    // Determine which key to track for key-up blocking based on platform
    let is_tracked_key = key == Key::KeyG;

    // Block key up if key down was blocked (to avoid orphan key up event on remote).
    // This must be checked before clearing the flag below.
    if is_tracked_key && !is_press && EXIT_SHORTCUT_KEY_DOWN.swap(false, Ordering::SeqCst) {
        return true;
    }

    // Exit relative mouse mode shortcuts:
    // - macOS: Cmd+G
    // Guard it to supported/eligible sessions to avoid blocking the chord unexpectedly.
    if is_exit_relative_mouse_shortcut(key) {
        if !can_exit_relative_mouse_mode_from_grab_loop() {
            return false;
        }
        if is_press {
            // Only trigger exit on transition from "not pressed" to "pressed".
            // This prevents retriggering on OS key-repeat.
            if !EXIT_SHORTCUT_KEY_DOWN.swap(true, Ordering::SeqCst) {
                notify_exit_relative_mouse_mode();
            }
        }
        return true;
    }

    false
}

fn start_grab_loop() {
    std::env::set_var("KEYBOARD_ONLY", "y");
    std::thread::spawn(move || {
        let try_handle_keyboard = move |event: Event, key: Key, is_press: bool| -> Option<Event> {
            // fix #2211: CAPS LOCK don't work
            if key == Key::CapsLock || key == Key::NumLock {
                return Some(event);
            }

            let _scan_code = event.position_code;
            let _code = event.platform_code as KeyCode;

            #[cfg(feature = "flutter")]
            if should_block_relative_mouse_shortcut(key, is_press) {
                return None;
            }

            let res = if KEYBOARD_HOOKED.load(Ordering::SeqCst) {
                client::process_event(&get_keyboard_mode(), &event, None);
                if is_press {
                    None
                } else {
                    Some(event)
                }
            } else {
                Some(event)
            };

            unsafe {
                if _code == rdev::kVK_Option {
                    IS_LEFT_OPTION_DOWN = is_press;
                }
            }

            return res;
        };
        let func = move |event: Event| match event.event_type {
            EventType::KeyPress(key) => try_handle_keyboard(event, key, true),
            EventType::KeyRelease(key) => try_handle_keyboard(event, key, false),
            _ => Some(event),
        };
        rdev::set_is_main_thread(false);
        if let Err(error) = rdev::grab(func) {
            log::error!("rdev Error: {:?}", error)
        }
    });
}

// #[allow(dead_code)] is ok here. No need to stop grabbing loop.
#[allow(dead_code)]
fn stop_grab_loop() -> Result<(), rdev::GrabError> {
    rdev::exit_grab()?;
    Ok(())
}

pub fn is_long_press(event: &Event) -> bool {
    let keys = MODIFIERS_STATE.lock().unwrap();
    match event.event_type {
        EventType::KeyPress(k) => {
            if let Some(&state) = keys.get(&k) {
                if state == true {
                    return true;
                }
            }
        }
        _ => {}
    };
    return false;
}

pub fn release_remote_keys(keyboard_mode: &str) {
    // todo!: client quit suddenly, how to release keys?
    let to_release = TO_RELEASE.lock().unwrap().clone();
    TO_RELEASE.lock().unwrap().clear();
    for (key, mut event) in to_release.into_iter() {
        event.event_type = EventType::KeyRelease(key);
        client::process_event(keyboard_mode, &event, None);
        // If Alt or AltGr is pressed, we need to send another key stoke to release it.
        // Because the controlled side may hold the alt state, if local window is switched by [Alt + Tab].
        if key == Key::Alt || key == Key::AltGr {
            event.event_type = EventType::KeyPress(key);
            client::process_event(keyboard_mode, &event, None);
            event.event_type = EventType::KeyRelease(key);
            client::process_event(keyboard_mode, &event, None);
        }
    }
}

pub fn get_keyboard_mode_enum(keyboard_mode: &str) -> KeyboardMode {
    match keyboard_mode {
        "map" => KeyboardMode::Map,
        "translate" => KeyboardMode::Translate,
        "legacy" => KeyboardMode::Legacy,
        _ => KeyboardMode::Map,
    }
}

#[inline]
pub fn is_modifier(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::MetaLeft
            | Key::MetaRight
            | Key::Alt
            | Key::AltGr
    )
}

#[inline]
#[allow(dead_code)]
pub fn is_modifier_code(evt: &KeyEvent) -> bool {
    match evt.union {
        Some(key_event::Union::Chr(code)) => {
            let key = rdev::linux_key_from_code(code);
            is_modifier(&key)
        }
        _ => false,
    }
}

#[inline]
pub fn is_numpad_rdev_key(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::Kp0
            | Key::Kp1
            | Key::Kp2
            | Key::Kp3
            | Key::Kp4
            | Key::Kp5
            | Key::Kp6
            | Key::Kp7
            | Key::Kp8
            | Key::Kp9
            | Key::KpMinus
            | Key::KpMultiply
            | Key::KpDivide
            | Key::KpPlus
            | Key::KpDecimal
    )
}

#[inline]
pub fn is_letter_rdev_key(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::KeyA
            | Key::KeyB
            | Key::KeyC
            | Key::KeyD
            | Key::KeyE
            | Key::KeyF
            | Key::KeyG
            | Key::KeyH
            | Key::KeyI
            | Key::KeyJ
            | Key::KeyK
            | Key::KeyL
            | Key::KeyM
            | Key::KeyN
            | Key::KeyO
            | Key::KeyP
            | Key::KeyQ
            | Key::KeyR
            | Key::KeyS
            | Key::KeyT
            | Key::KeyU
            | Key::KeyV
            | Key::KeyW
            | Key::KeyX
            | Key::KeyY
            | Key::KeyZ
    )
}

// https://github.com/rustdesk/rustdesk/issues/8599
// We just add these keys as letter keys.
#[inline]
pub fn is_letter_rdev_key_ex(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::LeftBracket | Key::RightBracket | Key::SemiColon | Key::Quote | Key::Comma | Key::Dot
    )
}

#[inline]
fn is_numpad_key(event: &Event) -> bool {
    matches!(event.event_type, EventType::KeyPress(key) | EventType::KeyRelease(key) if is_numpad_rdev_key(&key))
}

// Check is letter key for lock modes.
// Only letter keys need to check and send Lock key state.
#[inline]
fn is_letter_key_4_lock_modes(event: &Event) -> bool {
    matches!(event.event_type, EventType::KeyPress(key) | EventType::KeyRelease(key) if (is_letter_rdev_key(&key) || is_letter_rdev_key_ex(&key)))
}

fn parse_add_lock_modes_modifiers(
    key_event: &mut KeyEvent,
    lock_modes: i32,
    is_numpad_key: bool,
    is_letter_key: bool,
) {
    const CAPS_LOCK: i32 = 1;
    const NUM_LOCK: i32 = 2;
    // const SCROLL_LOCK: i32 = 3;
    if is_letter_key && (lock_modes & (1 << CAPS_LOCK) != 0) {
        key_event.modifiers.push(ControlKey::CapsLock.into());
    }
    if is_numpad_key && lock_modes & (1 << NUM_LOCK) != 0 {
        key_event.modifiers.push(ControlKey::NumLock.into());
    }
    // if lock_modes & (1 << SCROLL_LOCK) != 0 {
    //     key_event.modifiers.push(ControlKey::ScrollLock.into());
    // }
}

fn add_lock_modes_modifiers(key_event: &mut KeyEvent, is_numpad_key: bool, is_letter_key: bool) {
    if is_letter_key && get_key_state(enigo::Key::CapsLock) {
        key_event.modifiers.push(ControlKey::CapsLock.into());
    }
    if is_numpad_key && get_key_state(enigo::Key::NumLock) {
        key_event.modifiers.push(ControlKey::NumLock.into());
    }
}

pub fn convert_numpad_keys(key: Key) -> Key {
    if get_key_state(enigo::Key::NumLock) {
        return key;
    }
    match key {
        Key::Kp0 => Key::Insert,
        Key::KpDecimal => Key::Delete,
        Key::Kp1 => Key::End,
        Key::Kp2 => Key::DownArrow,
        Key::Kp3 => Key::PageDown,
        Key::Kp4 => Key::LeftArrow,
        Key::Kp5 => Key::Clear,
        Key::Kp6 => Key::RightArrow,
        Key::Kp7 => Key::Home,
        Key::Kp8 => Key::UpArrow,
        Key::Kp9 => Key::PageUp,
        _ => key,
    }
}

fn update_modifiers_state(event: &Event) {
    // for mouse
    let mut keys = MODIFIERS_STATE.lock().unwrap();
    match event.event_type {
        EventType::KeyPress(k) => {
            if keys.contains_key(&k) {
                keys.insert(k, true);
            }
        }
        EventType::KeyRelease(k) => {
            if keys.contains_key(&k) {
                keys.insert(k, false);
            }
        }
        _ => {}
    };
}

pub fn event_to_key_events(
    mut peer: String,
    event: &Event,
    keyboard_mode: KeyboardMode,
    _lock_modes: Option<i32>,
) -> Vec<KeyEvent> {
    peer.retain(|c| !c.is_whitespace());

    let mut key_event = KeyEvent::new();
    update_modifiers_state(event);

    match event.event_type {
        EventType::KeyPress(key) => {
            TO_RELEASE.lock().unwrap().insert(key, event.clone());
        }
        EventType::KeyRelease(key) => {
            TO_RELEASE.lock().unwrap().remove(&key);
        }
        _ => {}
    }

    key_event.mode = keyboard_mode.into();

    let mut key_events = match keyboard_mode {
        KeyboardMode::Map => map_keyboard_mode(peer.as_str(), event, key_event),
        KeyboardMode::Translate => translate_keyboard_mode(peer.as_str(), event, key_event),
        _ => {
            legacy_keyboard_mode(event, key_event)
        }
    };

    let is_numpad_key = is_numpad_key(&event);
    if keyboard_mode != KeyboardMode::Translate || is_numpad_key {
        let is_letter_key = is_letter_key_4_lock_modes(&event);
        for key_event in &mut key_events {
            if let Some(lock_modes) = _lock_modes {
                parse_add_lock_modes_modifiers(key_event, lock_modes, is_numpad_key, is_letter_key);
            } else {
                add_lock_modes_modifiers(key_event, is_numpad_key, is_letter_key);
            }
        }
    }
    key_events
}

pub fn send_key_event(key_event: &KeyEvent) {
    #[cfg(not(any(feature = "flutter", feature = "cli")))]
    if let Some(session) = CUR_SESSION.lock().unwrap().as_ref() {
        session.send_key_event(key_event);
    }

    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        session.send_key_event(key_event);
    }
}

pub fn get_peer_platform() -> String {
    #[cfg(not(any(feature = "flutter", feature = "cli")))]
    if let Some(session) = CUR_SESSION.lock().unwrap().as_ref() {
        return session.peer_platform();
    }
    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        return session.peer_platform();
    }
    "Windows".to_string()
}

pub fn legacy_keyboard_mode(event: &Event, mut key_event: KeyEvent) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    // legacy mode(0): Generate characters locally, look for keycode on other side.
    let (mut key, down_or_up) = match event.event_type {
        EventType::KeyPress(key) => (key, true),
        EventType::KeyRelease(key) => (key, false),
        _ => {
            return events;
        }
    };

    let peer = get_peer_platform();
    let is_win = peer == "Windows";
    if is_win {
        key = convert_numpad_keys(key);
    }

    let alt = get_key_state(enigo::Key::Alt);
    let ctrl = get_key_state(enigo::Key::Control) || get_key_state(enigo::Key::RightControl);
    let shift = get_key_state(enigo::Key::Shift) || get_key_state(enigo::Key::RightShift);
    let command = get_key_state(enigo::Key::Meta);
    let control_key = match key {
        Key::Alt => Some(ControlKey::Alt),
        Key::AltGr => Some(ControlKey::RAlt),
        Key::Backspace => Some(ControlKey::Backspace),
        Key::ControlLeft => {
            Some(ControlKey::Control)
        }
        Key::ControlRight => Some(ControlKey::RControl),
        Key::DownArrow => Some(ControlKey::DownArrow),
        Key::Escape => Some(ControlKey::Escape),
        Key::F1 => Some(ControlKey::F1),
        Key::F10 => Some(ControlKey::F10),
        Key::F11 => Some(ControlKey::F11),
        Key::F12 => Some(ControlKey::F12),
        Key::F2 => Some(ControlKey::F2),
        Key::F3 => Some(ControlKey::F3),
        Key::F4 => Some(ControlKey::F4),
        Key::F5 => Some(ControlKey::F5),
        Key::F6 => Some(ControlKey::F6),
        Key::F7 => Some(ControlKey::F7),
        Key::F8 => Some(ControlKey::F8),
        Key::F9 => Some(ControlKey::F9),
        Key::LeftArrow => Some(ControlKey::LeftArrow),
        Key::MetaLeft => Some(ControlKey::Meta),
        Key::MetaRight => Some(ControlKey::RWin),
        Key::Return => Some(ControlKey::Return),
        Key::RightArrow => Some(ControlKey::RightArrow),
        Key::ShiftLeft => Some(ControlKey::Shift),
        Key::ShiftRight => Some(ControlKey::RShift),
        Key::Space => Some(ControlKey::Space),
        Key::Tab => Some(ControlKey::Tab),
        Key::UpArrow => Some(ControlKey::UpArrow),
        Key::Delete => {
            if is_win && ctrl && alt {
                client::ctrl_alt_del();
                return events;
            }
            Some(ControlKey::Delete)
        }
        Key::Apps => Some(ControlKey::Apps),
        Key::Cancel => Some(ControlKey::Cancel),
        Key::Clear => Some(ControlKey::Clear),
        Key::Kana => Some(ControlKey::Kana),
        Key::Hangul => Some(ControlKey::Hangul),
        Key::Junja => Some(ControlKey::Junja),
        Key::Final => Some(ControlKey::Final),
        Key::Hanja => Some(ControlKey::Hanja),
        Key::Hanji => Some(ControlKey::Hanja),
        Key::Lang2 => Some(ControlKey::Convert),
        Key::Print => Some(ControlKey::Print),
        Key::Select => Some(ControlKey::Select),
        Key::Execute => Some(ControlKey::Execute),
        Key::PrintScreen => Some(ControlKey::Snapshot),
        Key::Help => Some(ControlKey::Help),
        Key::Sleep => Some(ControlKey::Sleep),
        Key::Separator => Some(ControlKey::Separator),
        Key::KpReturn => Some(ControlKey::NumpadEnter),
        Key::Kp0 => Some(ControlKey::Numpad0),
        Key::Kp1 => Some(ControlKey::Numpad1),
        Key::Kp2 => Some(ControlKey::Numpad2),
        Key::Kp3 => Some(ControlKey::Numpad3),
        Key::Kp4 => Some(ControlKey::Numpad4),
        Key::Kp5 => Some(ControlKey::Numpad5),
        Key::Kp6 => Some(ControlKey::Numpad6),
        Key::Kp7 => Some(ControlKey::Numpad7),
        Key::Kp8 => Some(ControlKey::Numpad8),
        Key::Kp9 => Some(ControlKey::Numpad9),
        Key::KpDivide => Some(ControlKey::Divide),
        Key::KpMultiply => Some(ControlKey::Multiply),
        Key::KpDecimal => Some(ControlKey::Decimal),
        Key::KpMinus => Some(ControlKey::Subtract),
        Key::KpPlus => Some(ControlKey::Add),
        Key::CapsLock | Key::NumLock | Key::ScrollLock => {
            return events;
        }
        Key::Home => Some(ControlKey::Home),
        Key::End => Some(ControlKey::End),
        Key::Insert => Some(ControlKey::Insert),
        Key::PageUp => Some(ControlKey::PageUp),
        Key::PageDown => Some(ControlKey::PageDown),
        Key::Pause => Some(ControlKey::Pause),
        _ => None,
    };
    if let Some(k) = control_key {
        key_event.set_control_key(k);
    } else {
        let name = event
            .unicode
            .as_ref()
            .and_then(|unicode| unicode.name.clone());
        let mut chr = match &name {
            Some(ref s) => {
                if s.len() <= 2 {
                    // exclude chinese characters
                    s.chars().next().unwrap_or('\0')
                } else {
                    '\0'
                }
            }
            _ => '\0',
        };
        if chr == '\u{00b7}' {
            // special for Chinese
            chr = '`';
        }
        if chr == '\0' {
            chr = match key {
                Key::Num1 => '1',
                Key::Num2 => '2',
                Key::Num3 => '3',
                Key::Num4 => '4',
                Key::Num5 => '5',
                Key::Num6 => '6',
                Key::Num7 => '7',
                Key::Num8 => '8',
                Key::Num9 => '9',
                Key::Num0 => '0',
                Key::KeyA => 'a',
                Key::KeyB => 'b',
                Key::KeyC => 'c',
                Key::KeyD => 'd',
                Key::KeyE => 'e',
                Key::KeyF => 'f',
                Key::KeyG => 'g',
                Key::KeyH => 'h',
                Key::KeyI => 'i',
                Key::KeyJ => 'j',
                Key::KeyK => 'k',
                Key::KeyL => 'l',
                Key::KeyM => 'm',
                Key::KeyN => 'n',
                Key::KeyO => 'o',
                Key::KeyP => 'p',
                Key::KeyQ => 'q',
                Key::KeyR => 'r',
                Key::KeyS => 's',
                Key::KeyT => 't',
                Key::KeyU => 'u',
                Key::KeyV => 'v',
                Key::KeyW => 'w',
                Key::KeyX => 'x',
                Key::KeyY => 'y',
                Key::KeyZ => 'z',
                Key::Comma => ',',
                Key::Dot => '.',
                Key::SemiColon => ';',
                Key::Quote => '\'',
                Key::LeftBracket => '[',
                Key::RightBracket => ']',
                Key::Slash => '/',
                Key::BackSlash => '\\',
                Key::Minus => '-',
                Key::Equal => '=',
                Key::BackQuote => '`',
                _ => '\0',
            }
        }
        if chr != '\0' {
            if chr == 'l' && is_win && command {
                client::lock_screen();
                return events;
            }
            key_event.set_chr(chr as _);
        } else {
            log::error!("Unknown key {:?}", &event);
            return events;
        }
    }
    let (alt, ctrl, shift, command) = client::get_modifiers_state(alt, ctrl, shift, command);
    client::legacy_modifiers(&mut key_event, alt, ctrl, shift, command);

    if down_or_up == true {
        key_event.down = true;
    }
    events.push(key_event);
    events
}

#[inline]
pub fn map_keyboard_mode(_peer: &str, event: &Event, key_event: KeyEvent) -> Vec<KeyEvent> {
    _map_keyboard_mode(_peer, event, key_event)
        .map(|e| vec![e])
        .unwrap_or_default()
}

fn _map_keyboard_mode(_peer: &str, event: &Event, mut key_event: KeyEvent) -> Option<KeyEvent> {
    match event.event_type {
        EventType::KeyPress(..) => {
            key_event.down = true;
        }
        EventType::KeyRelease(..) => {
            key_event.down = false;
        }
        _ => return None,
    };

    let keycode = match _peer {
        OS_LOWER_WINDOWS => rdev::macos_code_to_win_scancode(event.platform_code as _)?,
        OS_LOWER_MACOS => event.platform_code as _,
        OS_LOWER_ANDROID => rdev::macos_code_to_android_key_code(event.platform_code as _)?,
        _ => rdev::macos_code_to_linux_code(event.platform_code as _)?,
    };
    key_event.set_chr(keycode as _);
    Some(key_event)
}

fn try_fill_unicode(_peer: &str, event: &Event, key_event: &KeyEvent, events: &mut Vec<KeyEvent>) {
    match &event.unicode {
        Some(unicode_info) => {
            if let Some(name) = &unicode_info.name {
                if name.len() > 0 {
                    let mut evt = key_event.clone();
                    evt.set_seq(name.to_string());
                    evt.down = true;
                    events.push(evt);
                }
            }
        }
        None => {}
    }
}

// https://github.com/rustdesk/rustdesk/wiki/FAQ#keyboard-translation-modes
pub fn translate_keyboard_mode(peer: &str, event: &Event, key_event: KeyEvent) -> Vec<KeyEvent> {
    let mut events: Vec<KeyEvent> = Vec::new();

    if let Some(unicode_info) = &event.unicode {
        if unicode_info.is_dead {
            if peer != OS_LOWER_MACOS && unsafe { IS_LEFT_OPTION_DOWN } {
                // try clear dead key state
                // rdev::clear_dead_key_state();
            } else {
                return events;
            }
        }
    }

    if is_numpad_key(&event) {
        events.append(&mut map_keyboard_mode(peer, event, key_event));
        return events;
    }

    // ignore right option key
    if event.platform_code == rdev::kVK_RightOption as u32 {
        return events;
    }

    if !unsafe { IS_LEFT_OPTION_DOWN } {
        try_fill_unicode(peer, event, &key_event, &mut events);
    }

    if events.is_empty() {
        events.append(&mut map_keyboard_mode(peer, event, key_event));
    }
    events
}

pub fn keycode_to_rdev_key(keycode: u32) -> Key {
    return rdev::macos_key_from_code(keycode.try_into().unwrap_or_default());
}

#[cfg(feature = "flutter")]
pub mod input_source {
    use hbb_common::log;
    use hbb_common::SessionID;

    use crate::ui_interface::{get_local_option, set_local_option};

    pub const CONFIG_OPTION_INPUT_SOURCE: &str = "input-source";
    // rdev grab mode
    pub const CONFIG_INPUT_SOURCE_1: &str = "Input source 1";
    pub const CONFIG_INPUT_SOURCE_1_TIP: &str = "input_source_1_tip";
    // flutter grab mode
    pub const CONFIG_INPUT_SOURCE_2: &str = "Input source 2";
    pub const CONFIG_INPUT_SOURCE_2_TIP: &str = "input_source_2_tip";

    pub const CONFIG_INPUT_SOURCE_DEFAULT: &str = CONFIG_INPUT_SOURCE_1;

    pub fn init_input_source() {
        if !crate::platform::macos::is_can_input_monitoring(false) {
            log::error!("init_input_source, is_can_input_monitoring() false");
            set_local_option(
                CONFIG_OPTION_INPUT_SOURCE.to_string(),
                CONFIG_INPUT_SOURCE_2.to_string(),
            );
            return;
        }
        let cur_input_source = get_cur_session_input_source();
        if cur_input_source == CONFIG_INPUT_SOURCE_1 {
            super::IS_RDEV_ENABLED.store(true, super::Ordering::SeqCst);
        }
        super::client::start_grab_loop();
    }

    pub fn change_input_source(session_id: SessionID, input_source: String) {
        let cur_input_source = get_cur_session_input_source();
        if cur_input_source == input_source {
            return;
        }
        if input_source == CONFIG_INPUT_SOURCE_1 {
            if !crate::platform::macos::is_can_input_monitoring(false) {
                log::error!("change_input_source, is_can_input_monitoring() false");
                return;
            }
            // It is ok to start grab loop multiple times.
            super::client::start_grab_loop();
            super::IS_RDEV_ENABLED.store(true, super::Ordering::SeqCst);
            crate::flutter_ffi::session_enter_or_leave(session_id, true);
        } else if input_source == CONFIG_INPUT_SOURCE_2 {
            // No need to stop grab loop.
            crate::flutter_ffi::session_enter_or_leave(session_id, false);
            super::IS_RDEV_ENABLED.store(false, super::Ordering::SeqCst);
        }
        set_local_option(CONFIG_OPTION_INPUT_SOURCE.to_string(), input_source);
    }

    #[inline]
    pub fn get_cur_session_input_source() -> String {
        let input_source = get_local_option(CONFIG_OPTION_INPUT_SOURCE.to_string());
        if input_source.is_empty() {
            CONFIG_INPUT_SOURCE_DEFAULT.to_string()
        } else {
            input_source
        }
    }

    #[inline]
    pub fn get_supported_input_source() -> Vec<(String, String)> {
        vec![
            (
                CONFIG_INPUT_SOURCE_1.to_string(),
                CONFIG_INPUT_SOURCE_1_TIP.to_string(),
            ),
            (
                CONFIG_INPUT_SOURCE_2.to_string(),
                CONFIG_INPUT_SOURCE_2_TIP.to_string(),
            ),
        ]
    }
}
