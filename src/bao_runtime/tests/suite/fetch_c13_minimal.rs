use std::io::{Read, Write};
use std::net::TcpListener;

fn start_test_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(false).unwrap();

    std::thread::spawn(move || match listener.accept() {
        Ok((mut stream, _addr)) => {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).unwrap_or(0);
            let body = b"hi";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
        Err(_) => {}
    });

    port
}

#[test]
fn test_send_sync_direct() {
    // Initialize Output stream before HTTPThread spawns (avoids STDOUT_STREAM_SET assert)
    bun_core::output::init_test();

    let port = start_test_server();
    let url = format!("http://127.0.0.1:{}/test", port);

    let result = bun_runtime::http_client::http_request(bun_http::Method::GET, &url, &[], None);

    match result {
        Ok(resp) => {
            assert_eq!(resp.status_code, 200);
            assert_eq!(resp.body.as_ref(), b"hi");
        }
        Err(e) => {
            panic!("C13 FAIL: {}", e);
        }
    }
}
