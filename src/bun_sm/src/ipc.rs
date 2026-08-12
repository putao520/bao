// @trace REQ-ENG-001
//! IPC — inter-process communication channel.
//!
//! Uses `bun_io::StreamBuffer` for message framing. Phase 2 will add
//! Unix socket transport. Phase 1 provides state machine + buffered send.

use ::std::collections::VecDeque;
use ::std::sync::Mutex;

pub enum IpcDirection {
    ParentToChild,
    ChildToParent,
}

#[derive(Debug, Clone)]
pub enum IpcMessage {
    Json(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcState {
    Disconnected,
    Connected,
    Closed,
}

pub struct IpcChannel {
    state: IpcState,
    direction: IpcDirection,
    buffer: Mutex<VecDeque<IpcMessage>>,
}

impl IpcChannel {
    pub fn new(direction: IpcDirection) -> Self {
        Self {
            state: IpcState::Disconnected,
            direction,
            buffer: Mutex::new(VecDeque::new()),
        }
    }

    pub fn state(&self) -> IpcState {
        self.state
    }

    pub fn direction(&self) -> &IpcDirection {
        &self.direction
    }

    pub fn connect(&mut self) {
        if self.state == IpcState::Disconnected {
            self.state = IpcState::Connected;
        }
    }

    pub fn close(&mut self) {
        self.state = IpcState::Closed;
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn send(&self, msg: IpcMessage) -> Result<(), IpcError> {
        match self.state {
            IpcState::Connected => {
                self.buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back(msg);
                Ok(())
            }
            IpcState::Disconnected => Err(IpcError::Disconnected),
            IpcState::Closed => Err(IpcError::Closed),
        }
    }

    pub fn recv(&self) -> Option<IpcMessage> {
        if self.state != IpcState::Connected {
            return None;
        }
        self.buffer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    pub fn pending_count(&self) -> usize {
        self.buffer.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcError {
    Disconnected,
    Closed,
    EncodingError,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_channel_lifecycle() {
        let mut ch = IpcChannel::new(IpcDirection::ParentToChild);
        assert_eq!(ch.state(), IpcState::Disconnected);
        ch.connect();
        assert_eq!(ch.state(), IpcState::Connected);
        ch.close();
        assert_eq!(ch.state(), IpcState::Closed);
    }

    #[test]
    fn ipc_send_recv() {
        let mut ch = IpcChannel::new(IpcDirection::ChildToParent);
        ch.connect();
        ch.send(IpcMessage::Json("{}".into())).unwrap();
        ch.send(IpcMessage::Binary(vec![1, 2, 3])).unwrap();
        assert_eq!(ch.pending_count(), 2);

        let msg = ch.recv().unwrap();
        assert!(matches!(msg, IpcMessage::Json(s) if s == "{}"));
        assert_eq!(ch.pending_count(), 1);
    }

    #[test]
    fn ipc_send_disconnected() {
        let ch = IpcChannel::new(IpcDirection::ParentToChild);
        assert_eq!(
            ch.send(IpcMessage::Binary(vec![])),
            Err(IpcError::Disconnected)
        );
    }
}
