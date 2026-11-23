#![allow(clippy::unwrap_used)] // Tests may unwrap.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[test]
fn wubi_then_pinyin_then_translate_and_commit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("测试 API 未收到连接：{error}"),
            }
        };
        // A socket accepted from a non-blocking listener stays non-blocking on Windows
        // (WSAEWOULDBLOCK / 10035 on the first read). Put it back to blocking first.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let size = stream.read(&mut chunk).unwrap();
            assert_ne!(size, 0, "客户端在发送完整请求前断开");
            request.extend_from_slice(&chunk[..size]);
            if request
                .windows("你好世界".len())
                .any(|part| part == "你好世界".as_bytes())
            {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("Authorization: Bearer test\r\n"));
        let response = r#"{"choices":[{"message":{"content":"Hello, world."}}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_yiime-app"))
        .env("TRANSLATION_API_URL", format!(" http://{address} "))
        .env("TRANSLATION_MODEL", " test ")
        .env("TRANSLATION_API_KEY", " test ")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("http_proxy", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("all_proxy", "http://127.0.0.1:9")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"wqvb\n:1\nshijie\n:1\n:translate\n:commit\n:quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("草稿：你好世界"));
    assert!(stdout.contains("译文预览：Hello, world."));
    assert!(stdout.contains("提交英文：Hello, world."));
}

#[test]
fn non_utf8_line_does_not_stop_the_cli() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yiime-app"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"\xc4\xe3\xba\xc3\nwqvb\n:1\n:quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("草稿：你好"), "{stdout}");
}
