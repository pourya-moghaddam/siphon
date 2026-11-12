use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCategory {
    Archives,
    Videos,
    Music,
    Documents,
    Programs,
    Images,
    General,
}

impl FileCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileCategory::Archives => "Archives",
            FileCategory::Videos => "Videos",
            FileCategory::Music => "Music",
            FileCategory::Documents => "Documents",
            FileCategory::Programs => "Programs",
            FileCategory::Images => "Images",
            FileCategory::General => "General",
        }
    }

    pub fn subfolder(&self) -> &'static str {
        match self {
            FileCategory::Archives => "archives",
            FileCategory::Videos => "videos",
            FileCategory::Music => "music",
            FileCategory::Documents => "documents",
            FileCategory::Programs => "programs",
            FileCategory::Images => "images",
            FileCategory::General => "downloads",
        }
    }
}

pub fn categorize_file(filename: &str) -> FileCategory {
    let path = Path::new(filename);
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "zip" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "rar" | "iso" | "tgz" => {
            FileCategory::Archives
        }
        "mp4" | "mkv" | "avi" | "webm" | "mov" | "flv" | "wmv" | "m4v" => FileCategory::Videos,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" => FileCategory::Music,
        "pdf" | "epub" | "docx" | "doc" | "xlsx" | "xls" | "pptx" | "ppt" | "txt" | "md" => {
            FileCategory::Documents
        }
        "deb" | "rpm" | "apk" | "exe" | "msi" | "appimage" | "pkg" | "dmg" | "run" => {
            FileCategory::Programs
        }
        "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "bmp" | "ico" => FileCategory::Images,
        _ => FileCategory::General,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categorize_files() {
        assert_eq!(categorize_file("ubuntu.iso"), FileCategory::Archives);
        assert_eq!(categorize_file("movie.mkv"), FileCategory::Videos);
        assert_eq!(categorize_file("song.flac"), FileCategory::Music);
        assert_eq!(categorize_file("manual.pdf"), FileCategory::Documents);
        assert_eq!(categorize_file("installer.deb"), FileCategory::Programs);
        assert_eq!(categorize_file("photo.png"), FileCategory::Images);
        assert_eq!(categorize_file("unknown.xyz"), FileCategory::General);
    }
}
