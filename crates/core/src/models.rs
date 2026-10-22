//! Cleanup model catalog (spec section 2): every file pinned to a Hugging Face commit with
//! its size and sha256, plus the pick by OS and macOS version and whether RAM allows Best.
use crate::settings::Speed;

/// Shabdly's own copies of every catalog file, used when the primary URL fails.
pub const MIRROR_BASE: &str = "https://huggingface.co/Karthik-Kumar02/shabdly-models/resolve/main";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    Mlx,
    Llama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
}

#[derive(Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Primary URL, pinned to a commit so a repo update never changes the bytes.
    pub url: &'static str,
}

#[derive(Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub engine: EngineKind,
    pub speed: Speed,
    pub files: &'static [ModelFile],
    /// Memory in use while loaded, in GB.
    pub ram_gb: f32,
}

const fn f(name: &'static str, size: u64, sha256: &'static str, url: &'static str) -> ModelFile {
    ModelFile {
        name,
        size,
        sha256,
        url,
    }
}

// mlx-community/gemma-4-e4b-it-5bit at e648f3767cea9a0b1b5b54b16cb1735b13e8f51c
// 5-bit, not 4-bit: on the Swift helper the 4-bit build translated case 8 (14/15, spec risk Q2);
// 5-bit passes 15/15 with a 478 to 493 ms median on the M5 (user accepted over 450 ms, 2026-10-22).
const E4B_MLX: &[ModelFile] = &[
    f(
        "chat_template.jinja",
        17336,
        "2f1b4d75d067bae3fe44e676721c7f077d243bc007156cb9c2f8b5836613d082",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/chat_template.jinja",
    ),
    f(
        "config.json",
        6628,
        "3d70703016aaf06d58760450b6bcaafc79fc5364fb3ea8d2a03c40a291a4385e",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/config.json",
    ),
    f(
        "generation_config.json",
        208,
        "d4226bbe3117d2d253ba4609720ba82c6c4ce4627a9a6ae05387c78983ac03de",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/generation_config.json",
    ),
    f(
        "model-00001-of-00002.safetensors",
        5367389748,
        "e4f38cc0f9ae1806f5693011dec3403f62ecc3ef97437eacfe15e79f1d185149",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/model-00001-of-00002.safetensors",
    ),
    f(
        "model-00002-of-00002.safetensors",
        712953326,
        "86f74a73371cac7e06e9a7cdc6fea75eb90fd4eaaad2798d0111603624be3a99",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/model-00002-of-00002.safetensors",
    ),
    f(
        "model.safetensors.index.json",
        282511,
        "f6619883e1cc02e52cef068bd4ddbffa49ad3beaecfd4c7bf82a37fc705e0990",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/model.safetensors.index.json",
    ),
    f(
        "processor_config.json",
        1316,
        "de3e580aebdc98272d4c4547daffe6525fcbae18a83a0e0bcf0d7444d4ee6f37",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/processor_config.json",
    ),
    f(
        "tokenizer.json",
        32169626,
        "cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/tokenizer.json",
    ),
    f(
        "tokenizer_config.json",
        2740,
        "080d9e1aff284e2f6043889cd05367966f7c7b80e025fbc0b06745e218158656",
        "https://huggingface.co/mlx-community/gemma-4-e4b-it-5bit/resolve/e648f3767cea9a0b1b5b54b16cb1735b13e8f51c/tokenizer_config.json",
    ),
];

// mlx-community/gemma-4-e2b-it-4bit at 238767527555cb75a05732a84dff5d6ba0dd6809
const E2B_MLX: &[ModelFile] = &[
    f(
        "chat_template.jinja",
        17336,
        "2f1b4d75d067bae3fe44e676721c7f077d243bc007156cb9c2f8b5836613d082",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/chat_template.jinja",
    ),
    f(
        "config.json",
        6395,
        "6397cb6eca41b911d1dcab74e17941351057bd759284052a2331918ff6f9246c",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/config.json",
    ),
    f(
        "generation_config.json",
        208,
        "d4226bbe3117d2d253ba4609720ba82c6c4ce4627a9a6ae05387c78983ac03de",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/generation_config.json",
    ),
    f(
        "model.safetensors",
        3550670554,
        "038e39a37a7667373d2c3991375446b10c96ae1d717a68674870343db376b76e",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/model.safetensors",
    ),
    f(
        "model.safetensors.index.json",
        218323,
        "edb157dbf495e23f37377af4a628a9ad13c4ee7937f93ccb36ec9e9a19940f16",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/model.safetensors.index.json",
    ),
    f(
        "processor_config.json",
        1316,
        "de3e580aebdc98272d4c4547daffe6525fcbae18a83a0e0bcf0d7444d4ee6f37",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/processor_config.json",
    ),
    f(
        "tokenizer.json",
        32169626,
        "cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/tokenizer.json",
    ),
    f(
        "tokenizer_config.json",
        2740,
        "080d9e1aff284e2f6043889cd05367966f7c7b80e025fbc0b06745e218158656",
        "https://huggingface.co/mlx-community/gemma-4-e2b-it-4bit/resolve/238767527555cb75a05732a84dff5d6ba0dd6809/tokenizer_config.json",
    ),
];

