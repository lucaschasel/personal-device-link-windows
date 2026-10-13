use serde::{Deserialize, Serialize};
use std::io;

/// A connection never grants input control. Both channels are opt-in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlOptions {
    pub keyboard: bool,
    pub pointer: bool,
    pub shortcut_mapping: bool,
}
impl ControlOptions {
    pub const ALL: Self = Self {
        keyboard: true,
        pointer: true,
        shortcut_mapping: true,
    };
    pub fn validate(self) -> io::Result<()> {
        if !self.keyboard && !self.pointer {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "请至少选择键盘或鼠标/触摸板",
            ))
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub options: ControlOptions,
    pub target_os: String,
}
/// Marks our generated events so capture never feeds them back to another peer.
pub const INJECTED_TAG: i64 = 0x50444c49;
