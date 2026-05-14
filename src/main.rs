//! daedal — OpenAI gpt-image-2 이미지 생성 CLI (단일 Rust 바이너리).
//! POST /v1/images/generations → base64 PNG → file.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const ENDPOINT: &str = "https://api.openai.com/v1/images/generations";

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Preset {
    /// 1024x1024 auto quality (default)
    Square,
    /// 1536x1024 high quality — PPT 16:9 슬라이드용 (텍스트·차트 강화)
    Slide,
    /// 1024x1536 high quality — 세로 포스터/안내문
    Poster,
    /// 1536x1024 high quality — 인포그래픽 (정보·라벨 밀도)
    Infographic,
}

impl Preset {
    fn size(self) -> &'static str {
        match self {
            Preset::Square => "1024x1024",
            Preset::Slide | Preset::Infographic => "1536x1024",
            Preset::Poster => "1024x1536",
        }
    }
    fn quality(self) -> &'static str {
        match self {
            Preset::Square => "auto",
            _ => "high",
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about = "daedal — OpenAI gpt-image-2 이미지 생성 CLI")]
struct Args {
    /// Prompt text
    prompt: String,
    /// Output path (default: <auto-dir>/daedal-<epoch>.png)
    #[arg(long, short = 'o')]
    out: Option<PathBuf>,
    /// Preset: 슬라이드/포스터/인포그래픽 자동 사이즈+품질. --size/--quality 로 override 가능
    #[arg(long, value_enum)]
    preset: Option<Preset>,
    /// Size: 1024x1024 | 1024x1536 | 1536x1024 | auto (preset override)
    #[arg(long)]
    size: Option<String>,
    /// Quality: low | medium | high | auto (preset override)
    #[arg(long)]
    quality: Option<String>,
    /// Number of images
    #[arg(long, short = 'n', default_value = "1")]
    n: u32,
    /// Model ID (snapshot pin 가능, 예: gpt-image-2-2026-04-21)
    #[arg(long, env = "DAEDAL_MODEL", default_value = "gpt-image-2")]
    model: String,
    /// Do not add daedal's quality/preset prompt wrapper
    #[arg(long)]
    raw: bool,
    /// Print only path (scripts)
    #[arg(long)]
    quiet: bool,
}

#[derive(Serialize)]
struct Req<'a> {
    model: &'a str,
    prompt: &'a str,
    size: &'a str,
    quality: &'a str,
    n: u32,
    output_format: &'a str,
}

