# daedal

OpenAI **`gpt-image-2`** (codename: **ducktape**, 출시 2026-04-21) 로 이미지를 만드는 작은 Rust CLI.
단일 정적 바이너리. Python·Node.js 불필요. 한글 텍스트 99% 정확도, PPT 슬라이드 프리셋 내장.

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
> 5. `daedal "a small red apple on white table" --quality low` 로 한 장 생성해서 정상 동작 확인.
> 6. 설치된 바이너리 경로와 테스트 이미지 경로를 알려줘.

Claude Code 가 알아서 다 처리합니다. OpenAI API 키만 미리 준비하세요 ([발급 페이지](https://platform.openai.com/api-keys)).

### Claude Code 가 없다면 — 원라이너 설치 스크립트

Linux / macOS / Termux 터미널에서:

```bash
curl -fsSL https://raw.githubusercontent.com/Hostingglobal-Tech/daedal/main/install.sh | bash
```

자동으로 처리:
- Rust 툴체인 없으면 `rustup` 설치
- `cargo install` 로 daedal 빌드
- `~/.cargo/bin` 이 `PATH` 에 없으면 rc 파일에 추가
- OpenAI API 키 입력받아 rc 파일에 저장
- 샘플 이미지 1장 생성해 정상 동작 확인

---

## 사용

```bash
daedal "흰 배경에 빨간 큐브"
daedal "유화풍으로 달 위에 앉은 파란 고양이" --quality high
daedal "벚꽃 핀 한옥 마당" --size 1024x1536 -o hanok.png
daedal "로고 시안 3가지" -n 3
daedal "스크립트용" --quiet -o out.png   # stdout 에 파일 경로만 출력
```

### 옵션

| Flag | 값 | 기본 |
|---|---|---|
| `--preset` | `square` · `slide` · `poster` · `infographic` | (없음) |
| `--size` | `1024x1024` · `1024x1536` · `1536x1024` · `auto` | preset 또는 `1024x1024` |
| `--quality` | `low` · `medium` · `high` · `auto` | preset 또는 `auto` |
| `--model` | model ID (snapshot pin: `gpt-image-2-2026-04-21`) | `gpt-image-2` (env `DAEDAL_MODEL`) |
| `-n` | 1..=10 장 | `1` |
| `-o, --out` | 저장 경로 | 아래 표 참조 |
| `--quiet` | — | off |

### Preset (v0.2.0+)

자주 쓰는 사이즈+품질 조합. `--size` / `--quality` 로 override 가능.

| Preset | size | quality | 용도 |
|---|---|---|---|
| `square` | 1024x1024 | auto | 일반 (default 동등) |
| `slide` | 1536x1024 | high | PPT 16:9 슬라이드, 텍스트·차트 |
| `poster` | 1024x1536 | high | 세로 포스터·안내문 |
| `infographic` | 1536x1024 | high | 인포그래픽, 정보 밀도 |

```bash
daedal --preset slide "16:9 슬라이드. 제목 '2026 Q2 영업 실적' 큰 한글 헤딩, 매출 +28%"
daedal --preset poster "당뇨 관리 안내, 한글 캘리그라피"
daedal --preset infographic "8 노드 클러스터 구조. 각 노드 역할 라벨"
```

### 기본 저장 경로

`--out` 을 생략하면:

| 플랫폼 | 경로 |
|---|---|
| Android (Termux) | `/sdcard/DCIM/daedal-<epoch>.png` (갤러리 자동 등록) |
| Windows | `%USERPROFILE%\Pictures\daedal\` |
| macOS / Linux | `$HOME/Pictures/daedal/` |
| 직접 지정 | `export DAEDAL_OUT_DIR=/원하는/경로` |

폴더가 없으면 자동 생성됩니다.

### 예제

프롬프트: *"a futuristic seoul skyline at sunset, photorealistic"* · `1024x1024` · `low` quality

![서울 스카이라인](examples/seoul-skyline.png)

프롬프트: *"실사풍 한국 전통 한옥 마당에 벚꽃이 만발한 봄날 오후, 따뜻한 햇빛, 기와 지붕 디테일 정교, 고해상도 사진"* · `1024x1536` · `high` quality

![한옥 벚꽃](examples/hanok-blossom.png)

### 한글 텍스트 렌더링

`gpt-image-2` 부터 **한글 간판·타이포그래피**가 제대로 나옵니다. 이전 세대 (DALL-E 3 · gpt-image-1) 에서는 한글이 **깨진 자소**나 **유사 한자** 모양으로 나왔는데, 2세대부터 명조·고딕·붓글씨체·캘리그라피까지 자소 정확도가 크게 올라갔습니다.

쓸 만한 예:
```bash
daedal "한국 전통 한정식 간판 '맛집 1998' 붓글씨체, 나무 판에 음각, 낮은 조명"
daedal "서울 지하철 안내판 '강남역 1번 출구 — 삼성역 방향', 파란 배경에 흰 글씨"
daedal "봄 꽃 축제 포스터 '4월 벚꽃 축제', 한글 캘리그라피 + 날짜 '2026.04.10-20'" --size 1024x1536
```

한글이 필요한 포스터·간판·UI 시안을 뽑을 때 유용합니다.

---

## 수동 설치 (고급 사용자)

Claude Code 없이 직접 설치하려면:

### A. cargo install

```bash
# 1) Rust 설치 (이미 있으면 건너뛰기)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 2) daedal 빌드 · 설치 (~/.cargo/bin/daedal 에 생성)
cargo install --git https://github.com/Hostingglobal-Tech/daedal --locked

# 3) OpenAI API 키 등록
echo 'export OPENAI_API_KEY="sk-..."' >> ~/.bashrc
source ~/.bashrc

# 4) 테스트
daedal "a cute red panda" --quality low
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
# Rust 설치: https://rustup.rs 에서 rustup-init.exe 다운로드 후 실행
cargo install --git https://github.com/Hostingglobal-Tech/daedal --locked
setx OPENAI_API_KEY "sk-..."
# 새 PowerShell 창을 열어야 setx 값이 적용됨
daedal "a red cube on white"
```

---

## 요구 사항

- Rust stable toolchain (빌드용)
- `OPENAI_API_KEY` 환경변수 — `gpt-image-2` 사용 가능한 OpenAI 계정

---

## 모델

기본 **`gpt-image-2`** (ducktape). `--model` CLI 플래그 또는 `DAEDAL_MODEL` 환경변수로 변경 가능:

```bash
# Snapshot pin (재현성 — 모델 업데이트 후에도 동일 결과 보장)
DAEDAL_MODEL=gpt-image-2-2026-04-21 daedal "..."

# 또는 CLI 플래그
daedal --model gpt-image-2-2026-04-21 "..."
```

다른 image 모델 (이전 `gpt-image-1` 등) 도 endpoint 가 같으면 그대로 동작합니다.

## 비용

호출마다 usage 가 stderr 에 출력됩니다:

```
[daedal] usage: {"total_tokens":211, ...}
```

상세 요금은 [OpenAI pricing](https://openai.com/api/pricing/) 에서 `gpt-image-2` 항목 참조.

## 보안

- 소스에 API 키 하드코딩 없음. `OPENAI_API_KEY` 환경변수에서만 읽음.
- TLS 는 `rustls` 사용 (OpenSSL 의존 X).
- 텔레메트리·분석·에러 리포팅 없음.
- 외부 통신: `api.openai.com` 하나뿐.

## 라이선스

MIT — [LICENSE](LICENSE) 참조.

## 기여

바이너리는 작게, 코드는 지루하게. 기능 과잉·플러그인 시스템 없음.
한 파일, 한 목적.
