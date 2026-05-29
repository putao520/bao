use ::std::cell::RefCell;
use ::std::os::raw::c_void;
use ::std::ptr;

use mozjs::glue::{CreateJobQueue, DeleteJobQueue, JobQueueTraps};
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2::{RunJobs, SetJobQueue};

thread_local! {
    static JOB_QUEUE: RefCell<Vec<*mut JSObject>> = RefCell::new(Vec::new());
    static QUEUE_PTR: RefCell<*mut mozjs::jsapi::JobQueue> = RefCell::new(ptr::null_mut());
}

pub struct JobQueue;

impl JobQueue {
    pub fn init(cx: &mozjs::context::JSContext) -> bool {
        let traps = JobQueueTraps {
            getHostDefinedData: Some(get_host_defined_data),
            enqueuePromiseJob: Some(enqueue_job),
            runJobs: Some(run_jobs),
            empty: Some(is_empty),
            pushNewInterruptQueue: None,
            popInterruptQueue: None,
            dropInterruptQueues: None,
        };

        let queue = unsafe { CreateJobQueue(&traps, ptr::null(), ptr::null_mut()) };
        if queue.is_null() {
            return false;
        }

        QUEUE_PTR.with(|p| {
            *p.borrow_mut() = queue;
        });

        unsafe { SetJobQueue(cx, queue) }
        true
    }

    pub fn drain(cx: &mut mozjs::context::JSContext) {
        unsafe { RunJobs(cx) }
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        QUEUE_PTR.with(|p| {
            let ptr = *p.borrow();
            if !ptr.is_null() {
                unsafe { DeleteJobQueue(ptr) };
                *p.borrow_mut() = ptr::null_mut();
            }
        });
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn enqueue_job(
    _queue: *const c_void,
    _cx: *mut JSContext,
    _promise: Handle<*mut JSObject>,
    job: Handle<*mut JSObject>,
    _allocation_site: Handle<*mut JSObject>,
    _host_defined_data: Handle<*mut JSObject>,
) -> bool {
    JOB_QUEUE.with(|q| {
        q.borrow_mut().push(*job.ptr);
    });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn run_jobs(
    _queue: *const c_void,
    cx: *mut JSContext,
) {
    loop {
        let job = JOB_QUEUE.with(|q| q.borrow_mut().pop());
        let Some(job) = job else {
            break;
        };

        let fval = mozjs::jsval::ObjectValue(job);
        let mut rval = UndefinedValue();

        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            break;
        }

        let obj_handle = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &global,
        };
        let fval_handle = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &fval,
        };
        let empty_args = HandleValueArray::empty();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };

        unsafe {
            JS_CallFunctionValue(cx, obj_handle, fval_handle, &empty_args, rval_handle);
        }

        JS_ClearPendingException(cx);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn get_host_defined_data(
    _queue: *const c_void,
    _cx: *mut JSContext,
    data: MutableHandle<*mut JSObject>,
) -> bool {
    data.set(ptr::null_mut());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn is_empty(_queue: *const c_void) -> bool {
    JOB_QUEUE.with(|q| q.borrow().is_empty())
}
