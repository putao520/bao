// REQ-CDP-002: CDP WebSocket server — tungstenite-based  @trace REQ-CDP-001
use std::io::{Read, Write};

use tungstenite::Message;
use tungstenite::protocol::WebSocket;

pub fn read_message<S: Read + Write>(ws: &mut WebSocket<S>) -> Result<Option<String>, ()> {
    match ws.read() {
        Ok(Message::Text(text)) => Ok(Some(text.to_string())),
        Ok(Message::Binary(data)) => {
            Ok(Some(String::from_utf8_lossy(&data).into_owned()))
        }
        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Ok(None),
        Ok(Message::Close(_)) => Err(()),
        Ok(Message::Frame(_)) => Ok(None),
        Err(_) => Err(()),
    }
}

pub fn write_message<S: Read + Write>(ws: &mut WebSocket<S>, data: &str) -> Result<(), ()> {
    ws.send(Message::Text(data.into())).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use tungstenite::accept;
    use tungstenite::client::client;

    fn setup_ws_pair() -> (WebSocket<TcpStream>, WebSocket<TcpStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/", addr);

        let server_handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            accept(stream).unwrap()
        });

        thread::sleep(std::time::Duration::from_millis(50));

        let tcp = TcpStream::connect(addr).unwrap();
        let (client_ws, _response) = client(ws_url.as_str(), tcp).unwrap();
        let server_ws = server_handle.join().unwrap();

        (server_ws, client_ws)
    }

    #[test]
    fn read_text_message() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        client_ws.send(Message::Text("hello cdp".into())).unwrap();
        let msg = read_message(&mut server_ws).unwrap();
        assert_eq!(msg, Some("hello cdp".to_string()));
    }

    #[test]
    fn read_binary_message() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        client_ws.send(Message::Binary(vec![1u8, 2, 3].into())).unwrap();
        let msg = read_message(&mut server_ws).unwrap();
        assert_eq!(msg, Some("\x01\x02\x03".to_string()));
    }

    #[test]
    fn write_and_read_roundtrip() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        write_message(&mut server_ws, "{\"id\":1}").unwrap();
        let msg = read_message(&mut client_ws).unwrap();
        assert_eq!(msg, Some("{\"id\":1}".to_string()));
    }

    #[test]
    fn read_ping_returns_none() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        client_ws.send(Message::Ping(vec![1u8, 2, 3].into())).unwrap();
        let msg = read_message(&mut server_ws).unwrap();
        assert_eq!(msg, None);
    }

    #[test]
    fn read_close_returns_err() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        client_ws.send(Message::Close(None)).unwrap();
        let result = read_message(&mut server_ws);
        assert!(result.is_err());
    }

    #[test]
    fn write_message_success() {
        let (mut server_ws, mut client_ws) = setup_ws_pair();
        assert!(write_message(&mut server_ws, "test").is_ok());
        let msg = read_message(&mut client_ws).unwrap();
        assert_eq!(msg, Some("test".to_string()));
    }
}
