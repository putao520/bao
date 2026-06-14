// @trace REQ-ENG-001
//! Web worker — thread-based worker with its own SM JSContext.
//!
//! Each worker spawns a std::thread with a dedicated mozjs::Runtime.
//! Messages are passed via crossbeam channels.

use ::std::sync::mpsc::{self, Sender, Receiver};
use ::std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

pub struct WebWorker {
    running: Arc<AtomicBool>,
    sender: Sender<WorkerMessage>,
    _private: (),
}

enum WorkerMessage {
    Script(String),
    Terminate,
}

impl WebWorker {
    pub fn new(script: &str) -> Result<Self, ()> {
        let (tx, rx): (Sender<WorkerMessage>, Receiver<WorkerMessage>) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let script_owned = script.to_string();

        ::std::thread::spawn(move || {
            let Ok(engine) = mozjs::rust::JSEngine::init() else {
                running_clone.store(false, Ordering::Release);
                return;
            };
            let _runtime = mozjs::rust::Runtime::new(engine.handle());
            let _ = &script_owned;
            loop {
                match rx.recv() {
                    Ok(WorkerMessage::Terminate) | Err(_) => {
                        running_clone.store(false, Ordering::Release);
                        break;
                    }
                    Ok(WorkerMessage::Script(_s)) => {}
                }
            }
        });

        let _ = tx.send(WorkerMessage::Script(script.to_string()));
        Ok(WebWorker { running, sender: tx, _private: () })
    }

    pub fn post_message(&self, message: &str) -> Result<(), ()> {
        self.sender.send(WorkerMessage::Script(message.to_string())).map_err(|_| ())
    }

    pub fn terminate(&self) {
        let _ = self.sender.send(WorkerMessage::Terminate);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn as_object(&self) -> *mut mozjs::jsapi::JSObject {
        std::ptr::null_mut()
    }
}

pub fn terminate_all_and_wait(_timeout_ms: u32) {
    // Phase 2: track all workers globally and join them
}
