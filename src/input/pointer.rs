use serde::{Deserialize, Serialize};
use std::io;

pub const MAX_DELTA: i32 = 32_767;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Relative screen coordinates: +x right, +y down. Scroll uses Windows wheel
/// units (120 = one detent), +vertical up, +horizontal right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PointerEvent {
    Move { dx: i32, dy: i32 },
    Button { button: Button, pressed: bool },
    Scroll { horizontal: i32, vertical: i32 },
}

impl PointerEvent {
    pub fn validate(self) -> io::Result<()> {
        let (x, y) = match self {
            Self::Move { dx, dy } => (dx, dy),
            Self::Scroll {
                horizontal,
                vertical,
            } => (horizontal, vertical),
            Self::Button { .. } => return Ok(()),
        };
        if !(-MAX_DELTA..=MAX_DELTA).contains(&x) || !(-MAX_DELTA..=MAX_DELTA).contains(&y) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pointer delta out of range",
            ));
        }
        Ok(())
    }

    /// Merge only adjacent movement in the same direction. Never cross a
    /// button/key boundary or flatten a reversal during a drag.
    pub fn merge(&mut self, next: Self) -> bool {
        fn sum(a: i32, b: i32) -> Option<i32> {
            if a != 0 && b != 0 && a.signum() != b.signum() {
                return None;
            }
            a.checked_add(b)
                .filter(|v| (-MAX_DELTA..=MAX_DELTA).contains(v))
        }
        let pair = match (*self, next) {
            (Self::Move { dx: a, dy: b }, Self::Move { dx: c, dy: d }) => sum(a, c)
                .zip(sum(b, d))
                .map(|(dx, dy)| Self::Move { dx, dy }),
            (
                Self::Scroll {
                    horizontal: a,
                    vertical: b,
                },
                Self::Scroll {
                    horizontal: c,
                    vertical: d,
                },
            ) => sum(a, c)
                .zip(sum(b, d))
                .map(|(horizontal, vertical)| Self::Scroll {
                    horizontal,
                    vertical,
                }),
            _ => None,
        };
        if let Some(merged) = pair {
            *self = merged;
            true
        } else {
            false
        }
    }
}

/// Keep sub-unit wheel deltas so slow two-finger scrolling is not lost.
#[derive(Default)]
pub struct MacPointer {
    scroll_x: f64,
    scroll_y: f64,
}
impl MacPointer {
    pub fn movement(dx: i64, dy: i64) -> io::Result<Option<PointerEvent>> {
        let dx = i32::try_from(dx).map_err(|_| invalid_delta())?;
        let dy = i32::try_from(dy).map_err(|_| invalid_delta())?;
        let event = PointerEvent::Move { dx, dy };
        event.validate()?;
        Ok((dx != 0 || dy != 0).then_some(event))
    }

    /// Continuous devices supply pixels (40 pixels per wheel detent); discrete
    /// devices supply lines/detents. macOS has already applied natural scrolling.
    pub fn scroll(
        &mut self,
        horizontal: f64,
        vertical: f64,
        continuous: bool,
    ) -> io::Result<Option<PointerEvent>> {
        let scale = if continuous { 3.0 } else { 120.0 };
        let x = self.scroll_x - horizontal * scale;
        let y = self.scroll_y + vertical * scale;
        if !x.is_finite()
            || !y.is_finite()
            || x.abs() > MAX_DELTA as f64
            || y.abs() > MAX_DELTA as f64
        {
            return Err(invalid_delta());
        }
        let horizontal = x.trunc() as i32;
        let vertical = y.trunc() as i32;
        self.scroll_x = x - f64::from(horizontal);
        self.scroll_y = y - f64::from(vertical);
        Ok(
            (horizontal != 0 || vertical != 0).then_some(PointerEvent::Scroll {
                horizontal,
                vertical,
            }),
        )
    }
}
fn invalid_delta() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid captured pointer delta")
}
