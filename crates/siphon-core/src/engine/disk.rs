use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct DiskWriter {
    file: Arc<File>,
    part_path: PathBuf,
    final_path: PathBuf,
}

impl DiskWriter {
    pub fn open_or_create(final_path: &Path, total_bytes: Option<u64>) -> Result<Self> {
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create download folder {:?}", parent))?;
        }

        let mut part_path = final_path.to_path_buf();
        let ext = part_path.extension().unwrap_or_default().to_string_lossy();
        let new_ext = if ext.is_empty() {
            "siphon.part".to_string()
        } else {
            format!("{}.siphon.part", ext)
        };
        part_path.set_extension(new_ext);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part_path)
            .with_context(|| format!("Failed to open part file {:?}", part_path))?;

        // Pre-allocate disk space if total size is known
        if let Some(total) = total_bytes {
            let current_len = file.metadata()?.len();
            if current_len < total {
                // Pre-allocate disk space (fallocate) to prevent disk fragmentation
                let _ = file.allocate(total);
                let _ = file.set_len(total);
            }
        }

        Ok(Self {
            file: Arc::new(file),
            part_path,
            final_path: final_path.to_path_buf(),
        })
    }

    #[cfg(unix)]
    pub fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        use std::os::unix::fs::FileExt as UnixFileExt;
        self.file
            .write_all_at(buf, offset)
            .with_context(|| format!("Failed to write {} bytes at offset {}", buf.len(), offset))?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = &*self.file;
        // On non-unix, acquire lock or seek
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(buf)?;
        Ok(())
    }

    pub fn finish(&self) -> Result<()> {
        self.file.sync_all().context("Failed to sync file to disk")?;
        // Atomic rename from .siphon.part to target final name
        std::fs::rename(&self.part_path, &self.final_path).with_context(|| {
            format!(
                "Failed to rename {:?} to {:?}",
                self.part_path, self.final_path
            )
        })?;
        Ok(())
    }

    pub fn part_path(&self) -> &Path {
        &self.part_path
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }
}
