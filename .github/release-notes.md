## What's new

- **Built-in cleanup engine.** No Ollama needed. Setup downloads one small model with one click (about 4 GB together with the speech model), and the download resumes if your connection drops. On Mac, cleanup is about 2x faster than with Ollama.
- **Fastest or Best.** Every computer starts on Fastest. With 16 GB of memory or more, switch to Best in Settings for a little more accuracy.
- **Never stuck.** If the built-in engine stops, Shabdly restarts it, then falls back to Ollama (if installed), and otherwise types your words without cleanup. Settings shows which engine is running, with a Retry button.
- **Ollama is now optional.** Updating with Ollama set up? Shabdly switches to the built-in engine when its model is already on your computer, or offers the download. Ollama stays available under Advanced, and 0.1.5 still opens your settings if you go back.
- Questions keep their question mark, and long dictations are never cut off.
- Speech recognition: whisper.cpp flash attention was tested and stays off for now, because it changed language detection on real Hindi recordings.
- macOS 13.2 and earlier: the built-in engine needs macOS 13.3 or later, so keep using Ollama or a cloud engine there.

## Polish

- When cleanup is skipped, the pill says why: no Gemini or Claude key, Ollama not running, took too long, or engine error.
- Setup's hotkey check only counts the dictation key itself, and a Check again button re-runs it.
- When another app holds Secure Input, Shabdly copies your text and says so instead of failing quietly.
- Missed the Undo on a "Learned" toast? The last 10 learned words have Undo under Recently learned in the Dictionary tab.

## Download

| Your computer | File |
| --- | --- |
| Mac with Apple Silicon (M1 or newer) | `Shabdly_..._aarch64.dmg` |
| Windows 10 or 11 | `Shabdly_..._x64-setup.exe` |

These builds are not signed with a paid certificate yet, so your computer will warn you the first time. The README explains the one-time steps: [Install](https://github.com/Karthik-Kumar02/shabdly#install).
