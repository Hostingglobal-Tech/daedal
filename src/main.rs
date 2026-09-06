//! daedal — OpenAI 이미지 생성 CLI (단일 Rust 바이너리).
//!
//! 경로 두 개:
//! - images:    `/v1/images/generations` 를 API 키로 직접 (OPENAI_BASE_URL 없을 때 기본).
//!   한글 텍스트 품질이 검증된 경로 — 프롬프트를 원문 그대로 보낸다.
//! - responses: `/v1/responses` + image_generation 툴 (OPENAI_BASE_URL = OAuth 프록시일 때 기본,
//!   ChatGPT 구독 quota). 툴의 model 을 gpt-image-2 로 고정한다(툴 기본값은 gpt-image-1).
//!
//! 비용: 호출 전 예상 비용을 stderr 에 찍고, 상한(--max-cost, 기본 $1)을 넘으면 --yes 없이는 멈춘다.
//! 종료코드: 0 성공 / 1 API·IO 실패 / 2 인자·검증·비용 게이트 거부.

mod api;
mod cost;
mod output;
mod size;

use api::{Api, GenRequest, Route};
use clap::{Parser, ValueEnum};
use cost::QualityTier;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const EXIT_FAIL: i32 = 1;
const EXIT_USAGE: i32 = 2;
const DEFAULT_IMAGE_MODEL: &str = "gpt-image-2";
const DEFAULT_MAINLINE: &str = "gpt-5.6-luna";
const DEFAULT_MAX_COST: f64 = 1.0;
/// Responses 툴에 model 을 안 보낼 때 백엔드가 쓰는 기본 이미지 모델 (API 레퍼런스 2026-09-06).
const TOOL_DEFAULT_IMAGE_MODEL: &str = "gpt-image-1";
/// 이 장수부터 경고. 근거: 고품질 1장 ≈ $0.17 이라 3장(≈$0.5)부터는 실수가 사소하지 않다.
const WARN_N: u32 = 3;
const DEVELOPER_PROMPT: &str = "You are an image generation assistant. Generate an image using the image_generation tool exactly matching the user request. Render any quoted text exactly as written; do not translate or paraphrase quoted Korean text.";

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Preset {
    /// 1024x1024 auto
    Square,
    /// 1536x1024 high — PPT 슬라이드(3:2). 정확한 16:9 는 slide16
    Slide,
    /// 2048x1152 high — 정확한 16:9 2K 슬라이드 (gpt-image-2 전용, 비용 ≈1.5배)
    Slide16,
    /// 1024x1536 high — 세로 포스터/안내문
    Poster,
    /// 1536x1024 high — 인포그래픽 (정보·라벨 밀도)
    Infographic,
}

