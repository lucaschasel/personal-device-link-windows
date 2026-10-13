use personal_device_link::input::{keyboard::*, session::*};

#[test]
fn target_specific_modifiers_and_physical_mapping_roundtrip() {
    let mut mac_to_mac = MacKeyboard::for_target("macos", true);
    assert_eq!(
        mac_to_mac.event(55, true, false).unwrap().key,
        Key::LeftMeta
    );
    let mut mac_to_win = MacKeyboard::for_target("windows", true);
    assert_eq!(
        mac_to_win.event(55, true, false).unwrap().key,
        Key::LeftControl
    );
    let mut physical = MacKeyboard::for_target("windows", false);
    assert_eq!(physical.event(55, true, false).unwrap().key, Key::LeftMeta);
    for code in 0..128 {
        if let Some(key) = mac_key(
            code,
            ModifierMapping {
                command_as_meta: true,
            },
        ) {
            assert_eq!(mac_code(key), Some(code));
            let (scan, extended) = key.scan_code();
            assert_eq!(windows_key(scan, extended), Some(key));
        }
    }
}

#[test]
fn windows_modifiers_repeat_and_release_preserve_target_selection() {
    let mut keys = WindowsKeyboard::default();
    assert_eq!(
        keys.event(0x1d, false, true, "macos", true).unwrap().key,
        Key::LeftMeta
    );
    assert!(keys.event(0x1d, false, true, "macos", true).is_none());
    assert_eq!(
        keys.event(0x5b, true, true, "macos", true).unwrap().key,
        Key::LeftControl
    );
    assert_eq!(
        keys.event(0x1d, false, false, "windows", false)
            .unwrap()
            .key,
        Key::LeftMeta
    );
    assert!(!keys.event(0x1e, false, true, "macos", true).unwrap().repeat);
    assert!(keys.event(0x1e, false, true, "macos", true).unwrap().repeat);
    let mut physical = WindowsKeyboard::default();
    assert_eq!(
        physical
            .event(0x1d, true, true, "macos", false)
            .unwrap()
            .key,
        Key::RightControl
    );
}
use std::{
    io,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Recorder(Arc<Mutex<Vec<(Key, bool)>>>);
impl InputSink for Recorder {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        self.0.lock().unwrap().push((key, pressed));
        Ok(())
    }
}
fn event(key: Key, pressed: bool) -> KeyEvent {
    KeyEvent {
        key,
        pressed,
        repeat: false,
    }
}

#[test]
fn command_and_control_alias_does_not_release_early() {
    let mut keyboard = MacKeyboard::default();
    assert_eq!(
        keyboard.flags_changed(55, COMMAND_FLAG),
        Some(event(Key::LeftControl, true))
    );
    assert_eq!(
        keyboard.flags_changed(59, COMMAND_FLAG | CONTROL_FLAG),
        None
    );
    assert_eq!(keyboard.flags_changed(55, CONTROL_FLAG), None);
    assert_eq!(
        keyboard.flags_changed(59, 0),
        Some(event(Key::LeftControl, false))
    );
}

#[test]
fn left_and_right_modifiers_track_independent_releases() {
    let mut keyboard = MacKeyboard::default();
    assert_eq!(
        keyboard.flags_changed(56, SHIFT_FLAG),
        Some(event(Key::LeftShift, true))
    );
    assert_eq!(
        keyboard.flags_changed(60, SHIFT_FLAG),
        Some(event(Key::RightShift, true))
    );
    assert_eq!(
        keyboard.flags_changed(56, SHIFT_FLAG),
        Some(event(Key::LeftShift, false))
    );
    assert_eq!(
        keyboard.flags_changed(60, 0),
        Some(event(Key::RightShift, false))
    );
}

#[test]
fn letters_repeats_and_preexisting_keys_are_handled() {
    let mut keyboard = MacKeyboard::default();
    assert_eq!(keyboard.event(0, false, false), None);
    assert_eq!(keyboard.event(0, true, true), None);
    assert_eq!(keyboard.event(0, true, false), Some(event(Key::A, true)));
    assert_eq!(keyboard.event(0, true, false), None);
    assert_eq!(
        keyboard.event(0, true, true),
        Some(KeyEvent {
            key: Key::A,
            pressed: true,
            repeat: true
        })
    );
    assert_eq!(keyboard.event(0, false, false), Some(event(Key::A, false)));
}

