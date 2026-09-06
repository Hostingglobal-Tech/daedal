//! 비용 추정과 실비용 계산.
//!
//! 단가 (USD / 1M tokens) — developers.openai.com/api/docs/pricing, 2026-09-06 확인:
//!   gpt-image-2      text-in 5.00  image-in 8.00   image-out 30.00
//!   gpt-image-1.5    text-in 5.00  image-in 8.00   image-out 32.00
//!   gpt-image-1      text-in 5.00  image-in 10.00  image-out 40.00
//!   gpt-image-1-mini text-in 2.00  image-in 2.50   image-out 8.00
//! env 로 덮어쓴다 (재빌드 없이 단가 변경): DAEDAL_PRICE_TEXT_IN_PER_M / DAEDAL_PRICE_IMAGE_OUT_PER_M
//!
//! 출력 토큰 모델
//! - gpt-image-2: 실사용 로그 실측(2026-09-04~06).
//!   1536x1024 high = 5,488 (3회 동일; n=2 는 정확히 10,976) · 1024x1536 high = 5,488 (2회)
//!   1536x1024 auto = 343 ~ 1,372 (6회) · 1024x1024 auto = 196 (1회)
//!   → high 는 픽셀당 고정: 5,488 / 1.573 Mpx = 3,489 tok/Mpx.
//! - gpt-image-1 계열: 공식 모델 페이지 단가표 (1536x1024: low $0.016 / medium $0.063 / high $0.25,
//!   $40/M) → high 6,250 tok = 3,974 tok/Mpx, medium/high = 0.252, low/high = 0.064.
//!   gpt-image-1.5 · 1-mini 는 토큰표가 없어 gpt-image-1 과 같은 토큰으로 본다(단가만 다름).
//! - medium·low 는 gpt-image-2 실측이 없어 위 비율을 gpt-image-2 에도 적용한다 — 추정치다.
//! - auto 는 모델이 품질을 고르므로 범위 = low ~ high 로 잡는다. 게이트는 상한(high)으로 판정한다.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub text_in: f64,
    pub image_in: f64,
    pub image_out: f64,
}

