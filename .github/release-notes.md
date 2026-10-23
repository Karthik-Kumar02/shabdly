## Fixes

- **No more duplicate engine.** Switching between Fastest and Best (or turning off the built-in engine) while the engine was still starting could leave a second copy running in the background, using about 2.6 GB of memory. Shabdly now stops it.
- If your disk fills up during a download, Shabdly says how many GB to free instead of showing a system error.
- Ollama: a reply cut off at its length limit is no longer typed; your words go in without cleanup instead.
- Finds models you already downloaded with Hugging Face tools when their cache is in a custom folder (`HF_HUB_CACHE` or `XDG_CACHE_HOME`).
- Windows: Best quality is offered only when a graphics card is really found.

## Download

| Your computer | File |
| --- | --- |
| Mac with Apple Silicon (M1 or newer) | `Shabdly_..._aarch64.dmg` |
| Windows 10 or 11 | `Shabdly_..._x64-setup.exe` |

These builds are not signed with a paid certificate yet, so your computer will warn you the first time. The README explains the one-time steps: [Install](https://github.com/Karthik-Kumar02/shabdly#install).
