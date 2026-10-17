//! Work on several threads where there are threads: the renderer's start-up creates its pipelines
//! concurrently. wgpu compiles a pipeline's shaders when the pipeline is created, and on Direct3D
//! 12 that is DXC turning naga's HLSL into DXIL every time, with no cache across runs (about 1 s for
//! the whole renderer on one thread; docs/bench/dx12.md 10). wgpu's `Device` is `Send + Sync`
//! natively, so independent pipelines can be created at once. In the browser (wasm32, no threads)
//! every task runs at once, in order, on the calling thread.
//!
//! Natively every task gets a thread, but at most [`limit`] tasks run at a time, and a free slot
//! goes to the waiting task that was started first, a task's own subtasks counting as started
//! where it was (depth first: the subtasks of the first task run before the second task). A task
//! that waits for its subtasks gives up its slot meanwhile. So the longest pipelines, started
//! first, run without competing with the others for the CPU, and nested scopes cannot deadlock.

/// Runs `f` with a [`Scope`] whose tasks run on threads of their own natively and right away in
/// the browser; returns when every task has finished.
#[cfg(not(target_arch = "wasm32"))]
pub fn scope<'env, R>(f: impl for<'s> FnOnce(&Scope<'s, 'env>) -> R) -> R {
    let path = slots::current_path();
    let held = slots::holding();
    let r = std::thread::scope(|s| {
        let r = f(&Scope { inner: s, path });
        // The tasks not joined yet are joined when the scope ends: wait for them without a slot.
        if held {
            slots::release();
        }
        r
    });
    if held {
        slots::acquire(slots::current_path());
    }
    r
}

/// Runs `f` with a [`Scope`] whose tasks run on threads of their own natively and right away in
/// the browser; returns when every task has finished.
#[cfg(target_arch = "wasm32")]
pub fn scope<'env, R>(f: impl for<'s> FnOnce(&Scope<'s, 'env>) -> R) -> R {
    f(&Scope {
        _scope: std::marker::PhantomData,
    })
}

/// Spawns tasks (see [`scope`]).
pub struct Scope<'s, 'env: 's> {
    #[cfg(not(target_arch = "wasm32"))]
    inner: &'s std::thread::Scope<'s, 'env>,
    /// The priority of the task that opened the scope (empty on a thread outside any task).
    #[cfg(not(target_arch = "wasm32"))]
    path: Vec<u64>,
    #[cfg(target_arch = "wasm32")]
    _scope: std::marker::PhantomData<(&'s (), &'env ())>,
}

/// A task's result, taken with [`Task::join`].
pub struct Task<'s, T> {
    #[cfg(not(target_arch = "wasm32"))]
    handle: std::thread::ScopedJoinHandle<'s, T>,
    #[cfg(target_arch = "wasm32")]
    value: T,
    #[cfg(target_arch = "wasm32")]
    _scope: std::marker::PhantomData<&'s ()>,
}

impl<'s> Scope<'s, '_> {
    /// Starts `f` on a thread of its own; it runs once it has a slot.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn<T: Send + 's>(&self, f: impl FnOnce() -> T + Send + 's) -> Task<'s, T> {
        // Its priority: the opening task's, then the order of all spawns (unique, increasing).
        static SPAWNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut path = self.path.clone();
        path.push(SPAWNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        Task {
            handle: self.inner.spawn(move || {
                slots::acquire(path);
                let _slot = slots::Held;
                f()
            }),
        }
    }

    /// Runs `f` now (no threads in the browser).
    #[cfg(target_arch = "wasm32")]
    pub fn spawn<T: 's>(&self, f: impl FnOnce() -> T + 's) -> Task<'s, T> {
        Task {
            value: f(),
            _scope: std::marker::PhantomData,
        }
    }
}

impl<T> Task<'_, T> {
    /// The task's result; a panic in the task continues here.
    pub fn join(self) -> T {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let held = slots::holding();
            if held {
                slots::release();
            }
            let r = self.handle.join();
            if held {
                slots::acquire(slots::current_path());
            }
            r.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.value
        }
    }
}

/// `f` of every item, each a task (natively), in order.
#[cfg(not(target_arch = "wasm32"))]
pub fn map<T: Send, R: Send, const N: usize>(items: [T; N], f: impl Fn(T) -> R + Sync) -> [R; N] {
    let f = &f;
    scope(|s| items.map(|item| s.spawn(move || f(item))).map(Task::join))
}

/// `f` of every item, in order.
#[cfg(target_arch = "wasm32")]
pub fn map<T, R, const N: usize>(items: [T; N], f: impl Fn(T) -> R) -> [R; N] {
    items.map(f)
}

/// How many tasks run at a time: the machine's hardware threads.
#[cfg(not(target_arch = "wasm32"))]
pub fn limit() -> usize {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
}

/// The slots: a counter of running tasks and the waiting tasks by priority.
#[cfg(not(target_arch = "wasm32"))]
mod slots {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::sync::{Condvar, Mutex};

    struct State {
        running: usize,
        /// Waiting tasks' priorities: the smallest goes first.
        waiting: BTreeSet<Vec<u64>>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        running: 0,
        waiting: BTreeSet::new(),
    });
    static FREED: Condvar = Condvar::new();

    thread_local! {
        /// This thread's task priority (empty outside tasks).
        static PATH: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
        static HOLDS: Cell<bool> = const { Cell::new(false) };
    }

    pub fn current_path() -> Vec<u64> {
        PATH.with_borrow(Clone::clone)
    }

    pub fn holding() -> bool {
        HOLDS.get()
    }

    /// Waits for a slot as the task of priority `path`.
    pub fn acquire(path: Vec<u64>) {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.waiting.insert(path.clone());
        while st.running >= super::limit() || st.waiting.first() != Some(&path) {
            st = FREED.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.waiting.remove(&path);
        st.running += 1;
        drop(st);
        // The next in line may fit too.
        FREED.notify_all();
        PATH.set(path);
        HOLDS.set(true);
    }

    pub fn release() {
        HOLDS.set(false);
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.running -= 1;
        drop(st);
        FREED.notify_all();
    }

    /// Releases the task's slot when the task ends, panicking or not.
    pub struct Held;

    impl Drop for Held {
        fn drop(&mut self) {
            if holding() {
                release();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_return_their_results_in_order() {
        let base = 10;
        let (a, b) = scope(|s| {
            let a = s.spawn(|| base + 1);
            let b = s.spawn(|| format!("{}", base + 2));
            (a.join(), b.join())
        });
        assert_eq!((a, b.as_str()), (11, "12"));
        assert_eq!(map([1, 2, 3, 4], |x| x * base), [10, 20, 30, 40]);
    }

    /// Tasks within tasks, more than there are slots, finish: a task waiting for its own gives up
    /// its slot.
    #[test]
    fn nested_tasks_finish() {
        let n = 3 * limit();
        let total: usize = scope(|s| {
            let tasks: Vec<_> = (0..n)
                .map(|i| {
                    s.spawn(move || map([i, i + 1], |j| map([j, 1], |k| k).iter().sum::<usize>()))
                })
                .collect();
            tasks
                .into_iter()
                .map(|t| t.join().iter().sum::<usize>())
                .sum()
        });
        assert_eq!(total, (0..n).map(|i| 2 * i + 3).sum::<usize>());
    }

    #[test]
    #[should_panic(expected = "task failed")]
    fn a_panic_reaches_the_caller() {
        scope(|s| s.spawn(|| panic!("task failed")).join());
    }
}
