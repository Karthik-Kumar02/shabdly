//! A Windows user folder like `C:\Users\Rāhul` puts every model under a non-ASCII path. whisper.cpp
//! gets that path as UTF-8 and (MSVC build) widens it before opening, so the real model must load.
//! Windows CI runs this. Elsewhere: cargo test -p shabdly-core --test unicode_path -- --ignored
use shabdly_core::asr::WhisperAsr;
use shabdly_core::setup::download_file;
use std::time::Duration;

// ggerganov/whisper.cpp at 5359861c739e955e79d9a303bcbc70fb988958b1: the smallest multilingual model.
const URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny-q5_1.bin";
const SIZE: u64 = 32_152_673;
const SHA: &str = "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7";

#[cfg_attr(not(windows), ignore)]
#[tokio::test]
async fn whisper_loads_from_non_ascii_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Rāhul राहुल");
    std::fs::create_dir(&dir).unwrap();
    let model = dir.join("ggml-tiny-q5_1.bin");
    download_file(&[URL.to_string()], &model, SIZE, SHA, &[Duration::from_secs(2); 3], |_, _| {}).await.unwrap();
    WhisperAsr::load(&model).unwrap();
}
