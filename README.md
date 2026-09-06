# daedal

OpenAI **`gpt-image-2`** 로 이미지를 만드는 작은 Rust CLI.
단일 정적 바이너리. Python·Node.js 불필요. 슬라이드·포스터·인포그래픽 프리셋 내장.
호출 전에 예상 비용을 알려주고, 상한을 넘으면 멈춘다.

[![Rust](https://img.shields.io/badge/Rust-stable-orange)](https://rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue)](LICENSE)

![한옥 벚꽃](examples/hanok-blossom.png)

## 이름

그리스 신화 장인 **다이달로스(Daedalus)** — 미궁 설계자·밀랍 날개 제작자.
영어 사전에도 `daedal` = "정교하게 만든, 교묘한" 이라는 형용사로 남아있습니다.

---

## 🚀 가장 쉬운 설치 — Claude Code 한 방에

Rust·cargo·환경변수 아무것도 몰라도 됩니다. [Claude Code](https://www.claude.com/claude-code) 에 **아래 한 문단을 그대로 복사·붙여넣기** 하세요:

> daedal CLI 를 설치해줘. 저장소는 `https://github.com/Hostingglobal-Tech/daedal` 야.
>
> 자동으로 아래 순서대로 해줘 (중간에 물어보지 말고):
> 1. Rust 툴체인(`rustup`) 이 없으면 설치.
> 2. `cargo install --git https://github.com/Hostingglobal-Tech/daedal --locked` 실행.
> 3. `~/.cargo/bin` 이 `PATH` 에 없으면 `~/.bashrc` (또는 `~/.zshrc`) 에 추가.
> 4. OpenAI API 키를 내가 붙여넣을 수 있게 입력 안내. 붙여넣으면 `OPENAI_API_KEY` 환경변수를 rc 파일에 추가.
> 5. `daedal --list-models` 로 키가 동작하는지 확인(무료), 그 다음 `daedal "a small red apple on white table" --quality low` 로 한 장 생성.
> 6. 설치된 바이너리 경로와 테스트 이미지 경로를 알려줘.

Claude Code 가 알아서 다 처리합니다. OpenAI API 키만 미리 준비하세요 ([발급 페이지](https://platform.openai.com/api-keys)).

### Claude Code 가 없다면 — 원라이너 설치 스크립트

Linux / macOS / Termux 터미널에서:

```bash
curl -fsSL https://raw.githubusercontent.com/Hostingglobal-Tech/daedal/main/install.sh | bash
```

---

## 사용

```bash
daedal "흰 배경에 빨간 큐브"
daedal "유화풍으로 달 위에 앉은 파란 고양이" --quality high
daedal "벚꽃 핀 한옥 마당" --size 1024x1536 -o hanok.png
daedal --preset slide "16:9 슬라이드. 제목 '2026 Q2 실적', 하단 3개 메트릭 카드"
daedal --preset slide16 "정확한 16:9 2K 슬라이드"           # 2048x1152
daedal "로고 시안 3가지" -n 3                                 # 경고 후 진행 (3장 ≈ $0.5)
daedal "스크립트용" --quiet -o out.png                        # stdout 에 파일 경로만
daedal --dry-run --preset poster "…"                          # 호출 없이 요청·예상 비용만 (무료)
daedal --list-models                                          # 사용 가능한 이미지 모델 (무료)
```

호출 전후로 이렇게 알려줍니다:

```
[daedal] model=gpt-image-2 size=1536x1024 quality=high n=1 backend=images auth=api-key endpoint=https://api.openai.com/v1/images/generations
[daedal] 예상 비용: 1장 ≈ $0.167 × 1장 = $0.167  [gpt-image-2 1.57Mpx high(실측 고정): 출력 5,488 tok × $30/M + 텍스트 ≈236 tok × $5/M]
[daedal]  생성 중… 15초 경과
[daedal] 실비용: $0.167 (input 440 tok × $5/M + output 5,488 tok × $30/M) · 누적 $0.167 · 41초
[daedal] saved C:\Users\me\Pictures\daedal\daedal-1788000000.png (1371286 bytes)
```

### 옵션

| Flag | 값 | 기본 |
|---|---|---|
| `--preset` | `square` · `slide` · `slide16` · `poster` · `infographic` | (없음) |
| `--size` | `1024x1024` · `1536x1024` · `1024x1536` · `2048x2048` · `2048x1152` · `3840x2160` · `2160x3840` · `auto` · 임의 `WxH` | preset 또는 `1024x1024` |
| `--quality` | `low` · `medium` · `high` · `auto` | preset 또는 `auto` |
| `--format` / `--compression` | `png` · `jpeg` · `webp` (생략 시 `-o` 확장자에서 추론) / 0~100 | `png` / 100 |
| `--background` | `auto` · `opaque` · `transparent` (png/webp) | `auto` |
| `--model` | 이미지 모델 ID (`DAEDAL_MODEL`) | `gpt-image-2` |
| `--backend` | `auto` · `images` · `responses` (`DAEDAL_BACKEND`) | `auto` |
| `--enhance` | 프롬프트 앞뒤에 품질 계약을 덧붙임 (`DAEDAL_ENHANCE`) | off — 원문 전송 |
| `--max-cost` / `--yes` | 1회 실행 예상 비용 상한 USD / 초과 승인 | `1.0` / off |
| `-n` | 1..=10 장 (3장부터 경고) | `1` |
| `-o, --out` | 저장 경로 | 아래 표 참조 |
| `--dry-run` / `--list-models` / `--quiet` | — | off |

종료코드: `0` 성공 · `1` API/IO 실패 · `2` 인자·검증·비용 게이트 거부.

### Preset

| Preset | size | quality | 용도 |
|---|---|---|---|
| `square` | 1024x1024 | auto | 일반 (default 동등) |
| `slide` | 1536x1024 | high | PPT 슬라이드 (3:2), 텍스트·차트 |
| `slide16` | 2048x1152 | high | 정확한 16:9 2K 슬라이드 (≈1.5배 비용) |
| `poster` | 1024x1536 | high | 세로 포스터·안내문 |
| `infographic` | 1536x1024 | high | 인포그래픽, 정보 밀도 |

### 임의 해상도 (gpt-image-2)

양변 16 의 배수, 긴 변 ≤ 3840, 비율 ≤ 3:1, 총 픽셀 655,360~8,294,400. 검증은 호출 전에 로컬에서 한다.
비용은 픽셀에 비례한다 — 4K(3840x2160) high 1장 ≈ $0.87.

### 기본 저장 경로

`--out` 을 생략하면:

| 플랫폼 | 경로 |
|---|---|
| Android (Termux) | `/sdcard/DCIM/daedal-<epoch>.<ext>` (갤러리 자동 등록) |
| Windows | `%USERPROFILE%\Pictures\daedal\` |
| macOS / Linux | `$HOME/Pictures/daedal/` |
| 직접 지정 | `export DAEDAL_OUT_DIR=/원하는/경로` |

폴더가 없으면 자동 생성됩니다. `-n 2` 이상이면 `<이름>-0.png`, `<이름>-1.png` …

### 한글 텍스트 렌더링

`gpt-image-2` 부터 **한글 간판·타이포그래피**가 제대로 나옵니다. 박을 글자는 따옴표로 감싸고, 폰트 스타일·크기를 문장으로 지정하세요.
daedal 은 프롬프트를 **원문 그대로** 보냅니다(검증된 방식). `--enhance` 는 영어 품질 계약을 덧붙이는 선택 옵션입니다.

```bash
daedal "한국 전통 한정식 간판 '맛집 1998' 붓글씨체, 나무 판에 음각, 낮은 조명"
daedal "서울 지하철 안내판 '강남역 1번 출구 — 삼성역 방향', 파란 배경에 흰 글씨"
daedal "봄 꽃 축제 포스터 '4월 벚꽃 축제', 한글 캘리그라피 + 날짜 '2026.04.10-20'" --size 1024x1536
```

---

## 호출 경로와 인증

| 경로 | 조건 | 엔드포인트 |
|---|---|---|
| `images` (기본) | `OPENAI_BASE_URL` 없음 | `POST /v1/images/generations` — `OPENAI_API_KEY` 필수 |
| `responses` | `OPENAI_BASE_URL` 설정 (OpenAI 호환 프록시) | `POST /v1/responses` + `image_generation` 툴 (`--mainline`, 기본 `gpt-5.6-luna`) |

프록시일 때 자격은 `OPENCODEX_API_AUTH_TOKEN` > `OPENAI_API_KEY` > 없음 순. 키 값은 어떤 출력에도 찍히지 않습니다.
Responses 경로는 툴의 `model` 을 `gpt-image-2` 로 고정합니다(툴 기본값은 `gpt-image-1`). 프록시가 거부하면 `--no-tool-model`.

## 비용

- 단가(2026-09-06, developers.openai.com/api/docs/pricing): gpt-image-2 텍스트 입력 $5/1M, 이미지 출력 $30/1M.
- 실측: 1536x1024 · 1024x1536 `high` = 출력 5,488 토큰 = **$0.165/장**(+텍스트 소액). `auto` 는 196~1,372 토큰.
- 호출마다 예상 비용(전)과 실비용(후, usage 기반)을 stderr 에 출력합니다.
- `--max-cost`(기본 $1.00, `DAEDAL_MAX_COST_USD`, `inf` = 무제한)를 넘는 실행은 `--yes`(`DAEDAL_YES=1`) 없이는 시작하지 않습니다. `--dry-run` 은 막히지 않습니다.
- 연결 실패·429(quota 소진 제외)·500/502/503 은 2·4·8초 백오프로 최대 3회 재시도. 응답 대기 타임아웃·응답 끊김·504·그 밖의 4xx 는 재시도하지 않습니다(서버가 이미 생성을 끝냈을 수 있어 이중 과금 방지).
- bool 환경변수(`DAEDAL_YES`·`DAEDAL_ENHANCE`·`DAEDAL_NO_TOOL_MODEL`): `1/true/yes/on` 켬, `0/false/no/off/빈값` 끔, 그 밖은 오류(rc 2). CLI 로 끄려면 `--yes=false`.
- 자동 파일명은 `daedal-<epoch ms>-<pid>.<ext>`. `-o 디렉토리/` 면 그 안에 자동 이름으로 저장.
- 단가 교정: `DAEDAL_PRICE_IMAGE_OUT_PER_M`, `DAEDAL_PRICE_TEXT_IN_PER_M` (USD / 1M tokens).

---

## 수동 설치 (고급 사용자)

> 빌드 요구: rustc ≥ 1.85 (clap 4.6). 구 툴체인이면 `cargo update -p clap --precise 4.5.60` 후 빌드하세요.

### A. cargo install

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
cargo install --git https://github.com/Hostingglobal-Tech/daedal --locked
echo 'export OPENAI_API_KEY="sk-..."' >> ~/.bashrc
source ~/.bashrc
daedal --list-models && daedal "a cute red panda" --quality low
```

### B. 소스 빌드

```bash
git clone https://github.com/Hostingglobal-Tech/daedal
cd daedal
cargo build --release
cp target/release/daedal ~/.local/bin/   # 또는 PATH 안 아무 곳
```

### Windows PowerShell

```powershell
cargo install --git https://github.com/Hostingglobal-Tech/daedal --locked
setx OPENAI_API_KEY "sk-..."
# 새 PowerShell 창을 열어야 setx 값이 적용됨
daedal "a red cube on white"
```

---

## 요구 사항

- Rust stable toolchain (빌드용)
- `OPENAI_API_KEY` 환경변수 — `gpt-image-2` 사용 가능한 OpenAI 계정 (또는 `OPENAI_BASE_URL` 프록시)

## 모델

기본 모델은 **`gpt-image-2`** 입니다. 재현성이 필요하면 스냅샷을 지정할 수 있습니다.

```bash
DAEDAL_MODEL=gpt-image-2-2026-04-21 daedal "..."
daedal --model gpt-image-1.5 "..."     # 구세대 (크기는 1024x1024·1024x1536·1536x1024·auto 만)
```
