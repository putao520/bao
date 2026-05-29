use mozjs::rust::wrappers2::{RunJobs, UseInternalJobQueues};

pub struct JobQueue;

impl JobQueue {
    pub fn init(cx: &mozjs::context::JSContext) -> bool {
        unsafe { UseInternalJobQueues(cx) }
    }

    pub fn drain(cx: &mut mozjs::context::JSContext) {
        unsafe { RunJobs(cx) }
    }
}
