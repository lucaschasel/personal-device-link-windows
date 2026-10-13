use super::{lock, Content};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

pub const TEXT_LIMIT: u64 = 1024 * 1024;
pub const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
pub const FILE_LIMIT: u64 = 256 * 1024 * 1024;
pub const CACHE_LIMIT: u64 = 1024 * 1024 * 1024;
const BLOCK: usize = 64 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    name: String,
    size: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Header {
    Empty,
    Text { size: u64 },
    Image { width: u32, height: u32 },
    Files { files: Vec<Blob> },
}
fn error(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn alive(flag: &AtomicBool) -> io::Result<()> {
    if flag.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(error("剪贴板传输已取消"))
    }
}
fn image_size(width: u32, height: u32) -> io::Result<u64> {
    let size = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|s| s.checked_mul(4))
        .ok_or_else(|| error("图片尺寸溢出"))?;
    if width == 0 || height == 0 || size > IMAGE_LIMIT {
        return Err(error("剪贴板图片像素数据超过 64 MiB 或尺寸无效"));
    }
    Ok(size)
}
fn files_header(paths: &[PathBuf]) -> io::Result<Vec<Blob>> {
    let mut result = vec![];
    for path in paths {
        let m = fs::symlink_metadata(path)?;
        if !m.is_file() || m.file_type().is_symlink() {
            return Err(error("剪贴板只支持普通文件，文件夹或链接请先压缩"));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("文件名无法跨系统传输"))?
            .to_owned();
        result.push(Blob {
            name,
            size: m.len(),
        });
    }
    validate_files(&result)?;
    Ok(result)
}
fn validate_files(files: &[Blob]) -> io::Result<u64> {
    if files.is_empty() || files.len() > 16 {
        return Err(error("一次剪贴板复制支持 1 至 16 个文件"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut size = 0_u64;
    for f in files {
        crate::transfer::validate_name(&f.name)?;
        if !names.insert(f.name.to_lowercase()) {
            return Err(error("复制的文件存在重名，请分批复制"));
        }
        size = size
            .checked_add(f.size)
            .ok_or_else(|| error("文件大小溢出"))?;
        if size > FILE_LIMIT {
            return Err(error("剪贴板文件总量超过 256 MiB，请使用文件传输面板"));
        }
    }
    Ok(size)
}
fn write_header(w: &mut impl Write, h: &Header) -> io::Result<()> {
    let data = serde_json::to_vec(h).map_err(io::Error::other)?;
    if data.len() > 16384 {
        return Err(error("剪贴板描述过长"));
    }
    w.write_all(&(data.len() as u32).to_be_bytes())?;
    w.write_all(&data)
}
fn read_header(r: &mut impl Read) -> io::Result<Header> {
    let mut len = [0; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > 16384 {
        return Err(error("剪贴板描述长度无效"));
    }
    let mut data = vec![0; len];
    r.read_exact(&mut data)?;
    serde_json::from_slice(&data).map_err(io::Error::other)
}
fn transfer(
    r: &mut impl Read,
    w: &mut impl Write,
    size: u64,
    running: &AtomicBool,
) -> io::Result<[u8; 32]> {
    let mut left = size;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; BLOCK];
    while left > 0 {
        alive(running)?;
        let n = left.min(BLOCK as u64) as usize;
        r.read_exact(&mut buffer[..n])?;
        hash.update(&buffer[..n]);
        w.write_all(&buffer[..n])?;
        left -= n as u64;
    }
    Ok(hash.finalize().into())
}
fn send_bytes(w: &mut impl Write, bytes: &[u8], running: &AtomicBool) -> io::Result<()> {
    let hash = transfer(&mut &bytes[..], w, bytes.len() as u64, running)?;
    w.write_all(&hash)
}
fn receive_bytes(r: &mut impl Read, size: u64, running: &AtomicBool) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(size as usize);
    receive_blob(r, &mut bytes, size, running)?;
    Ok(bytes)
}
fn receive_blob(
    r: &mut impl Read,
    w: &mut impl Write,
    size: u64,
    running: &AtomicBool,
) -> io::Result<()> {
    let hash = transfer(r, w, size, running)?;
    let mut expected = [0; 32];
    r.read_exact(&mut expected)?;
    if hash != expected {
        return Err(error("剪贴板内容校验失败"));
    }
    Ok(())
}
pub fn send(w: &mut impl Write, content: Option<&Content>, running: &AtomicBool) -> io::Result<()> {
    alive(running)?;
    match content {
        None => write_header(w, &Header::Empty)?,
        Some(Content::Text(text)) => {
            if text.len() as u64 > TEXT_LIMIT {
                return Err(error("剪贴板文本超过 1 MiB"));
            }
            write_header(
                w,
                &Header::Text {
                    size: text.len() as u64,
                },
            )?;
            send_bytes(w, text.as_bytes(), running)?;
        }
        Some(Content::Image {
            width,
            height,
            rgba,
        }) => {
            if image_size(*width, *height)? != rgba.len() as u64 {
                return Err(error("图片像素数据长度无效"));
            }
            write_header(
                w,
                &Header::Image {
                    width: *width,
                    height: *height,
                },
            )?;
            send_bytes(w, rgba, running)?;
        }
        Some(Content::Files(paths)) => {
            let files = files_header(paths)?;
            write_header(
                w,
                &Header::Files {
                    files: files.clone(),
                },
            )?;
            for (path, header) in paths.iter().zip(files) {
                let mut file = File::open(path)?;
                let before = file.metadata()?;
                if before.len() != header.size {
                    return Err(error("复制的源文件已改变，请重新复制"));
                }
                let hash = transfer(&mut file, w, header.size, running)?;
                let after = file.metadata()?;
                if before.len() != after.len() || before.modified()? != after.modified()? {
                    return Err(error("传输期间源文件已改变"));
                }
                w.write_all(&hash)?;
            }
        }
    }
    w.flush()
}

pub struct Cache {
    root: PathBuf,
    used: Mutex<(u64, usize)>,
}
impl Cache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            used: Mutex::new((0, 0)),
        }
    }
}
struct Pending {
    directory: PathBuf,
    paths: Vec<PathBuf>,
    committed: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }
}
pub fn receive(
    r: &mut impl Read,
    cache: &Cache,
    running: &AtomicBool,
) -> io::Result<Option<Content>> {
    alive(running)?;
    match read_header(r)? {
        Header::Empty => Ok(None),
        Header::Text { size } => {
            if size > TEXT_LIMIT {
                return Err(error("剪贴板文本超过 1 MiB"));
            }
            let text =
                String::from_utf8(receive_bytes(r, size, running)?).map_err(io::Error::other)?;
            Ok(Some(Content::Text(text)))
        }
        Header::Image { width, height } => {
            let size = image_size(width, height)?;
            Ok(Some(Content::Image {
                width,
                height,
                rgba: receive_bytes(r, size, running)?,
            }))
        }
        Header::Files { files } => {
            let size = validate_files(&files)?;
            let mut used = lock(&cache.used);
            if used.0.saturating_add(size) > CACHE_LIMIT || used.1 >= 64 {
                return Err(error(
                    "本次运行的剪贴板文件缓存已达 1 GiB 或 64 批，请先粘贴文件并清理缓存后重启",
                ));
            }
            fs::create_dir_all(&cache.root)?;
            if fs::symlink_metadata(&cache.root)?.file_type().is_symlink() {
                return Err(error("剪贴板缓存目录不能是链接"));
            }
            let directory = cache.root.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&directory)?;
            let mut pending = Pending {
                directory,
                paths: vec![],
                committed: false,
            };
            for blob in files {
                let path = pending.directory.join(blob.name);
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                pending.paths.push(path);
                receive_blob(r, &mut file, blob.size, running)?;
                file.sync_all()?;
            }
            alive(running)?;
            used.0 += size;
            used.1 += 1;
            pending.committed = true;
            Ok(Some(Content::Files(pending.paths.clone())))
        }
    }
}
