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