impl Prices {
    pub fn for_model(model: &str) -> Prices {
        let mut p = if model.starts_with("gpt-image-1-mini") {
            Prices {
                text_in: 2.0,
                image_in: 2.5,
                image_out: 8.0,
            }
        } else if model.starts_with("gpt-image-1.5") {
            Prices {
                text_in: 5.0,
                image_in: 8.0,
                image_out: 32.0,
            }
        } else if model.starts_with("gpt-image-1") {
            Prices {
                text_in: 5.0,
                image_in: 10.0,
                image_out: 40.0,
            }
        } else {
            // gpt-image-2 및 미지의 모델 — 최신 모델 단가로 본다
            Prices {
                text_in: 5.0,
                image_in: 8.0,
                image_out: 30.0,
            }
        };
        if let Some(v) = env_f64("DAEDAL_PRICE_TEXT_IN_PER_M") {
            p.text_in = v;
        }
        if let Some(v) = env_f64("DAEDAL_PRICE_IMAGE_OUT_PER_M") {
            p.image_out = v;
        }
        p
    }
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// gpt-image-2 high 출력 토큰 / Mpx (실측 5,488 tok @ 1,572,864 px).
pub const TOK_PER_MPX_HIGH_V2: f64 = 5_488.0 / 1.572_864;
/// gpt-image-1 high 출력 토큰 / Mpx (공식 단가표 $0.25 @ $40/M = 6,250 tok @ 1,572,864 px).
pub const TOK_PER_MPX_HIGH_V1: f64 = 6_250.0 / 1.572_864;
pub const MEDIUM_RATIO: f64 = 0.252;
pub const LOW_RATIO: f64 = 0.064;

/// 모델별 high 토큰 계수.
pub fn tok_per_mpx_high(model: &str) -> f64 {
    if model.starts_with("gpt-image-1") {
        TOK_PER_MPX_HIGH_V1
    } else {
        TOK_PER_MPX_HIGH_V2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    Low,
    Medium,
    High,
    Auto,
}

/// (하한, 상한) 이미지 출력 토큰.
pub fn image_out_tokens(model: &str, pixels: u64, q: QualityTier) -> (u64, u64) {
    let mpx = pixels as f64 / 1_000_000.0;
    let high = (mpx * tok_per_mpx_high(model)).round() as u64;
    let medium = (high as f64 * MEDIUM_RATIO).round() as u64;
    let low = (high as f64 * LOW_RATIO).round() as u64;
    match q {
        QualityTier::High => (high, high),
        QualityTier::Medium => (medium, medium),
        QualityTier::Low => (low, low),
        QualityTier::Auto => (low, high),
    }
}

/// 한글 위주 프롬프트의 대략적 토큰 수 (2자 ≈ 1토큰 + 여유). 달러로는 무시할 수준이라 정밀할 필요가 없다.
pub fn text_tokens_guess(chars: usize) -> u64 {
    (chars as u64).div_ceil(2) + 16
}

/// Responses 경로에서 이미지 툴을 부르는 메인라인 모델 단가 (USD/1M, pricing 페이지 2026-09-06).
/// 반환: (input, output, 알려진 모델인가)
pub fn mainline_rates(model: &str) -> (f64, f64, bool) {
    match model {
        "gpt-5.6-luna" => (0.20, 1.20, true),
        "gpt-5.6-terra" => (2.00, 12.00, true),
        "gpt-5.6-sol" => (4.00, 20.00, true),
        "gpt-6-astra" => (10.00, 50.00, true),
        _ => (2.00, 12.00, false), // 미지의 모델은 terra 급으로 가정
    }
}

#[derive(Debug, Clone)]
pub struct Estimate {
    pub n: u32,
    pub per_low: f64,
    pub per_high: f64,
    /// 사람이 읽는 근거 한 줄 (토큰 수·단가·가정)
    pub basis: String,
}

impl Estimate {
    pub fn total_low(&self) -> f64 {
        self.per_low * f64::from(self.n)
    }
    pub fn total_high(&self) -> f64 {
        self.per_high * f64::from(self.n)
    }
    pub fn is_range(&self) -> bool {
        (self.per_high - self.per_low).abs() > 0.0005
    }
}

/// 호출 전 예상 비용. `mainline` 이 Some 이면 Responses 경로의 메인라인 모델 토큰도 더한다.
pub fn estimate(
    model: &str,
    pixels: u64,
    quality: QualityTier,
    prompt_chars: usize,
    n: u32,
    mainline: Option<&str>,
) -> Estimate {
    let p = Prices::for_model(model);
    let (tok_low, tok_high) = image_out_tokens(model, pixels, quality);
    let text_tokens = text_tokens_guess(prompt_chars);
    let text_cost = text_tokens as f64 * p.text_in / 1e6;
    let mut per_low = tok_low as f64 * p.image_out / 1e6 + text_cost;
    let mut per_high = tok_high as f64 * p.image_out / 1e6 + text_cost;

    let mpx = pixels as f64 / 1_000_000.0;
    let v2 = !model.starts_with("gpt-image-1");
    let tier = match (quality, v2) {
        (QualityTier::High, true) => "high(실측 고정)",
        (QualityTier::High, false) => "high(공식 단가표)",
        (QualityTier::Medium, _) => "medium(추정)",
        (QualityTier::Low, _) => "low(추정)",
        (QualityTier::Auto, true) => "auto(범위 low~high · 실측 196~1,372)",
        (QualityTier::Auto, false) => "auto(범위 low~high)",
    };
    let tok = if tok_low == tok_high {
        fmt_tok(tok_high)
    } else {
        format!("{}~{}", fmt_tok(tok_low), fmt_tok(tok_high))
    };
    let mut basis = format!(
        "{model} {mpx:.2}Mpx {tier}: 출력 {tok} tok × ${}/M + 텍스트 ≈{text_tokens} tok × ${}/M",
        p.image_out, p.text_in
    );
    if pixels > 1_600_000 {
        basis.push_str(" · 2K/4K 는 1.57Mpx 기준의 픽셀 비례 외삽");
    }
    if let Some(m) = mainline {
        let (i, o, known) = mainline_rates(m);
        // developer 메시지 + 프롬프트 입력, reasoning low + 툴 호출 출력을 대략 잡는다
        let ml = (text_tokens + 150) as f64 * i / 1e6 + 300.0 * o / 1e6;
        per_low += ml;
        per_high += ml;
        basis.push_str(&format!(
            " + 메인라인 {m} ≈${ml:.4}{}",
            if known {
                ""
            } else {
                "(단가 미확인, terra 급 가정)"
            }
        ));
    }
    Estimate {
        n,
        per_low,
        per_high,
        basis,
    }
}

/// Images API / Responses API 의 usage 객체.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub text_tokens: Option<u64>,
    pub image_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl Usage {
    pub fn from_value(v: &Value) -> Option<Usage> {
        let input_tokens = v.get("input_tokens")?.as_u64()?;
        let output_tokens = v.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        let details = v.get("input_tokens_details");
        Some(Usage {
            input_tokens,
            output_tokens,
            text_tokens: details
                .and_then(|d| d.get("text_tokens"))
                .and_then(Value::as_u64),
            image_tokens: details
                .and_then(|d| d.get("image_tokens"))
                .and_then(Value::as_u64),
            total_tokens: v.get("total_tokens").and_then(Value::as_u64),
        })
    }
}

/// Images API usage 로 계산한 실비용 (USD).
pub fn actual_cost(p: Prices, u: &Usage) -> f64 {
    let image_in = u.image_tokens.unwrap_or(0);
    let text_in = u
        .text_tokens
        .unwrap_or(u.input_tokens.saturating_sub(image_in));
    text_in as f64 * p.text_in / 1e6
        + image_in as f64 * p.image_in / 1e6
        + u.output_tokens as f64 * p.image_out / 1e6
}

pub fn usd(x: f64) -> String {
    format!("${x:.3}")
}

/// 천 단위 구분 (5488 → "5,488").
pub fn fmt_tok(t: u64) -> String {
    let digits: Vec<u8> = t.to_string().into_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(digits.len() + digits.len() / 3);
    let mut since_comma = 0;
    for &d in digits.iter().rev() {
        if since_comma == 3 {
            out.push(b',');
            since_comma = 0;
        }
        out.push(d);
        since_comma += 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_else(|_| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &str = "gpt-image-2";

    #[test]
    fn measured_cell_reproduces_todays_cost() {
        // 2026-09-06 실측: 1536x1024 high, input 440 / output 5488 → $0.16684
        let u = Usage {
            input_tokens: 440,
            output_tokens: 5488,
            text_tokens: Some(440),
            image_tokens: Some(0),
            total_tokens: Some(5928),
        };
        let c = actual_cost(Prices::for_model(V2), &u);
        assert!((c - 0.16684).abs() < 1e-6, "{c}");
        let (lo, hi) = image_out_tokens(V2, 1536 * 1024, QualityTier::High);
        assert_eq!((lo, hi), (5488, 5488));
    }

    #[test]
    fn tokens_scale_linearly_and_tiers_order() {
        let (sq, _) = image_out_tokens(V2, 1024 * 1024, QualityTier::High);
        assert!((3600..=3700).contains(&sq), "{sq}");
        let (lo, hi) = image_out_tokens(V2, 1536 * 1024, QualityTier::Auto);
        assert!(lo < hi && hi == 5488);
        assert!(lo <= 1_372, "auto 하한 {lo} 이 실측 최대 1,372 를 넘는다");
        let (m, _) = image_out_tokens(V2, 1536 * 1024, QualityTier::Medium);
        let (l, _) = image_out_tokens(V2, 1536 * 1024, QualityTier::Low);
        assert!(l < m && m < hi);
        // 4K high 는 1.57Mpx 의 5.27배 → 약 28.9k tok
        let (k4, _) = image_out_tokens(V2, 3840 * 2160, QualityTier::High);
        assert!((28_000..=29_500).contains(&k4), "{k4}");
    }

    #[test]
    fn gpt_image_1_uses_official_token_table() {
        // 공식 단가표: 1536x1024 high $0.25 @ $40/M = 6,250 tok
        let (hi, _) = image_out_tokens("gpt-image-1", 1536 * 1024, QualityTier::High);
        assert_eq!(hi, 6250);
        let e = estimate("gpt-image-1", 1536 * 1024, QualityTier::High, 0, 1, None);
        assert!(e.per_high >= 0.249 && e.per_high < 0.26, "{}", e.per_high);
        let (m, _) = image_out_tokens("gpt-image-1", 1536 * 1024, QualityTier::Medium);
        assert!((1_560..=1_590).contains(&m), "{m}"); // $0.063 ≈ 1,575 tok
        let (l, _) = image_out_tokens("gpt-image-1", 1536 * 1024, QualityTier::Low);
        assert!((395..=405).contains(&l), "{l}"); // $0.016 ≈ 400 tok
    }

    #[test]
    fn estimate_totals_and_gate_math() {
        let e = estimate(V2, 1536 * 1024, QualityTier::High, 300, 4, None);
        assert!(!e.is_range());
        assert!((e.per_high - 0.1655).abs() < 0.002, "{}", e.per_high);
        assert!(
            e.total_high() > 0.66 && e.total_high() < 0.67,
            "{}",
            e.total_high()
        );
        let r = estimate(
            V2,
            1536 * 1024,
            QualityTier::Auto,
            300,
            1,
            Some("gpt-5.6-luna"),
        );
        assert!(r.is_range() && r.per_high > r.per_low);
        assert!(r.basis.contains("메인라인 gpt-5.6-luna"));
    }

    #[test]
    fn usage_parses_images_api_shape_and_responses_shape() {
        let v: Value = serde_json::from_str(
            r#"{"input_tokens":440,"input_tokens_details":{"image_tokens":0,"text_tokens":440},"output_tokens":5488,"total_tokens":5928}"#,
        )
        .unwrap();
        let u = Usage::from_value(&v).unwrap();
        assert_eq!(u.text_tokens, Some(440));
        assert_eq!(u.total_tokens, Some(5928));
        let v2: Value =
            serde_json::from_str(r#"{"input_tokens":700,"output_tokens":120}"#).unwrap();
        let u2 = Usage::from_value(&v2).unwrap();
        assert_eq!(u2.text_tokens, None);
        assert!(Usage::from_value(&Value::Null).is_none());
    }

    #[test]
    fn prices_by_model_family() {
        assert_eq!(Prices::for_model("gpt-image-2-2026-04-21").image_out, 30.0);
        assert_eq!(Prices::for_model("gpt-image-1.5").image_out, 32.0);
        assert_eq!(Prices::for_model("gpt-image-1").image_out, 40.0);
        assert_eq!(Prices::for_model("gpt-image-1-mini").image_out, 8.0);
    }

    #[test]
    fn thousands_formatting() {
        assert_eq!(fmt_tok(5488), "5,488");
        assert_eq!(fmt_tok(196), "196");
        assert_eq!(fmt_tok(1_234_567), "1,234,567");
    }
}