impl Preset {
    fn size(self) -> &'static str {
        match self {
            Preset::Square => "1024x1024",
            Preset::Slide | Preset::Infographic => "1536x1024",
            Preset::Slide16 => "2048x1152",
            Preset::Poster => "1024x1536",
        }
    }
    fn quality(self) -> Quality {
        match self {
            Preset::Square => Quality::Auto,
            _ => Quality::High,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Quality {
    Low,
    Medium,
    High,
    Auto,
}

impl Quality {
    fn as_str(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
            Quality::Auto => "auto",
        }
    }
    fn tier(self) -> QualityTier {
        match self {
            Quality::Low => QualityTier::Low,
            Quality::Medium => QualityTier::Medium,
            Quality::High => QualityTier::High,
            Quality::Auto => QualityTier::Auto,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Backend {
    /// OPENAI_BASE_URL(프록시)이 있으면 responses, 없으면 images
    Auto,
    Images,
    Responses,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::Webp => "webp",
        }
    }
    /// `-o` 확장자로 형식을 추론한다. 모르는 확장자는 None.
    fn from_ext(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "webp" => Some(Format::Webp),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Background {
    Auto,
    Opaque,
    /// 투명 배경 (png/webp 만, gpt-image-2 preview)
    Transparent,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Moderation {
    Auto,
    Low,
}

/// bool env/플래그 값. 빈값 = 끔, 1/true/t/yes/y/on = 켬, 0/false/f/no/n/off = 끔, **그 밖은 오류**
/// (비용 게이트 승인값이 오타·뒤공백으로 "켬" 이 되면 안 되기 때문이다).
fn parse_bool_env(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "f" | "no" | "n" | "off" => Ok(false),
        "1" | "true" | "t" | "yes" | "y" | "on" => Ok(true),
        other => Err(format!(
            "'{other}' 는 bool 이 아니다 (1/true/yes/on 또는 0/false/no/off/빈값)"
        )),
    }
}

/// `--max-cost` / DAEDAL_MAX_COST_USD: 빈값 = 기본 $1, NaN·음수 거부 (NaN 은 비교가 늘 false 라 게이트가 조용히 풀린다), inf 허용.
fn parse_max_cost(s: &str) -> Result<f64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(DEFAULT_MAX_COST);
    }
    let v: f64 = t.parse().map_err(|_| format!("'{t}' 는 수가 아니다"))?;
    if v.is_nan() || v < 0.0 {
        return Err(format!("{v}: 0 이상의 수여야 한다 (inf = 무제한)"));
    }
    Ok(v)
}

/// `--backend` / DAEDAL_BACKEND: 빈값 = auto, 대소문자 무관.
fn parse_backend(s: &str) -> Result<Backend, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Ok(Backend::Auto),
        "images" => Ok(Backend::Images),
        "responses" => Ok(Backend::Responses),
        other => Err(format!("'{other}': auto | images | responses 중 하나")),
    }
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "daedal — OpenAI 이미지 생성 CLI (기본 gpt-image-2)",
    after_help = "종료코드 0 성공 · 1 API/IO 실패 · 2 인자·검증·비용게이트 거부\n\
환경변수: OPENAI_API_KEY · OPENAI_BASE_URL(프록시) · OPENCODEX_API_AUTH_TOKEN(사설 프록시 자격)\n\
          DAEDAL_MODEL · DAEDAL_MAINLINE_MODEL · DAEDAL_BACKEND · DAEDAL_NO_TOOL_MODEL · DAEDAL_ENHANCE\n\
          DAEDAL_MAX_COST_USD · DAEDAL_YES · DAEDAL_OUT_DIR · DAEDAL_PRICE_IMAGE_OUT_PER_M · DAEDAL_PRICE_TEXT_IN_PER_M\n\
bool 값: 1/true/yes/on = 켬 · 0/false/no/off/빈값 = 끔 · 그 밖은 오류. CLI 로 끄려면 --yes=false 처럼 '=' 로 준다"
)]
struct Args {
    /// 프롬프트 (원문 그대로 전송. 품질 계약을 덧붙이려면 --enhance)
    prompt: Option<String>,
    /// 출력 경로 (기본: <자동 디렉토리>/daedal-<epoch ms>-<pid>.<format>). 확장자가 없으면 붙이고, 디렉토리면 그 안에 자동 이름으로 저장
    #[arg(long, short = 'o')]
    out: Option<PathBuf>,
    /// 프리셋 (크기+품질). --size/--quality 로 덮어쓸 수 있다
    #[arg(long, value_enum, ignore_case = true)]
    preset: Option<Preset>,
    /// 크기: 1024x1024 | 1536x1024 | 1024x1536 | 2048x2048 | 2048x1152 | 3840x2160 | 2160x3840 | auto
    /// 또는 임의 WxH (gpt-image-2: 16 배수, 긴 변 ≤3840, 비율 ≤3:1, 0.66~8.3Mpx)
    #[arg(long)]
    size: Option<String>,
    /// 품질
    #[arg(long, value_enum, ignore_case = true)]
    quality: Option<Quality>,
    /// 장수 (1..=10). 각 장은 별도 요청이라 먼저 나온 것부터 저장된다
    #[arg(long, short = 'n', default_value_t = 1)]
    n: u32,
    /// 이미지 모델 (기본 gpt-image-2; 스냅샷 예: gpt-image-2-2026-04-21)
    #[arg(long, env = "DAEDAL_MODEL", default_value = DEFAULT_IMAGE_MODEL)]
    model: String,
    /// Responses 경로에서 image_generation 툴을 부르는 텍스트 모델
    #[arg(long, env = "DAEDAL_MAINLINE_MODEL", default_value = DEFAULT_MAINLINE)]
    mainline: String,
    /// 호출 경로: auto | images | responses (auto = 프록시 있으면 responses)
    #[arg(long, env = "DAEDAL_BACKEND", default_value = "auto", value_parser = parse_backend)]
    backend: Backend,
    /// Responses 경로에서 툴의 model 필드를 보내지 않는다 (프록시가 거부할 때). 백엔드 기본값 gpt-image-1 로 간주해 검증·단가를 잡는다
    #[arg(long, env = "DAEDAL_NO_TOOL_MODEL", action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true,
          default_missing_value = "true", default_value = "false", hide_default_value = true, value_parser = parse_bool_env)]
    no_tool_model: bool,
    /// 출력 형식 (생략하면 -o 확장자에서 추론, 아니면 png)
    #[arg(long, value_enum, ignore_case = true)]
    format: Option<Format>,
    /// 배경
    #[arg(long, value_enum, ignore_case = true, default_value = "auto")]
    background: Background,
    /// jpeg/webp 압축률 0~100 (기본 100)
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    compression: Option<u8>,
    /// 콘텐츠 검열 강도 (auto 기본 = 필드 생략)
    #[arg(long, value_enum, ignore_case = true, default_value = "auto")]
    moderation: Moderation,
    /// 프리셋 품질 계약 + 텍스트 정확도 요구를 프롬프트에 덧붙인다 (실운영 미검증 — 기본 꺼짐)
    #[arg(long, env = "DAEDAL_ENHANCE", action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true,
          default_missing_value = "true", default_value = "false", hide_default_value = true, value_parser = parse_bool_env)]
    enhance: bool,
    /// --enhance / DAEDAL_ENHANCE 를 끈다 (원문 전송 강제). 0.3.0 호환용
    #[arg(long, hide = true)]
    raw: bool,
    /// 1회 실행 예상 비용 상한 (USD). 넘으면 --yes 없이는 중단. inf = 무제한
    #[arg(long, env = "DAEDAL_MAX_COST_USD", default_value = "1.0", value_parser = parse_max_cost)]
    max_cost: f64,
    /// 비용 상한 초과를 승인한다 (env 를 CLI 로 끄려면 --yes=false)
    #[arg(long, short = 'y', env = "DAEDAL_YES", action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true,
          default_missing_value = "true", default_value = "false", hide_default_value = true, value_parser = parse_bool_env)]
    yes: bool,
    /// 요청 본문과 예상 비용만 출력하고 호출하지 않는다 (무료 — 비용 게이트에 막히지 않는다)
    #[arg(long)]
    dry_run: bool,
    /// GET /v1/models 로 사용 가능한 이미지 모델을 나열한다 (무료)
    #[arg(long)]
    list_models: bool,
    /// stdout 에 저장 경로만 출력 (스크립트용). 경고·오류는 stderr 로 그대로 나간다
    #[arg(long)]
    quiet: bool,
}

