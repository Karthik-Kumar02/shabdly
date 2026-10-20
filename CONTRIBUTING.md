# Contributing to Shabdly

Thanks for helping. Bug reports, fixes, and new dictionary or language ideas are all welcome.

## Report a bug

Open an [issue](https://github.com/Karthik-Kumar02/shabdly/issues) with:

1. Your operating system and version.
2. What you did, what you expected, and what happened.
3. The app you were typing into, if the problem is about inserting text.

Please never paste API keys or private dictation text into an issue.

## Make a change

1. Fork the repo and create a branch.
2. Set up the tools listed in the README under "Build from source".
3. Run the app with `npm run tauri dev`.
4. Add a test for your change, then run `cargo test --workspace` and `npm run build`. Both must pass.
5. Open a pull request that says what changed and why.

## Where things live

| Area | Folder |
| --- | --- |
| Dictionary, learning rules, word swaps | `crates/core/src/dictionary.rs`, `learning.rs`, `swaps.rs` |
| Cleanup prompt and engines | `crates/core/src/prompt.rs`, `providers.rs`, `pipeline.rs` |
| Speech recognition | `crates/core/src/asr.rs` |
| Hotkey and gestures | `src-tauri/src/hotkey.rs`, `gesture.rs` |
| Typing into apps, clipboard | `src-tauri/src/platform/` |
| Dictation flow and learning loop | `src-tauri/src/dictation.rs`, `observe.rs` |
| Interface | `src/` |

## Ground rules

- Keep `crates/core` free of operating-system code, so it can run on mobile later.
- Nothing may leave the user's computer with the default engine. Cloud engines get only the transcript and dictionary words.
- API keys go through the system keychain only, never into files or logs.
