// @trace REQ-ENG-004
use ::std::cell::RefCell;
use ::std::collections::VecDeque;
use ::std::ffi::CString;
use ::std::os::raw::c_void;
use ::std::ptr;
use ::std::sync::atomic::{AtomicUsize, Ordering};

use mozjs::glue::{CreateJobQueue, DeleteJobQueue, JobQueueTraps};
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2::{RunJobs, SetJobQueue};

static JOB_COUNTER: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // Track job IDs in order — the actual JSObject* is stored as a global property
    static JOB_IDS: RefCell<VecDeque<usize>> = const { RefCell::new(VecDeque::new()) };
    static QUEUE_PTR: RefCell<*mut mozjs::jsapi::JobQueue> = const { RefCell::new(ptr::null_mut()) };
}

fn job_prop_name(id: usize) -> CString {
    CString::new(format!("__job_{}", id)).unwrap_or_default()
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
    cx: *mut JSContext,
    _promise: Handle<*mut JSObject>,
    job: Handle<*mut JSObject>,
    _allocation_site: Handle<*mut JSObject>,
    _host_defined_data: Handle<*mut JSObject>,
) -> bool {
    let job_obj = *job.ptr;
    if job_obj.is_null() {
        return true;
    }

    let id = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let global = unsafe { CurrentGlobalOrNull(cx) };
    if global.is_null() {
        return true;
    }

    // Store job as a property on the global object — GC-safe
    let prop = job_prop_name(id);
    let job_val = mozjs::jsval::ObjectValue(job_obj);
    let job_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &job_val,
    };
    unsafe {
        JS_DefineProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
            prop.as_ptr(),
            job_h,
            0,
        );
    }

    JOB_IDS.with(|q| {
        q.borrow_mut().push_back(id);
    });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn run_jobs(
    _queue: *const c_void,
    cx: *mut JSContext,
) {
    loop {
        let job_id = JOB_IDS.with(|q| q.borrow_mut().pop_front());
        let Some(id) = job_id else {
            break;
        };

        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            break;
        }

        // Retrieve the job object from the global property
        let prop = job_prop_name(id);
        let mut job_val = UndefinedValue();
        unsafe {
            JS_GetProperty(
                cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
                prop.as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut job_val },
            );
        }

        if !job_val.is_object() {
            continue;
        }

        let mut rval = UndefinedValue();
        let obj_handle = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &global,
        };
        let fval_handle = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &job_val,
        };
        let empty_args = HandleValueArray::empty();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };

        unsafe {
            let ok = JS_CallFunctionValue(cx, obj_handle, fval_handle, &empty_args, rval_handle);
            if !ok {
                JS_ClearPendingException(cx);
            }
        }

        // Clean up the property after execution
        unsafe {
            JS_DeleteProperty1(
                cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
                prop.as_ptr(),
            );
        }
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
    JOB_IDS.with(|q| q.borrow().is_empty())
}
