use shabdly_core::setup::{OllamaStatus, download, parse_pull_line, parse_tags};
use std::sync::{Arc, Mutex};
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