struct Fail {
    code: i32,
    msg: String,
}

fn usage_err(msg: impl Into<String>) -> Fail {
    Fail {
        code: EXIT_USAGE,
        msg: msg.into(),
    }
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Fail {
        Fail {
            code: EXIT_FAIL,
            msg: format!("{e:#}"),
        }
    }
}

fn preset_prompt_contract(preset: Option<Preset>) -> &'static str {
    match preset {
        Some(Preset::Slide) | Some(Preset::Slide16) => {
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

/// 기본은 원문 그대로. `enhance` 일 때만 계약을 덧붙인다 (0.3.0 의 래퍼 문안 그대로 보존).
fn build_prompt(user_prompt: &str, preset: Option<Preset>, enhance: bool) -> String {
    if !enhance {
        return user_prompt.to_string();
    }
    format!(
        "{}\n\nUser request:\n{}\n\nHard requirements:\n- Render any quoted Korean/English text exactly as written, preserving spelling, numbers, punctuation, and spacing.\n- Do not add random placeholder text, watermarks, fake signatures, UI chrome, or unreadable filler letters.\n- Prefer fewer, larger text elements over many tiny labels unless the user explicitly asks for dense details.\n- Keep the image visually finished: consistent palette, coherent perspective, clean edges, and intentional layout.\n- If the prompt is ambiguous, choose a professional, realistic interpretation rather than a generic stock-image look.",
        preset_prompt_contract(preset),
        user_prompt
    )
}

/// `-o` 가 디렉토리인가: 끝이 구분자이거나 실재하는 폴더.
fn looks_like_dir(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.ends_with('/') || s.ends_with('\\') || p.is_dir()
}

/// 출력 형식과 경로를 맞춘다: 형식 생략 시 확장자에서 추론, 확장자 없으면 붙이고, 디렉토리면 그 안에 자동 이름.
/// 반환: (형식, 경로, 경고들)
fn resolve_format_and_out(
    format: Option<Format>,
    out: Option<PathBuf>,
) -> (Format, Option<PathBuf>, Vec<String>) {
    let mut warns = Vec::new();
    let Some(p) = out else {
        return (format.unwrap_or(Format::Png), None, warns);
    };
    if looks_like_dir(&p) {
        let f = format.unwrap_or(Format::Png);
        return (f, Some(p.join(output::default_filename(f.as_str()))), warns);
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    match (format, ext) {
        (Some(f), Some(e)) => {
            match Format::from_ext(&e) {
                Some(fe) if fe != f => warns.push(format!(
                    "[daedal] 경고: -o 확장자 .{e} 와 --format {} 이 다르다 — 파일 내용은 {} 이다",
                    f.as_str(),
                    f.as_str()
                )),
                None => warns.push(format!(
                    "[daedal] 경고: -o 확장자 .{e} 는 이미지 형식이 아니다 — 파일 내용은 {}",
                    f.as_str()
                )),
                _ => {}
            }
            (f, Some(p), warns)
        }
        (Some(f), None) => (f, Some(p.with_extension(f.as_str())), warns),
        (None, Some(e)) => match Format::from_ext(&e) {
            Some(fe) => (fe, Some(p), warns),
            None => {
                warns.push(format!(
                    "[daedal] 경고: -o 확장자 .{e} 를 모른다 — png 로 생성한다"
                ));
                (Format::Png, Some(p), warns)
            }
        },
        (None, None) => (Format::Png, Some(p.with_extension("png")), warns),
    }
}

/// 생성 대기 중 경과 시간을 stderr 로 알린다 (TTY 5초, 아니면 15초 간격).
fn spawn_ticker(label: String) -> tokio::task::JoinHandle<()> {
    let step = if std::io::stderr().is_terminal() {
        5
    } else {
        15
    };
    tokio::spawn(async move {
        let start = Instant::now();
        let mut iv = tokio::time::interval(Duration::from_secs(step));
        iv.tick().await;
        loop {
            iv.tick().await;
            eprintln!(
                "[daedal] {label} 생성 중… {}초 경과",
                start.elapsed().as_secs()
            );
        }
    })
}

fn request_timeout(pixels: u64) -> Duration {
    // 1.5Mpx high 가 보통 20~90초. 2K/4K 는 여유를 더 준다.
    if pixels > 2_500_000 {
        Duration::from_secs(480)
    } else {
        Duration::from_secs(300)
    }
}

async fn run(args: Args) -> Result<(), Fail> {
    let quiet = args.quiet;
    let info = |s: &str| {
        if !quiet {
            eprintln!("{s}");
        }
    };
    let warn = |s: &str| eprintln!("{s}");

    let (base, proxy) = api::base_url_from_env();
    api::validate_base(&base).map_err(usage_err)?;
    let auth = api::auth_from_env(&base, proxy);
    if let Some(n) = &auth.note {
        warn(n);
    }
    let key_missing = !proxy && auth.bearer.is_none();
    let need_key = || -> Result<(), Fail> {
        if key_missing {
            return Err(Fail {
                code: EXIT_FAIL,
                msg: "OPENAI_API_KEY 가 없다 (OPENAI_BASE_URL 프록시도 설정되지 않음)".into(),
            });
        }
        Ok(())
    };

    if args.list_models {
        if args.dry_run {
            info(&format!(
                "[daedal] dry-run: GET {} 생략 (무료 조회지만 호출하지 않음)",
                api::display_base(&format!("{base}/models"))
            ));
            return Ok(());
        }
        need_key()?;
        let api = Api::new(&base, auth.bearer.clone(), Duration::from_secs(30))?;
        let models = api.list_models().await?;
        let mut shown = 0;
        for (id, created) in &models {
            if id.contains("image") || id.starts_with("dall-e") {
                println!("{id}\tcreated={created}");
                shown += 1;
            }
        }
        info(&format!(
            "[daedal] 이미지 계열 {shown}개 / 전체 {}개 ({})",
            models.len(),
            api::display_base(&api.endpoint("models"))
        ));
        return Ok(());
    }

    let user_prompt = args
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| usage_err("프롬프트가 비어 있다 (--list-models 가 아니면 필수)"))?;
    if !(1..=10).contains(&args.n) {
        return Err(usage_err(format!("n={} — 1..=10 만 허용", args.n)));
    }
    let max_cost = args.max_cost;

    // 빈 env(`DAEDAL_MODEL=`)는 "미설정" 으로 본다 — env 파일에 빈 줄이 남아도 도구가 죽지 않게
    let mut image_model = args.model.trim().to_string();
    if image_model.is_empty() {
        warn(&format!(
            "[daedal] 경고: --model/DAEDAL_MODEL 이 비어 있어 기본값 {DEFAULT_IMAGE_MODEL} 사용"
        ));
        image_model = DEFAULT_IMAGE_MODEL.to_string();
    }
    let mut mainline = args.mainline.trim().to_string();
    if mainline.is_empty() {
        warn(&format!("[daedal] 경고: --mainline/DAEDAL_MAINLINE_MODEL 이 비어 있어 기본값 {DEFAULT_MAINLINE} 사용"));
        mainline = DEFAULT_MAINLINE.to_string();
    }

    let route = match args.backend {
        Backend::Images => Route::Images,
        Backend::Responses => Route::Responses,
        Backend::Auto => {
            if proxy {
                Route::Responses
            } else {
                Route::Images
            }
        }
    };
    let omit_tool_model = route == Route::Responses && args.no_tool_model;
    if args.no_tool_model && route == Route::Images {
        warn("[daedal] 경고: --no-tool-model 은 responses 경로에만 의미가 있다 — 무시함");
    }
    // 툴 model 을 생략하면 백엔드 기본값(gpt-image-1)이 그린다 — 검증·단가도 그 기준이어야 맞다
    let effective_model: &str = if omit_tool_model {
        TOOL_DEFAULT_IMAGE_MODEL
    } else {
        image_model.as_str()
    };
    if omit_tool_model {
        warn(&format!(
            "[daedal] 경고: --no-tool-model — 툴 기본 모델 {TOOL_DEFAULT_IMAGE_MODEL} 기준으로 크기 검증·단가를 잡는다 (2K/4K 불가, $40/M)"
        ));
    }

    let size = args
        .size
        .as_deref()
        .map(str::trim)
        .map(str::to_string)
        .or_else(|| args.preset.map(|p| p.size().to_string()))
        .unwrap_or_else(|| "1024x1024".to_string());
    let quality = args
        .quality
        .or_else(|| args.preset.map(Preset::quality))
        .unwrap_or(Quality::Auto);
    size::validate_size(&size, effective_model).map_err(usage_err)?;
    let (format, out, fmt_warns) = resolve_format_and_out(args.format, args.out.clone());
    for w in &fmt_warns {
        warn(w);
    }
    if args.background == Background::Transparent && format == Format::Jpeg {
        return Err(usage_err(
            "--background transparent 는 png/webp 만 가능 (jpeg 는 투명 불가)",
        ));
    }
    if args.compression.is_some() && format == Format::Png {
        return Err(usage_err("--compression 은 jpeg/webp 에만 적용된다"));
    }

    let enhance = args.enhance && !args.raw;
    let prompt = build_prompt(user_prompt, args.preset, enhance);
    let gen = GenRequest {
        image_model: if omit_tool_model {
            None
        } else {
            Some(image_model.as_str())
        },
        mainline: &mainline,
        prompt: &prompt,
        developer: DEVELOPER_PROMPT,
        size: &size,
        quality: quality.as_str(),
        format: format.as_str(),
        background: match args.background {
            Background::Auto => None,
            Background::Opaque => Some("opaque"),
            Background::Transparent => Some("transparent"),
        },
        compression: args.compression,
        moderation: match args.moderation {
            Moderation::Auto => None,
            Moderation::Low => Some("low"),
        },
        reasoning_effort: "low",
    };

    // ── 비용 예측 · 경고 · 게이트 ──
    let dims = size::dims_for_estimate(&size);
    let est = cost::estimate(
        effective_model,
        dims.pixels(),
        quality.tier(),
        prompt.chars().count(),
        args.n,
        if route == Route::Responses {
            Some(mainline.as_str())
        } else {
            None
        },
    );
    let per = if est.is_range() {
        format!("{}~{}", cost::usd(est.per_low), cost::usd(est.per_high))
    } else {
        cost::usd(est.per_high)
    };
    let total = if est.is_range() {
        format!(
            "{}~{}",
            cost::usd(est.total_low()),
            cost::usd(est.total_high())
        )
    } else {
        cost::usd(est.total_high())
    };
    info(&format!(
        "[daedal] model={} size={size} quality={} n={} format={} backend={} auth={} endpoint={}/{}",
        if omit_tool_model {
            format!("(툴 기본값 {TOOL_DEFAULT_IMAGE_MODEL} 가정)")
        } else {
            image_model.clone()
        },
        quality.as_str(),
        args.n,
        format.as_str(),
        route.name(),
        auth.mode.label(),
        api::display_base(&base),
        if route == Route::Images {
            "images/generations"
        } else {
            "responses"
        },
    ));
    info(&format!(
        "[daedal] 예상 비용: 1장 ≈ {per} × {}장 = {total}  [{}]",
        args.n, est.basis
    ));
    if args.n >= WARN_N {
        warn(&format!(
            "[daedal] 경고: n={} — 유료 요청 {}회, 예상 총 상한 {}. 필요한 장수인지 확인하라 (근거: 고품질 1장 ≈ $0.17, 3장부터 실수 비용이 $0.5 를 넘는다)",
            args.n,
            args.n,
            cost::usd(est.total_high())
        ));
    }
    let over_cap = est.total_high() > max_cost;

    let body = match route {
        Route::Images => api::images_body(&gen),
        Route::Responses => api::responses_body(&gen),
    };
    if args.dry_run {
        // 무료 경로 — 게이트 판정은 알려주되 막지 않는다
        if key_missing {
            warn("[daedal] 경고: OPENAI_API_KEY 가 없다 — dry-run 은 되지만 실제 실행은 rc=1 로 실패한다");
        }
        info(&format!(
            "[daedal] dry-run: 호출하지 않음. 비용 게이트 = {} (상한 {} vs 한도 {}). 요청 본문 ↓",
            if over_cap && !args.yes {
                "차단 예정(--yes 필요)"
            } else {
                "통과"
            },
            cost::usd(est.total_high()),
            cost::usd(max_cost)
        ));
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );
        return Ok(());
    }
    if over_cap {
        if !args.yes {
            return Err(usage_err(format!(
                "비용 게이트: 예상 상한 {} > 한도 {} (--max-cost / DAEDAL_MAX_COST_USD). 진행하려면 --yes",
                cost::usd(est.total_high()),
                cost::usd(max_cost)
            )));
        }
        warn(&format!(
            "[daedal] 비용 상한 초과를 --yes 로 승인함: 예상 상한 {} > 한도 {}",
            cost::usd(est.total_high()),
            cost::usd(max_cost)
        ));
    }

    // ── 호출 ──
    need_key()?;
    let api = Api::new(&base, auth.bearer.clone(), request_timeout(dims.pixels()))?;
    let prices = cost::Prices::for_model(effective_model);
    let is_termux = output::is_termux();
    let paths = output::plan_paths(out, args.n, format.as_str(), is_termux);
    let mut spent = 0.0_f64;
    let mut saved: Vec<PathBuf> = Vec::new();

    for (i, path) in paths.iter().enumerate() {
        let label = if args.n > 1 {
            format!("{}/{}", i + 1, args.n)
        } else {
            String::new()
        };
        let ticker = if quiet {
            None
        } else {
            Some(spawn_ticker(label.clone()))
        };
        let started = Instant::now();
        let result = api.generate(route, &gen, &warn).await;
        if let Some(t) = ticker {
            t.abort();
        }
        let g = match result {
            Ok(g) => g,
            Err(e) => {
                if !saved.is_empty() {
                    warn(&format!(
                        "[daedal] {}장 저장 후 실패 (누적 실비용 {}): {}",
                        saved.len(),
                        cost::usd(spent),
                        saved
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                return Err(Fail {
                    code: EXIT_FAIL,
                    msg: format!("{label} {e:#}").trim().to_string(),
                });
            }
        };
        output::write_image(path, &g.bytes)?;
        if is_termux {
            output::termux_media_scan(path);
        }
        saved.push(path.clone());

        if let Some(u) = g.usage.as_ref().and_then(cost::Usage::from_value) {
            match route {
                Route::Images => {
                    let c = cost::actual_cost(prices, &u);
                    spent += c;
                    info(&format!(
                        "[daedal] 실비용: {} (input {} tok × ${}/M + output {} tok × ${}/M) · 누적 {} · {}초",
                        cost::usd(c),
                        cost::fmt_tok(u.input_tokens),
                        prices.text_in,
                        cost::fmt_tok(u.output_tokens),
                        prices.image_out,
                        cost::usd(spent),
                        started.elapsed().as_secs()
                    ));
                }
                Route::Responses => {
                    spent += est.per_high;
                    info(&format!(
                        "[daedal] usage(메인라인 {mainline}): input {} / output {} tok — 이미지 툴 토큰은 usage 에 안 잡힌다(확인 필요). 예상 상한 {} 로 누적 {} · {}초",
                        cost::fmt_tok(u.input_tokens),
                        cost::fmt_tok(u.output_tokens),
                        cost::usd(est.per_high),
                        cost::usd(spent),
                        started.elapsed().as_secs()
                    ));
                }
            }
        } else {
            spent += est.per_high;
            warn(&format!(
                "[daedal] 경고: 응답에 usage 가 없다 — 예상 상한 {} 로 계상 (누적 {})",
                cost::usd(est.per_high),
                cost::usd(spent)
            ));
        }

        if quiet {
            println!("{}", path.display());
        } else {
            eprintln!(
                "[daedal] saved {} ({} bytes)",
                path.display(),
                g.bytes.len()
            );
            if let Some(rp) = g.revised_prompt.as_deref().filter(|s| !s.is_empty()) {
                let preview: String = rp.chars().take(120).collect();
                eprintln!("[daedal] revised_prompt: {preview}");
            }
        }
    }

    if args.n > 1 {
        info(&format!(
            "[daedal] 완료: {}장, 실비용 합계 {}",
            saved.len(),
            cost::usd(spent)
        ));
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(f) = run(args).await {
        eprintln!("[daedal] 오류: {}", api::redact(&f.msg));
        std::process::exit(f.code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// env 를 만지는 테스트와 env 를 읽는 파서 테스트가 병렬로 돌면 경합한다 — 직렬화
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn raw_prompt_is_default_and_enhance_keeps_user_text_verbatim() {
        let p = "제목 '2026 Q2 실적' 한글 헤딩";
        assert_eq!(build_prompt(p, Some(Preset::Slide), false), p);
        let e = build_prompt(p, Some(Preset::Infographic), true);
        assert!(
            e.contains(p)
                && e.starts_with("Output type: explanatory infographic")
                && e.contains("Hard requirements")
        );
    }

    #[test]
    fn presets_map_to_documented_sizes() {
        assert_eq!(
            (Preset::Slide.size(), Preset::Slide.quality()),
            ("1536x1024", Quality::High)
        );
        assert_eq!(
            (Preset::Slide16.size(), Preset::Slide16.quality()),
            ("2048x1152", Quality::High)
        );
        assert_eq!(
            (Preset::Poster.size(), Preset::Poster.quality()),
            ("1024x1536", Quality::High)
        );
        assert_eq!(
            (Preset::Square.size(), Preset::Square.quality()),
            ("1024x1024", Quality::Auto)
        );
        for p in [
            Preset::Square,
            Preset::Slide,
            Preset::Slide16,
            Preset::Poster,
            Preset::Infographic,
        ] {
            assert!(size::validate_size(p.size(), DEFAULT_IMAGE_MODEL).is_ok());
        }
    }

    #[test]
    fn cli_parses_defaults_and_case_insensitive_enums() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = Args::try_parse_from(["daedal", "hello"]).unwrap();
        assert_eq!(a.n, 1);
        assert_eq!(a.backend, Backend::Auto);
        assert_eq!(a.format, None);
        assert!((a.max_cost - 1.0).abs() < 1e-9);
        assert!(!a.yes && !a.enhance && !a.no_tool_model);
        assert!(Args::try_parse_from(["daedal", "x", "--compression", "101"]).is_err());
        assert!(Args::try_parse_from(["daedal", "x", "--preset", "slide16", "--raw"]).is_ok());
        let a = Args::try_parse_from(["daedal", "x", "--backend", "Images", "--quality", "HIGH"])
            .unwrap();
        assert_eq!(a.backend, Backend::Images);
        assert_eq!(a.quality, Some(Quality::High));
        assert!(Args::try_parse_from(["daedal", "x", "--max-cost", "nan"]).is_err());
        assert!(Args::try_parse_from(["daedal", "x", "--max-cost", "-1"]).is_err());
        assert!(Args::try_parse_from(["daedal", "x", "--max-cost", "inf"])
            .unwrap()
            .max_cost
            .is_infinite());
        // 플래그 뒤의 프롬프트를 값으로 삼키지 않는다 (require_equals)
        let a = Args::try_parse_from(["daedal", "--yes", "슬라이드"]).unwrap();
        assert!(a.yes);
        assert_eq!(a.prompt.as_deref(), Some("슬라이드"));
        assert!(
            !Args::try_parse_from(["daedal", "--yes=false", "x"])
                .unwrap()
                .yes
        );
        assert!(Args::try_parse_from(["daedal", "-y", "x"]).unwrap().yes);
        assert!(Args::try_parse_from(["daedal", "--yes=maybe", "x"]).is_err());
    }

    #[test]
    fn bool_and_number_env_parsers() {
        for v in ["1", "true", "TRUE", "yes", "on", " y "] {
            assert_eq!(parse_bool_env(v), Ok(true), "{v:?}");
        }
        for v in ["", "0", "false", "no", "off", "0 ", " n"] {
            assert_eq!(parse_bool_env(v), Ok(false), "{v:?}");
        }
        for v in ["maybe", "tru", "2", "yes please"] {
            assert!(parse_bool_env(v).is_err(), "{v:?}");
        }
        assert_eq!(parse_max_cost(""), Ok(DEFAULT_MAX_COST));
        assert_eq!(parse_max_cost(" 2.5 "), Ok(2.5));
        assert!(
            parse_max_cost("nan").is_err()
                && parse_max_cost("-0.1").is_err()
                && parse_max_cost("abc").is_err()
        );
        assert_eq!(parse_backend(""), Ok(Backend::Auto));
        assert_eq!(parse_backend(" Responses "), Ok(Backend::Responses));
        assert!(parse_backend("proxy").is_err());
    }

    #[test]
    fn env_bool_flags_accept_boolish_values_and_reject_junk() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 한 테스트 안에서 순차 실행 — 다른 테스트는 DAEDAL_YES 를 읽지 않는다.
        let cases = [
            ("1", true),
            ("true", true),
            ("yes", true),
            ("on", true),
            ("0", false),
            ("0 ", false),
            ("false", false),
            ("", false),
        ];
        for (v, expect) in cases {
            std::env::set_var("DAEDAL_YES", v);
            let a = Args::try_parse_from(["daedal", "x"])
                .unwrap_or_else(|e| panic!("DAEDAL_YES={v:?}: {e}"));
            assert_eq!(a.yes, expect, "DAEDAL_YES={v:?}");
        }
        for v in ["maybe", "tru"] {
            std::env::set_var("DAEDAL_YES", v);
            assert!(
                Args::try_parse_from(["daedal", "x"]).is_err(),
                "DAEDAL_YES={v:?} 는 오류여야 한다"
            );
        }
        std::env::set_var("DAEDAL_YES", "1");
        assert!(
            !Args::try_parse_from(["daedal", "--yes=false", "x"])
                .unwrap()
                .yes,
            "CLI 가 env 를 이겨야 한다"
        );
        std::env::remove_var("DAEDAL_YES");
        assert!(!Args::try_parse_from(["daedal", "x"]).unwrap().yes);
    }

    #[test]
    fn format_inference_and_out_path_rules() {
        let (f, o, w) = resolve_format_and_out(None, Some(PathBuf::from("a/b.jpg")));
        assert_eq!(
            (f, o.unwrap(), w.is_empty()),
            (Format::Jpeg, PathBuf::from("a/b.jpg"), true)
        );
        let (f, o, _) = resolve_format_and_out(Some(Format::Webp), Some(PathBuf::from("a/noext")));
        assert_eq!(
            (f, o.unwrap()),
            (Format::Webp, PathBuf::from("a/noext.webp"))
        );
        let (f, o, w) = resolve_format_and_out(Some(Format::Png), Some(PathBuf::from("x.jpg")));
        assert_eq!((f, o.unwrap()), (Format::Png, PathBuf::from("x.jpg")));
        assert!(w[0].contains("다르다"));
        let (f, _, w) = resolve_format_and_out(None, Some(PathBuf::from("x.bmp")));
        assert_eq!(f, Format::Png);
        assert!(!w.is_empty());
        let (f, o, _) = resolve_format_and_out(None, None);
        assert_eq!((f, o), (Format::Png, None));
        // 끝 점은 확장자 없음으로 본다
        let (_, o, _) = resolve_format_and_out(None, Some(PathBuf::from("name.")));
        assert_eq!(o.unwrap(), PathBuf::from("name.png"));
        // 디렉토리(끝 구분자 또는 실재 폴더)면 그 안에 자동 이름
        let (_, o, _) = resolve_format_and_out(None, Some(PathBuf::from("out/")));
        let o = o.unwrap();
        assert!(
            o.starts_with("out")
                && o.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("daedal-"),
            "{o:?}"
        );
        let tmp = std::env::temp_dir();
        let (_, o, _) = resolve_format_and_out(Some(Format::Jpeg), Some(tmp.clone()));
        let o = o.unwrap();
        assert!(
            o.starts_with(&tmp) && o.extension().unwrap() == "jpeg",
            "{o:?}"
        );
        // 대문자 확장자
        let (f, _, w) = resolve_format_and_out(None, Some(PathBuf::from("X.PNG")));
        assert_eq!((f, w.is_empty()), (Format::Png, true));
    }
}
