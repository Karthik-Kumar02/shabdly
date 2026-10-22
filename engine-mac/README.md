# shabdly-engine (Mac)

Small Swift helper that loads one MLX model and serves Shabdly's cleanup requests on `127.0.0.1`.
macOS 14 or later, Apple silicon.

## Build

```sh
xcodebuild -downloadComponent MetalToolchain   # once per Xcode install, if `metal` is missing
scripts/build-engine-mac.sh
```

Output: `src-tauri/engines/mac/shabdly-engine` and `mlx-swift_Cmlx.bundle` (holds `default.metallib`).
The bundle must stay next to the binary. Plain `swift build` is not enough for shipping: use the script.

## Run

```sh
SHABDLY_ENGINE_TOKEN=<64 hex chars> shabdly-engine --model <model dir> --port <n> --parent-pid <pid>
```

- `GET /health`: `200 {"status":"ok"}` once the model is loaded, `503` before. No token needed.
- `POST /v1/chat/completions`: needs `Authorization: Bearer <token>`, else `401`. Body: `messages`,
  `max_tokens` (default 4096), `temperature` (default 0). Thinking is always off.
  Reply: `{"choices":[{"finish_reason":"stop"|"length","message":{"role":"assistant","content":"..."}}]}`.
- Anything else: `404`. One request per connection; generations run one at a time.
- Exit codes: `0` parent gone (checked every second), `1` bad arguments, missing token or port in use,
  `2` model failed to load.

## Notes

- mlx-swift-lm 3.x does not load tokenizers itself; `TransformersLoader` in `main.swift` bridges
  swift-transformers' `AutoTokenizer` (no macros, so no macro trust prompt in CI).
- The KV cache of the previous prompt is kept and rewound to the shared prefix, so only the part after the
  system prompt is prefilled. This saves about 160 ms per request with E4B on an M5. If the cache cannot be
  rewound exactly (sliding-window layers past their window), the whole prompt is prefilled.
