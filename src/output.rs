//! 출력 경로 결정과 파일 쓰기.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn is_termux() -> bool {
    std::env::var("PREFIX")
        .map(|p| p.contains("com.termux"))
        .unwrap_or(false)
}

/// 기본 출력 디렉토리.
/// 우선순위: DAEDAL_OUT_DIR > termux /sdcard/DCIM > Windows %USERPROFILE%\Pictures\daedal > $HOME/Pictures/daedal > CWD
pub fn default_out_dir(is_termux: bool) -> PathBuf {
    if let Ok(d) = std::env::var("DAEDAL_OUT_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    if is_termux {
        return PathBuf::from("/sdcard/DCIM");
    }
    if cfg!(windows) {
        if let Ok(p) = std::env::var("USERPROFILE") {
            return PathBuf::from(p).join("Pictures").join("daedal");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join("Pictures").join("daedal");
    }
    PathBuf::from(".")
}

/// 자동 파일명 `daedal-<epoch ms>-<pid>.<ext>` — 같은 초에 두 프로세스가 돌아도 덮어쓰지 않는다.
pub fn default_filename(ext: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("daedal-{ms}-{}.{ext}", std::process::id())
}

/// n 장의 저장 경로. `out` 이 없으면 `<dir>/daedal-<epoch ms>-<pid>.<ext>`.
/// n > 1 이면 `<stem>-<i>.<ext>` (i 는 0부터 — 0.2.0 이후 유지되는 규약).
pub fn plan_paths(out: Option<PathBuf>, n: u32, ext: &str, is_termux: bool) -> Vec<PathBuf> {
    let base = out.unwrap_or_else(|| default_out_dir(is_termux).join(default_filename(ext)));
    if n <= 1 {
        return vec![base];
    }
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("daedal")
        .to_string();
    let ext = base
        .extension()
        .and_then(|s| s.to_str())
        .filter(|e| !e.is_empty())
        .unwrap_or(ext)
        .to_string();
    (0..n)
        .map(|i| base.with_file_name(format!("{stem}-{i}.{ext}")))
        .collect()
}

pub fn write_image(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("디렉토리 생성 실패 {}", parent.display()))?;
        }
    }
    std::fs::write(path, bytes).with_context(|| format!("파일 쓰기 실패 {}", path.display()))
}

/// Termux: 갤러리에 보이도록 미디어 스캐너를 깨운다 (실패해도 조용히 넘어간다 — 부가 기능).
pub fn termux_media_scan(path: &Path) {
    let p = path.to_string_lossy();
    if !(p.starts_with("/sdcard/") || p.starts_with("/storage/")) {
        return;
    }
    let _ = std::process::Command::new("su")
        .arg("-c")
        .arg(format!(
            "chmod 644 '{p}' && am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file://{p}"
        ))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_path_is_verbatim() {
        let p = plan_paths(Some(PathBuf::from("out/x.jpg")), 1, "png", false);
        assert_eq!(p, vec![PathBuf::from("out/x.jpg")]);
    }

    #[test]
    fn multi_paths_numbered_from_zero_keeping_user_ext() {
        let p = plan_paths(Some(PathBuf::from("dir/logo.webp")), 3, "png", false);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], PathBuf::from("dir/logo-0.webp"));
        assert_eq!(p[2], PathBuf::from("dir/logo-2.webp"));
    }

    #[test]
    fn default_name_uses_format_ext_and_is_unique() {
        let p = plan_paths(None, 1, "jpeg", false);
        let name = p[0].file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with("daedal-") && name.ends_with(".jpeg"),
            "{name}"
        );
        let a = default_filename("png");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = default_filename("png");
        assert_ne!(a, b, "밀리초 단위 이름이 같은 초 안에서 겹치면 안 된다");
        assert!(a.contains(&format!("-{}.", std::process::id())));
    }
}