#[derive(Deserialize)]
struct Resp {
    data: Vec<ImgData>,
    usage: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ImgData {
    b64_json: String,
}

/// Default output directory per platform.
/// Priority: DAEDAL_OUT_DIR env > termux sdcard > Windows Pictures > $HOME/Pictures/daedal > CWD.
fn default_out_dir(is_termux: bool) -> PathBuf {
    if let Ok(d) = std::env::var("DAEDAL_OUT_DIR") {
        if !d.is_empty() {
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

fn api_key() -> Result<String> {
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    bail!("OPENAI_API_KEY env var not set");
}

fn validate_choice(name: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        bail!("invalid {name}: {value} (allowed: {})", allowed.join(", "));
    }
}

fn preset_prompt_contract(preset: Option<Preset>) -> &'static str {
    match preset {
        Some(Preset::Slide) => {
            "Output type: polished 16:9 presentation slide. Use strong hierarchy: title, one central message, and structured supporting elements. Keep generous margins, clean grid alignment, high contrast, and no clutter. If charts or metrics are requested, make them visually coherent and readable."
        }
        Some(Preset::Poster) => {
            "Output type: vertical poster. Use a clear headline area, one memorable visual focal point, balanced whitespace, and legible supporting text. Make it print-poster quality with intentional typography."
        }
        Some(Preset::Infographic) => {
            "Output type: explanatory infographic. Organize information into clearly separated sections with icons, labels, arrows, or steps. Prioritize readability, spatial logic, and consistent visual language over decoration."
        }
        Some(Preset::Square) | None => {
            "Output type: high-quality single image. Use deliberate composition, coherent lighting, clean subject separation, and a finished editorial look."
        }
    }
}

fn build_prompt(user_prompt: &str, preset: Option<Preset>, raw: bool) -> String {
    if raw {
        return user_prompt.to_string();
    }

    format!(
        "{}\n\nUser request:\n{}\n\nHard requirements:\n- Render any quoted Korean/English text exactly as written, preserving spelling, numbers, punctuation, and spacing.\n- Do not add random placeholder text, watermarks, fake signatures, UI chrome, or unreadable filler letters.\n- Prefer fewer, larger text elements over many tiny labels unless the user explicitly asks for dense details.\n- Keep the image visually finished: consistent palette, coherent perspective, clean edges, and intentional layout.\n- If the prompt is ambiguous, choose a professional, realistic interpretation rather than a generic stock-image look.",
        preset_prompt_contract(preset),
        user_prompt
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let key = api_key()?;
    if args.n == 0 || args.n > 10 {
        bail!("n must be 1..=10");
    }

    let preset_size = args.preset.map(|p| p.size().to_string());
    let preset_quality = args.preset.map(|p| p.quality().to_string());
    let size = args
        .size
        .clone()
        .or(preset_size)
        .unwrap_or_else(|| "1024x1024".to_string());
    let quality = args
        .quality
        .clone()
        .or(preset_quality)
        .unwrap_or_else(|| "auto".to_string());
    validate_choice(
        "size",
        &size,
        &["1024x1024", "1024x1536", "1536x1024", "auto"],
    )?;
    validate_choice("quality", &quality, &["low", "medium", "high", "auto"])?;
    let prompt = build_prompt(&args.prompt, args.preset, args.raw);

    let req = Req {
        model: &args.model,
        prompt: &prompt,
        size: &size,
        quality: &quality,
        n: args.n,
        output_format: "png",
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()?;

    if !args.quiet {
        eprintln!(
            "[daedal] model={} size={} quality={} n={}",
            args.model, size, quality, args.n
        );
    }

    let r = client
        .post(ENDPOINT)
        .bearer_auth(&key)
        .json(&req)
        .send()
        .await
        .context("HTTP request failed")?;

    let status = r.status();
    let body = r.text().await?;
    if !status.is_success() {
        bail!("API error {}: {}", status, body);
    }

    let parsed: Resp = serde_json::from_str(&body).with_context(|| {
        format!(
            "parse response: {}",
            body.chars().take(300).collect::<String>()
        )
    })?;

    if parsed.data.is_empty() {
        bail!("empty data in response");
    }

    let is_termux = std::env::var("PREFIX")
        .map(|p| p.contains("com.termux"))
        .unwrap_or(false);
    let base: PathBuf = args.out.unwrap_or_else(|| {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let dir = default_out_dir(is_termux);
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("daedal-{}.png", ts))
    });

    for (i, d) in parsed.data.iter().enumerate() {
        let png = STANDARD.decode(&d.b64_json).context("base64 decode")?;
        let path = if parsed.data.len() == 1 {
            base.clone()
        } else {
            let stem = base
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("daedal");
            let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("png");
            base.with_file_name(format!("{}-{}.{}", stem, i, ext))
        };
        std::fs::write(&path, &png).with_context(|| format!("write {:?}", path))?;
        // Android: make gallery-visible via MediaStore broadcast when path under /sdcard/ or /storage/
        let path_str = path.to_string_lossy().to_string();
        if is_termux && (path_str.starts_with("/sdcard/") || path_str.starts_with("/storage/")) {
            let _ = std::process::Command::new("su")
                .arg("-c")
                .arg(format!("chmod 644 '{}' && am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file://{}", path_str, path_str))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        if args.quiet {
            println!("{}", path.display());
        } else {
            eprintln!("[daedal] saved {} ({} bytes)", path.display(), png.len());
        }
    }

    if !args.quiet {
        if let Some(u) = parsed.usage {
            eprintln!("[daedal] usage: {}", u);
        }
    }
    Ok(())
}
