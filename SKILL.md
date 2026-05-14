# daedal

Use `daedal` when the user asks to generate a raster image, poster, slide image, infographic, wallpaper, or comparison image through the Daedal CLI.

## Hard Rules

- Daedal is an OpenAI `gpt-image-2` image-generation CLI.
- Always use `gpt-image-2`; do not offer or use model override flags or environment variables.
- Do not use `--model`, `DAEDAL_MODEL`, snapshot model IDs, or older image models.
- Save the generated image to the user-requested path. If no path is requested, use the platform default output path or an explicit useful path such as Desktop when the user asks for comparison.
- Keep API keys in environment variables only. Never write keys into prompts, files, logs, or commit history.

## CLI

```bash
daedal "prompt"
daedal --preset slide --quality high -o output.png "16:9 slide prompt"
daedal --preset poster -o poster.png "vertical poster prompt"
daedal --preset infographic -o infographic.png "structured infographic prompt"
```

## Options

- `--preset square|slide|poster|infographic`: choose the common image layout.
- `--size 1024x1024|1024x1536|1536x1024|auto`: override preset size.
- `--quality low|medium|high|auto`: override preset quality.
- `-n`: generate multiple images.
- `-o, --out`: output path.
- `--raw`: send the prompt without Daedal's prompt-quality wrapper.
- `--quiet`: print only the output path.

## Prompt Guidance

- Put exact text to render inside quotes.
- For Korean text, ask for fewer and larger text elements unless the user needs a dense infographic.
- For slide images, specify layout regions such as title, main message, and bottom metrics.
- For posters, specify vertical composition, focal subject, headline, and supporting text.
- For infographics, specify the number of steps or sections and request clear labels, icons, and arrows.

## Verification

After changing Daedal itself:

```bash
cargo build --release
daedal --version
daedal --help
```

The help output must not contain model-selection options.
