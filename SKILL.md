# daedal

Use `daedal` when the user asks to generate a raster image — poster, slide, infographic, wallpaper, hero art, or a comparison set — from a text prompt.

## Rules

- Send the user's prompt **as written**. Do not rewrite, translate, or pad it. `--enhance` opts into a quality wrapper; leave it off unless the user asks.
- Default model is `gpt-image-2`. Change it only when the user names a different model.
- Save to the path the user asked for. With no path, daedal writes to the platform default (`~/Pictures/daedal/`, or `/sdcard/DCIM/` on Android).
- Keep API keys in environment variables. Never put a key in a prompt, a file, a log, or a commit.
- Every call costs money. Check the printed estimate before generating more than one image.

## CLI

```bash
daedal "prompt"
daedal --preset slide16   -o slide.png       "16:9 slide prompt"
daedal --preset poster    -o poster.png      "vertical poster prompt"
daedal --preset infographic -o info.png      "structured infographic prompt"
daedal --dry-run "prompt"                    # cost estimate + request body, no API call
```

## Options

| Option | Meaning |
|---|---|
| `--preset square\|slide\|slide16\|poster\|infographic` | common layouts. `slide16` is exactly 16:9 (2048×1152) |
| `--size <WxH>\|auto` | override preset size. gpt-image-2 accepts arbitrary sizes (multiples of 16, long edge ≤ 3840, ratio ≤ 3:1) |
| `--quality low\|medium\|high\|auto` | override preset quality |
| `--format png\|jpeg\|webp` | output encoding |
| `--background transparent` | transparent background (gpt-image-2) |
| `-n <N>` | generate N images. Cost scales linearly — a warning prints at N ≥ 3 |
| `-o, --out <PATH>` | output path, file or directory |
| `--model <ID>` | image model. Default `gpt-image-2` |
| `--enhance` | prepend a quality contract to the prompt (off by default) |
| `--max-cost <USD>` | refuse to spend more than this. Default `$1` |
| `--yes` | approve a call that exceeds `--max-cost` |
| `--dry-run` | print the estimate and request body, make no call |
| `--list-models` | list image models the key can reach |
| `--quiet` | print only the output path |

## Cost

daedal prints an estimate to stderr before every call and the actual cost after:

```
[daedal] 예상 비용: 1장 ≈ $0.165 × 1장 = $0.165
[daedal] 실비용: $0.167 (input 513 tok × $5/M + output 5,488 tok × $30/M) · 94초
```

Estimates come from the published price table plus measured output-token counts. A call that would exceed `--max-cost` stops with exit code 2 unless `--yes` is given.

## Prompt guidance

- Put text that must appear in the image inside quotes.
- For Korean or other non-Latin text, ask for fewer and larger text elements — dense small text degrades first.
- Slides: name the regions (title, body, footer metrics).
- Posters: state the vertical composition, the focal subject, and the headline.
- Infographics: give the number of sections and ask for explicit labels.

## Backends

daedal picks one automatically:

- **images** — `/v1/images/generations` with `OPENAI_API_KEY`. Default. This is the path with verified Korean text quality.
- **responses** — `/v1/responses` with the `image_generation` tool. Used when `OPENAI_BASE_URL` points at a proxy. The tool's model is pinned to `gpt-image-2` (the tool default is the older `gpt-image-1`).

Force one with `--backend images|responses`.

## Exit codes

`0` success · `1` API or IO failure · `2` bad arguments, failed validation, or a blocked cost gate.

## Verifying a change

```bash
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
./target/release/daedal --version
./target/release/daedal --dry-run "test"     # no API call, no cost
```
