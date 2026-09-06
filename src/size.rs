//! 이미지 크기 파싱·검증.
//!
//! 근거 (developers.openai.com/api/docs/guides/image-generation · api-reference/responses/create,
//! 2026-09-06 확인):
//! - 명명 크기: 1024x1024 · 1536x1024 · 1024x1536 · 2048x2048 · 2048x1152 · 3840x2160 · 2160x3840 · auto
//! - gpt-image-2 는 임의 `WIDTHxHEIGHT` 허용: 양변 16 의 배수, 긴 변 ≤ 3840, 비율 ≤ 3:1,
//!   총 픽셀 655,360 ~ 8,294,400
//! - gpt-image-1 / 1.5 / 1-mini 는 1024x1024 · 1024x1536 · 1536x1024 · auto 만
//!
//! 문자열은 엄격하게 본다 — 공백·부호가 섞이면 실패다. 호출자(main)가 CLI 값을 trim 해서 넘긴다.

/// 구세대(gpt-image-1 계열) 모델이 받는 크기.
pub const CLASSIC_SIZES: &[&str] = &["1024x1024", "1024x1536", "1536x1024", "auto"];
/// gpt-image-2 문서에 명시된 크기 (안내용 — 검증은 규칙으로 한다).
pub const NAMED_SIZES_V2: &[&str] = &[
    "1024x1024",
    "1536x1024",
    "1024x1536",
    "2048x2048",
    "2048x1152",
    "3840x2160",
    "2160x3840",
    "auto",
];
pub const MAX_EDGE: u32 = 3840;
pub const MIN_PIXELS: u64 = 655_360;
pub const MAX_PIXELS: u64 = 8_294_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dims {
    pub w: u32,
    pub h: u32,
}

impl Dims {
    pub fn pixels(self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }
}

fn parse_edge(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|v| *v > 0)
}

/// `WIDTHxHEIGHT` 를 파싱한다. `auto`·공백·부호·잘못된 문자열은 None.
pub fn parse_dims(s: &str) -> Option<Dims> {
    let (w, h) = s.split_once('x')?;
    Some(Dims {
        w: parse_edge(w)?,
        h: parse_edge(h)?,
    })
}

/// 임의 해상도를 받는 모델인가 (문서상 gpt-image-2 계열만).
pub fn supports_custom_size(model: &str) -> bool {
    model.starts_with("gpt-image-2")
}

/// gpt-image-2 임의 해상도 규칙.
pub fn check_custom_rules(d: Dims) -> Result<(), String> {
    // 16 의 배수 = 하위 4비트가 0 (is_multiple_of 는 rustc 1.87+ 라 구 툴체인 호환을 위해 비트마스크)
    if d.w & 0xF != 0 || d.h & 0xF != 0 {
        return Err(format!("{}x{}: 양변이 16 의 배수여야 한다", d.w, d.h));
    }
    if d.w.max(d.h) > MAX_EDGE {
        return Err(format!("{}x{}: 긴 변이 {}px 을 넘는다", d.w, d.h, MAX_EDGE));
    }
    let long = u64::from(d.w.max(d.h));
    let short = u64::from(d.w.min(d.h));
    if long > short * 3 {
        return Err(format!("{}x{}: 비율이 3:1 을 넘는다", d.w, d.h));
    }
    let px = d.pixels();
    if !(MIN_PIXELS..=MAX_PIXELS).contains(&px) {
        return Err(format!(
            "{}x{}: 총 픽셀 {}px 이 허용 범위({}~{}) 밖",
            d.w, d.h, px, MIN_PIXELS, MAX_PIXELS
        ));
    }
    Ok(())
}

pub fn validate_size(size: &str, model: &str) -> Result<(), String> {
    if size == "auto" {
        return Ok(());
    }
    let dims = parse_dims(size).ok_or_else(|| {
        format!("size '{size}': WIDTHxHEIGHT 또는 auto 형식이어야 한다 (공백·부호 불가)")
    })?;
    if supports_custom_size(model) {
        check_custom_rules(dims)
            .map_err(|e| format!("{e} (문서 명명 크기: {})", NAMED_SIZES_V2.join(", ")))
    } else if CLASSIC_SIZES.contains(&size) {
        Ok(())
    } else {
        Err(format!(
            "size '{size}' 는 {model} 에서 불가 (허용: {}) — 임의 해상도는 gpt-image-2 계열만",
            CLASSIC_SIZES.join(", ")
        ))
    }
}

/// 비용 추정에 쓰는 픽셀 수. `auto` 는 모델이 고르므로 흔한 최대인 1536x1024 상당으로 본다
/// (auto 가 2K 이상을 고르는지는 확인 필요 — 고른다면 상한이 과소다).
pub fn dims_for_estimate(size: &str) -> Dims {
    parse_dims(size).unwrap_or(Dims { w: 1536, h: 1024 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_v2_sizes_pass_custom_rules() {
        for s in NAMED_SIZES_V2 {
            assert!(validate_size(s, "gpt-image-2").is_ok(), "{s}");
        }
        assert!(validate_size("2048x1152", "gpt-image-2-2026-04-21").is_ok());
    }

    #[test]
    fn custom_rules_reject_bad_dims() {
        assert!(validate_size("1000x1000", "gpt-image-2").is_err()); // 16 배수 아님
        assert!(validate_size("4096x1024", "gpt-image-2").is_err()); // 긴 변 초과
        assert!(validate_size("3840x1264", "gpt-image-2").is_err()); // 비율 > 3:1
        assert!(validate_size("3840x1280", "gpt-image-2").is_ok()); // 정확히 3:1
        assert!(validate_size("1024x624", "gpt-image-2").is_err()); // 최소 픽셀 미만
        assert!(validate_size("1024x640", "gpt-image-2").is_ok()); // 최소 픽셀 경계
        assert!(validate_size("0x16", "gpt-image-2").is_err());
        assert!(validate_size("abc", "gpt-image-2").is_err());
    }

    #[test]
    fn strict_parsing_rejects_whitespace_and_sign() {
        assert!(validate_size(" 1024x1024", "gpt-image-2").is_err());
        assert!(validate_size("1024x1024 ", "gpt-image-2").is_err());
        assert!(validate_size("+1024x1024", "gpt-image-2").is_err());
        assert!(validate_size("1024 x1024", "gpt-image-2").is_err());
        assert_eq!(parse_dims("1024x1024"), Some(Dims { w: 1024, h: 1024 }));
    }

    #[test]
    fn classic_models_only_classic_sizes() {
        assert!(validate_size("1536x1024", "gpt-image-1.5").is_ok());
        assert!(validate_size("auto", "gpt-image-1").is_ok());
        assert!(validate_size("2048x1152", "gpt-image-1.5").is_err());
    }

    #[test]
    fn estimate_dims_auto_is_landscape() {
        assert_eq!(dims_for_estimate("auto"), Dims { w: 1536, h: 1024 });
        assert_eq!(dims_for_estimate("1024x1024").pixels(), 1_048_576);
    }
}