// google/gemma-4-E4B-it-qat-q4_0-gguf at 4b4a2c1d584be7264f87aac328a1bc739ce81b6c
const E4B_GGUF: &[ModelFile] = &[f(
    "gemma-4-E4B_q4_0-it.gguf",
    5154941280,
    "676c35070db6dbe52f93e9c864ee0fba4eddea94b9c875d9cb10daff453fbaee",
    "https://huggingface.co/google/gemma-4-E4B-it-qat-q4_0-gguf/resolve/4b4a2c1d584be7264f87aac328a1bc739ce81b6c/gemma-4-E4B_q4_0-it.gguf",
)];

// google/gemma-4-E2B-it-qat-q4_0-gguf at 675cff42a74c774d6cb76f76d8eacb49b48c9b93
const E2B_GGUF: &[ModelFile] = &[f(
    "gemma-4-E2B_q4_0-it.gguf",
    3349516256,
    "fa401b55b07ee70a54c6dae3903c783a6e65064312529ea57175cb5f8dec6634",
    "https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf/resolve/675cff42a74c774d6cb76f76d8eacb49b48c9b93/gemma-4-E2B_q4_0-it.gguf",
)];

// GGUF ram_gb: llama-server b11819 RSS on an M5 Mac (Metal) with `-c 8192 -np 1`, measured in Task 8.
// The E2B MLX build is 4-bit: 5-bit (9/15) and 6-bit (7/15) also failed the punctuation
// gate, so Task 10 adds the deterministic punctuation fix (spec risk Q1).
pub static CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "mlx-gemma4-e4b-5bit",
        engine: EngineKind::Mlx,
        speed: Speed::Best,
        files: E4B_MLX,
        ram_gb: 4.0,
    },
    ModelSpec {
        id: "mlx-gemma4-e2b",
        engine: EngineKind::Mlx,
        speed: Speed::Fast,
        files: E2B_MLX,
        ram_gb: 2.9,
    },
    ModelSpec {
        id: "gguf-gemma4-e4b-qat",
        engine: EngineKind::Llama,
        speed: Speed::Best,
        files: E4B_GGUF,
        ram_gb: 5.2,
    },
    ModelSpec {
        id: "gguf-gemma4-e2b-qat",
        engine: EngineKind::Llama,
        speed: Speed::Fast,
        files: E2B_GGUF,
        ram_gb: 3.4,
    },
];

/// Every machine starts on the fast model (user decision, 2026-10-22); Best is opt-in.
pub const DEFAULT_SPEED: Speed = Speed::Fast;

/// Best is offered only with 16 GB of RAM or more and a usable GPU.
pub fn best_offered(ram_gb: u64, has_gpu: bool) -> bool {
    ram_gb >= 16 && has_gpu
}

/// MLX needs macOS 14 or later; older Macs and Windows use llama-server with the GGUF files.
pub fn pick(os: Os, macos_major: u32, speed: Speed) -> &'static ModelSpec {
    let engine = if os == Os::Mac && macos_major >= 14 {
        EngineKind::Mlx
    } else {
        EngineKind::Llama
    };
    CATALOG
        .iter()
        .find(|s| s.engine == engine && s.speed == speed)
        .expect("catalog covers every engine and speed")
}

pub fn mirror_url(spec: &ModelSpec, f: &ModelFile) -> String {
    format!("{MIRROR_BASE}/{}/{}", spec.id, f.name)
}

pub fn total_bytes(spec: &ModelSpec) -> u64 {
    spec.files.iter().map(|f| f.size).sum()
}