#[test]
fn emergency_requires_physical_control_option_and_escape_down() {
    assert!(emergency_exit(53, CONTROL_FLAG | OPTION_FLAG, true));
    assert!(emergency_exit(
        53,
        CONTROL_FLAG | OPTION_FLAG | SHIFT_FLAG,
        true
    ));
    assert!(!emergency_exit(53, COMMAND_FLAG | OPTION_FLAG, true));
    assert!(!emergency_exit(53, CONTROL_FLAG | OPTION_FLAG, false));
    assert!(!emergency_exit(0, CONTROL_FLAG | OPTION_FLAG, true));
}

#[test]
fn local_pointer_chord_preserves_local_button_release_before_resuming_remote() {
    assert!(local_pointer_mode(
        true,
        false,
        CONTROL_FLAG | OPTION_FLAG,
        false,
        false
    ));
    assert!(!local_pointer_mode(
        false,
        false,
        CONTROL_FLAG | OPTION_FLAG,
        false,
        false
    ));
    assert!(local_pointer_mode(true, true, 0, true, false));
    assert!(local_pointer_mode(true, true, 0, false, true));
    assert!(!local_pointer_mode(true, true, 0, false, false));
    assert!(!local_pointer_mode(
        true,
        false,
        COMMAND_FLAG | OPTION_FLAG,
        false,
        false
    ));
}

#[test]
fn maps_common_keys_and_extended_windows_scan_codes() {
    let mapping = ModifierMapping::default();
    for (code, key) in [
        (0, Key::A),
        (18, Key::Digit1),
        (49, Key::Space),
        (36, Key::Enter),
        (51, Key::Backspace),
        (48, Key::Tab),
        (58, Key::LeftAlt),
        (55, Key::LeftControl),
        (123, Key::ArrowLeft),
        (126, Key::ArrowUp),
    ] {
        assert_eq!(mac_key(code, mapping), Some(key));
    }
    assert_eq!(
        mac_key(
            55,
            ModifierMapping {
                command_as_meta: true
            }
        ),
        Some(Key::LeftMeta)
    );
    assert_eq!(mac_key(65535, mapping), None);
    assert_eq!(Key::A.scan_code(), (0x1e, false));
    assert_eq!(Key::ArrowLeft.scan_code(), (0x4b, true));
    assert_eq!(Key::RightControl.scan_code(), (0x1d, true));
}

#[test]
fn cleanup_releases_owned_keys_once_before_modifiers() {
    let recorder = Recorder(Arc::new(Mutex::new(Vec::new())));
    {
        let mut session = InputSession::new(Box::new(recorder.clone()));
        session.apply(event(Key::LeftControl, true)).unwrap();
        session.apply(event(Key::A, true)).unwrap();
        session.apply(event(Key::B, false)).unwrap(); // Never release an unowned key.
        session.apply(event(Key::A, true)).unwrap(); // Duplicate is idempotent.
    }
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![
            (Key::LeftControl, true),
            (Key::A, true),
            (Key::A, false),
            (Key::LeftControl, false)
        ]
    );
}

#[test]
fn rejects_repeat_without_down_or_on_modifier() {
    let recorder = Recorder(Arc::new(Mutex::new(Vec::new())));
    let mut session = InputSession::new(Box::new(recorder.clone()));
    assert!(session
        .apply(KeyEvent {
            key: Key::A,
            pressed: true,
            repeat: true
        })
        .is_err());
    session.apply(event(Key::LeftShift, true)).unwrap();
    assert!(session
        .apply(KeyEvent {
            key: Key::LeftShift,
            pressed: true,
            repeat: true
        })
        .is_err());
    assert!(session
        .apply(KeyEvent {
            key: Key::LeftShift,
            pressed: false,
            repeat: true
        })
        .is_err());
}

#[test]
fn release_failure_does_not_skip_other_keys_and_drop_retries() {
    struct FailOnce {
        failed: bool,
        recorder: Recorder,
    }
    impl InputSink for FailOnce {
        fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
            if !pressed && !self.failed {
                self.failed = true;
                return Err(io::Error::other("simulated input failure"));
            }
            self.recorder.send(key, pressed)
        }
    }
    let recorder = Recorder(Arc::new(Mutex::new(Vec::new())));
    let mut session = InputSession::new(Box::new(FailOnce {
        failed: false,
        recorder: recorder.clone(),
    }));
    session.apply(event(Key::A, true)).unwrap();
    session.apply(event(Key::LeftShift, true)).unwrap();
    assert!(session.release_all().is_err());
    drop(session);
    let records = recorder.0.lock().unwrap();
    assert!(records.contains(&(Key::A, false)));
    assert!(records.contains(&(Key::LeftShift, false)));
}
