use super::{Backend, Content};
use std::{borrow::Cow, io};

pub struct Native(arboard::Clipboard);
pub fn create() -> io::Result<Box<dyn Backend>> {
    arboard::Clipboard::new()
        .map(|c| Box::new(Native(c)) as Box<dyn Backend>)
        .map_err(io::Error::other)
}
impl Backend for Native {
    fn read(&mut self) -> io::Result<Option<Content>> {
        // Copying applications may briefly own the clipboard while publishing formats.
        for attempt in 0..5 {
            match self.read_once() {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && attempt < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                result => return result,
            }
        }
        unreachable!()
    }
    fn write(&mut self, content: &Content) -> io::Result<()> {
        match content {
            Content::Text(text) => self.0.set_text(text),
            Content::Image {
                width,
                height,
                rgba,
            } => self.0.set_image(arboard::ImageData {
                width: *width as usize,
                height: *height as usize,
                bytes: Cow::Borrowed(rgba),
            }),
            Content::Files(paths) => self.0.set().file_list(paths),
        }
        .map_err(io::Error::other)
    }
}
impl Native {
    fn read_once(&mut self) -> io::Result<Option<Content>> {
        // File lists have priority over the textual paths many apps also publish.
        match self.0.get().file_list() {
            Ok(paths) if !paths.is_empty() => return Ok(Some(Content::Files(paths))),
            Err(arboard::Error::ClipboardOccupied) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "剪贴板暂被其他应用占用，请重试同步",
                ))
            }
            _ => (),
        }
        match self.0.get_image() {
            Ok(image) => {
                let width = u32::try_from(image.width).map_err(io::Error::other)?;
                let height = u32::try_from(image.height).map_err(io::Error::other)?;
                if image.bytes.len() as u64 > super::wire::IMAGE_LIMIT {
                    return Err(io::Error::other("剪贴板图片超过 64 MiB"));
                }
                return Ok(Some(Content::Image {
                    width,
                    height,
                    rgba: image.bytes.into_owned(),
                }));
            }
            Err(arboard::Error::ClipboardOccupied) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "剪贴板暂被其他应用占用，请重试同步",
                ))
            }
            _ => (),
        }
        match self.0.get_text() {
            Ok(text) => Ok(Some(Content::Text(text))),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(arboard::Error::ClipboardOccupied) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "剪贴板暂被其他应用占用，请重试同步",
            )),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}
