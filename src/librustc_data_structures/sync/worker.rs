use super::{Lock, Scope};
use std::mem;
use std::sync::Arc;
#[cfg(parallel_compiler)]
use crate::jobserver;
#[cfg(parallel_compiler)]
use parking_lot::Condvar;

pub trait Worker: super::Send {
    type Message: super::Send;
    type Result: super::Send;

    fn message(&mut self, msg: Self::Message);

    fn complete(self) -> Self::Result;
}

struct WorkerQueue<T: Worker> {
    active: bool,
    complete: bool,
    messages: Vec<T::Message>,
}

/// Allows executing a worker on any Rayon thread,
/// sending it messages and waiting for it to complete its computation.
pub struct WorkerExecutor<T: Worker> {
    queue: Lock<WorkerQueue<T>>,
    worker: Lock<Option<T>>,
    result: Lock<Option<T::Result>>,
    #[cfg(parallel_compiler)]
    cond_var: Condvar,
}

impl<T: Worker> WorkerExecutor<T> {
    pub fn new(worker: T) -> Self {
        WorkerExecutor {
            queue: Lock::new(WorkerQueue {
                active: false,
                complete: false,
                messages: Vec::new(),
            }),
            worker: Lock::new(Some(worker)),
            result: Lock::new(None),
            #[cfg(parallel_compiler)]
            cond_var: Condvar::new(),
        }
    }

    fn run_worker(&self) {
        let mut worker = self.worker.lock();
        let worker_ref = worker.as_mut().expect("worker has completed");

        loop {
            let msgs = {
                let mut queue = self.queue.lock();
                let msgs = mem::replace(&mut queue.messages, Vec::new());
                if msgs.is_empty() {
                    queue.active = false;
                    if queue.complete {
                        eprintln!("completing");
                        let result = worker.take().unwrap().complete();
                        *self.result.lock() = Some(result);
                        self.cond_var.notify_all();
                    }
                    break;
                }
                msgs
            };
            for msg in msgs {
                worker_ref.message(msg);
            }
        }
    }

    pub fn complete(&self) -> T::Result {
        let was_active = {
            let mut queue = self.queue.lock();
            assert!(!queue.complete);
            queue.complete = true;
            let was_active = queue.active;
            if !was_active {
                queue.active = true;
            }
            was_active
        };
        if !was_active {
            eprintln!("compl-inactive");
            // Just run the worker on the current thread
            self.run_worker();
        } else {
            eprintln!("compl-active");
            #[cfg(parallel_compiler)]
            {
                // Wait for the result
                jobserver::release_thread();
                self.cond_var.wait(&mut self.result.lock());
                jobserver::acquire_thread();
            }
        }
        self.result.lock().take().unwrap()
    }

    fn queue_message(&self, msg: T::Message) -> bool {
        let mut queue = self.queue.lock();
        queue.messages.push(msg);
        let was_active = queue.active;
        if !was_active {
            queue.active = true;
        }
        was_active
    }

    pub fn message_in_pool(self: &Arc<Self>, msg: T::Message)
    where
        T: 'static
    {
        if !self.queue_message(msg) {
            let this = self.clone();
            #[cfg(parallel_compiler)]
            rayon::spawn(move || this.run_worker());
            #[cfg(not(parallel_compiler))]
            this.run_worker();
        }
    }
}

impl<'a, T: Worker + 'a> WorkerExecutor<T> {
    pub fn message_in_scope(&'a self, scope: &Scope<'a>, msg: T::Message) {
        if !self.queue_message(msg) {
            scope.spawn(|_| self.run_worker());
        }
    }
}
