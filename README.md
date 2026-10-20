# Shabdly

**Words that learn you.**

Shabdly is a free, private voice dictation app for macOS and Windows. Hold a key, speak in English, Hindi, or Hinglish, let go, and clean text appears in whatever app you are typing in. When you fix a word it got wrong, Shabdly remembers, so it gets your names and terms right next time.

## Features

- **Learning dictionary.** Correct a word after dictating and Shabdly learns it, with a one-click Undo. You can also add words yourself, star the important ones, and import or export your dictionary as JSON. Common words need the same correction twice before Shabdly swaps them, so "their" never silently becomes "there".
- **Hinglish.** In Auto mode, English stays English and any Hindi is written in Roman script ("main kal aaunga"). Hindi (Devanagari) and Hinglish modes are there too. Fillers like "um" are removed and self-corrections ("5, no wait, 6") are applied.
- **Local by default.** Speech recognition runs on your computer with Whisper, and cleanup runs on your computer with Ollama.
- **Switch engines.** Prefer the cloud? Pick Gemini (free tier or paid) or Claude Haiku in settings. If an engine is slow or down, Shabdly still inserts your words, just without the cleanup.
- **Hold or hands-free.** Hold the key to talk, or double-tap it to keep recording until you tap again. Esc cancels.

## Privacy

- With the default engine, nothing leaves your computer.
- Audio is never saved. History keeps only text (your last 100 dictations), and you can clear it or turn it off.
- Cloud engines receive only the transcript and your dictionary words. No screenshots, no on-screen text, no app names.
- API keys live in your system keychain (Keychain on macOS, Credential Manager on Windows), never in files or logs.

## Install from source

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20 or newer, [CMake](https://cmake.org), and [Ollama](https://ollama.com) for the local engine.

```bash
git clone https://github.com/Karthik-Kumar02/shabdly.git
cd shabdly
npm install
npm run tauri build
```

The app is in `target/release/bundle/`. For development, run `npm run tauri dev`.

## First run

Shabdly walks you through setup:

1. Microphone permission.
2. On macOS, Accessibility and Input Monitoring, so Shabdly can type into other apps and see your dictation key.
3. The local cleanup model (`gemma4:e4b` through Ollama), or a cloud engine instead.
4. The speech model download (about 0.9 GB).
5. A practice box: hold **Fn** on macOS or **Ctrl+Win** on Windows and say hello.

## Project layout

- `crates/core`: the platform-neutral core (dictionary, learning, cleanup pipeline, speech recognition). No OS calls, so it can be reused on mobile.
- `src-tauri`: the desktop app (hotkey, overlay, text insertion, settings).
- `src`: the user interface (TypeScript, no framework).

Run the tests with `cargo test --workspace`. With Ollama running, `cargo test -p shabdly-core --test spike_cases -- --ignored` checks the cleanup engine against real Hindi, English, and Hinglish samples.

An open-source alternative to Wispr Flow.

## License

MIT. See [LICENSE](LICENSE).
