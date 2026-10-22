use shabdly_core::model_store::{ModelState, locate, sha256_file};
use shabdly_core::models::{EngineKind, ModelFile, ModelSpec};
use shabdly_core::settings::Speed;
use shabdly_core::setup::{OllamaStatus, download, download_file, download_model, parse_pull_line, parse_tags};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn tags_ready_when_model_listed() {
    let json = r#"{"models":[{"name":"gemma4:12b"},{"name":"gemma4:e4b","size":9600000000}]}"#;
    assert_eq!(parse_tags(json, "gemma4:e4b"), OllamaStatus::Ready);
}

#[test]
fn tags_no_model() {
    assert_eq!(parse_tags(r#"{"models":[{"name":"gemma4:12b"}]}"#, "gemma4:e4b"), OllamaStatus::NoModel);
    assert_eq!(parse_tags(r#"{"models":[]}"#, "gemma4:e4b"), OllamaStatus::NoModel);
    assert_eq!(parse_tags("not json", "gemma4:e4b"), OllamaStatus::Missing);
}

#[test]
fn pull_line_progress() {
    assert_eq!(parse_pull_line(r#"{"status":"pulling abc","digest":"sha256:abc","total":2000,"completed":500}"#), Some((500, 2000)));
    assert_eq!(parse_pull_line(r#"{"status":"pulling manifest"}"#), None);
    assert_eq!(parse_pull_line(r#"{"status":"success"}"#), None);
    assert_eq!(parse_pull_line("garbage"), None);
}

/// Serve one HTTP response on a random local port; returns the URL.
async fn serve_once(response: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await;
        sock.write_all(&response).await.unwrap();
        let _ = sock.shutdown().await;
    });
    format!("http://{addr}/model.bin")
}

#[tokio::test]
async fn download_writes_part_then_renames() {
    let body = vec![7u8; 100_000];
    let mut resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    resp.extend_from_slice(&body);
    let url = serve_once(resp).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    download(&url, &dest, move |done, total| s2.lock().unwrap().push((done, total))).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(!dir.path().join("model.bin.part").exists());
    let seen = seen.lock().unwrap();
    assert_eq!(*seen.last().unwrap(), (100_000, 100_000));
}

#[tokio::test]
async fn download_failure_leaves_no_dest() {
    let url = serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    assert!(download(&url, &dest, |_, _| {}).await.is_err());
    assert!(!dest.exists());
    assert!(!dir.path().join("model.bin.part").exists());

    // A connection that dies mid-body also leaves nothing behind.
    let url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nConnection: close\r\n\r\nshort".to_vec()).await;
    assert!(download(&url, &dest, |_, _| {}).await.is_err());
    assert!(!dest.exists());
    assert!(!dir.path().join("model.bin.part").exists());
}

/// Serve every connection with `reply(request)`; returns the URL and the requests seen
/// (lowercased, so header names compare regardless of case).
async fn serve(reply: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            let resp = reply(&req);
            s2.lock().unwrap().push(req);
            let _ = sock.write_all(&resp).await;
            let _ = sock.shutdown().await;
        }
    });
    (format!("http://{addr}/model.bin"), seen)
}

fn http(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len()).into_bytes();
    r.extend_from_slice(body);
    r
}

fn body() -> Vec<u8> {
    (0..100_000u32).map(|i| (i % 251) as u8).collect()
}

fn sha_of(bytes: &[u8]) -> String {
    let t = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(t.path(), bytes).unwrap();
    sha256_file(t.path()).unwrap()
}

const NO_WAIT: &[Duration] = &[Duration::ZERO; 3];

fn part(dest: &Path) -> std::path::PathBuf {
    dest.with_file_name("model.bin.part")
}

#[tokio::test]
async fn resume_continues_from_existing_part() {
    let full = body();
    let rest = full[50_000..].to_vec();
    let (url, seen) = serve(move |_| http("206 Partial Content", "Content-Range: bytes 50000-99999/100000\r\n", &rest)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    std::fs::write(part(&dest), &full[..50_000]).unwrap();
    let first = Arc::new(Mutex::new(None));
    let f2 = first.clone();
    download_file(&[url], &dest, 100_000, &sha_of(&full), NO_WAIT, move |done, _| {
        f2.lock().unwrap().get_or_insert(done);
    })
    .await
    .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), full);
    assert!(!part(&dest).exists());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("range: bytes=50000-"), "{}", seen[0]);
    assert!(first.lock().unwrap().unwrap() > 50_000, "progress counts the bytes already on disk");
}

#[tokio::test]
async fn server_without_range_restarts_file() {
    let full = body();
    let f2 = full.clone();
    let (url, _) = serve(move |_| http("200 OK", "", &f2)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    std::fs::write(part(&dest), &full[..50_000]).unwrap();
    download_file(&[url], &dest, 100_000, &sha_of(&full), NO_WAIT, |_, _| {}).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), full);
}

#[tokio::test]
async fn checksum_mismatch_retries_once_then_errors() {
    let (url, seen) = serve(|_| http("200 OK", "", &[1u8; 100_000])).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    let err = download_file(&[url], &dest, 100_000, &sha_of(&body()), NO_WAIT, |_, _| {}).await.unwrap_err();
    assert_eq!(err.to_string(), "Download failed. Check your internet and try again");
    assert!(!dest.exists());
    assert!(!part(&dest).exists());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn primary_down_uses_mirror() {
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let primary = format!("http://{}/model.bin", dead.local_addr().unwrap());
    drop(dead);
    let full = body();
    let f2 = full.clone();
    let (mirror, _) = serve(move |_| http("200 OK", "", &f2)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.bin");
    download_file(&[primary, mirror], &dest, 100_000, &sha_of(&full), NO_WAIT, |_, _| {}).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), full);
}

#[tokio::test]
async fn download_model_fetches_only_missing_files_then_is_ready() {
    let a = b"hello".to_vec();
    let full = body();
    let f2 = full.clone();
    let (url, seen) = serve(move |_| http("200 OK", "", &f2)).await;
    let files: &'static [ModelFile] = Box::leak(Box::new([
        ModelFile { name: "a.bin", size: 5, sha256: Box::leak(sha_of(&a).into_boxed_str()), url: "http://127.0.0.1:9/a.bin" },
        ModelFile { name: "b.bin", size: 100_000, sha256: Box::leak(sha_of(&full).into_boxed_str()), url: Box::leak(url.into_boxed_str()) },
    ]));
    let spec = ModelSpec { id: "dl-test", engine: EngineKind::Llama, speed: Speed::Fast, files, ram_gb: 1.0 };
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("dl-test")).unwrap();
    std::fs::write(t.path().join("dl-test/a.bin"), &a).unwrap();
    let last = Arc::new(Mutex::new((0, 0)));
    let l2 = last.clone();
    download_model(&spec, t.path(), move |d, n| *l2.lock().unwrap() = (d, n)).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(*last.lock().unwrap(), (100_005, 100_005));
    assert!(t.path().join("dl-test/verified.json").exists());
    assert_eq!(locate(&spec, t.path(), None).unwrap(), ModelState::Ready(t.path().join("dl-test")));
}
