# Velo — Autonomous OS & Desktop Agent

> A hyper-fast, low-latency autonomous desktop agent built in 100% pure Rust.  
> Floating command bar • Real-time AI streaming • Voice-to-action • Deep OS integration

---

## Architecture

```
velo/
├── velo-core/      # Agent brain: actors, tools, NIM client, audio, snapshots
├── velo-agent/     # Binary: supervision tree, Tauri bridge
├── src-tauri/      # Tauri v2 shell: borderless floating window
└── ui/             # HTML + TypeScript frontend (Vite)
```

## Prerequisites

```bash
# Rust
rustup update stable

# Tauri CLI v2
cargo install tauri-cli --version "^2"

# Node.js + pnpm
npm install -g pnpm

# Linux system libraries
sudo apt install -y \
  libasound2-dev libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev at-spi2-core libatspi2.0-dev \
  libxdo-dev cmake

```

## Setup

```bash
cp velo.yaml ~/.velo/config.yaml
# Edit ~/.velo/config.yaml and fill in your NVIDIA_API_KEY

cd ui && pnpm install && pnpm build && cd ..
cargo tauri dev
```

## Configuration

Primary config is `~/.velo/config.yaml` (see `velo.yaml`).  
The `stt` LLM entry defaults to **Google Gemini** (`generateContent` with inline
audio — no local model required). Set `provider: openai` for OpenAI-compatible
`/v1/audio/transcriptions` APIs.  
Individual settings can still be overridden via environment variables:

| Variable | Overrides | Description |
|---|---|---|
| `NVIDIA_API_KEY` | `llms.*.api_key` | Your `nvapi-*` key from build.nvidia.com |
| `NIM_MODEL` | `llms.react.model` | Model slug (default: `meta/llama-3.3-70b-instruct`) |
| `NIM_VISION_MODEL` | `llms.vision.model` | Vision model slug (default: `microsoft/phi-3.5-vision-instruct`) |
| `NIM_BASE_URL` | `llms.*.base_url` | NIM endpoint (default: `https://integrate.api.nvidia.com/v1`) |
| `STT_PROVIDER` | `llms.stt.provider` | STT provider: `google` or `openai` (default: `google`) |
| `STT_MODEL` | `llms.stt.model` | STT model slug (default: `models/gemini-2.5-flash-native-audio-latest`) |
| `STT_BASE_URL` | `llms.stt.base_url` | STT endpoint (default: `https://generativelanguage.googleapis.com/v1beta`) |
| `STT_API_KEY` | `llms.stt.api_key` | STT API key |
| `GEMINI_API_KEY` | (falls back to `STT_API_KEY`) | Google AI Studio API key |
| `VELO_CONFIG_PATH` | — | Path to YAML config file (default: `~/.velo/config.yaml`) |
| `VELO_LOG` | — | Tracing filter (default: `velo=info`) |

## Usage

- **Type** a command in the floating bar → ReAct loop executes
- **🎙 Voice** button → speak, transcription feeds the agent
- **≡ Dashboard** → expand task timeline, execution logs, undo controls
- **Destructive actions** → always show a confirmation modal before proceeding
- **Clipboard** → copy an error log → Velo auto-suggests a fix

## Safety

All destructive actions (recursive deletion, registry edits, etc.) are intercepted and require explicit UI confirmation. Volatile tool calls run inside a `wasmtime` WASM sandbox. Snapshot manifests are stored at `.velo/snapshots/` for undo.

## License

MIT
