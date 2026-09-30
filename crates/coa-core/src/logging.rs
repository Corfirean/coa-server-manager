//! Size-bounded rotating log file: `manager.log`, `manager.log.1` .. `manager.log.N`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct RotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Option<File>,
    written: u64,
}

impl RotatingWriter {
    pub fn open(path: impl Into<PathBuf>, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self { path, max_bytes, keep, file: Some(file), written })
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{n}"));
        self.path.with_file_name(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        let _ = fs::remove_file(self.numbered(self.keep));
        for n in (1..self.keep).rev() {
            let from = self.numbered(n);
            if from.exists() {
                let _ = fs::rename(&from, self.numbered(n + 1));
            }
        }
        if self.keep > 0 {
            let _ = fs::rename(&self.path, self.numbered(1));
        } else {
            let _ = fs::remove_file(&self.path);
        }
        self.file = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let file = self.file.as_mut().ok_or_else(|| io::Error::other("log closed"))?;
        let n = file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
pub struct SharedWriter(Arc<Mutex<RotatingWriter>>);

impl SharedWriter {
    pub fn new(w: RotatingWriter) -> Self {
        Self(Arc::new(Mutex::new(w)))
    }
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().map_err(|_| io::Error::other("log poisoned"))?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().map_err(|_| io::Error::other("log poisoned"))?.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install the global tracing subscriber writing to a rotated file (2 MiB x 5).
pub fn init(log_path: &Path) -> io::Result<()> {
    let writer = SharedWriter::new(RotatingWriter::open(log_path, 2 * 1024 * 1024, 5)?);
    let _ = tracing_subscriber::fmt().with_writer(writer).with_ansi(false).with_max_level(tracing::Level::INFO).try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_and_bounds_total_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.log");
        let mut w = RotatingWriter::open(&p, 100, 2).unwrap();
        for _ in 0..50 {
            w.write_all(&[b'x'; 30]).unwrap();
        }
        w.flush().unwrap();
        let mut names: Vec<_> =
            fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["m.log", "m.log.1", "m.log.2"]);
        let total: u64 = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum();
        assert!(total <= 3 * 100);
    }
}
