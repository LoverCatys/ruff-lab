//! Dedicated submission-sending thread (`VICTORY_SEND_THREAD=1`, on by default
//! when `late()` is enabled; `VICTORY_SEND_THREAD=0` sends inline).
//!
//! Ported from `Kemerd/VIctory-NVIDIA-RT` (`d06b7ab`, `8b40230`, `4c0d0e9`):
//! a submission sent ahead (`Host::submit` with `late`) previously ended on the
//! submitting thread with `ExecuteCommandLists`, `Signal`, and `Host::card_sent`.
//! Handing that tail to a dedicated `victory-send` thread lets the submitting
//! thread immediately begin preparing the next submission.
//!
//! Ordering invariants:
//! 1. All title-memory refreshes, snapshots, and list closes finish before `Job`
//!    is handed over; the send thread never reads title memory.
//! 2. `*submitted` (the fence value) is reserved on the submitting thread before
//!    handing over, and the `Flight` enters `line()` in the same order.
//! 3. Any code that puts work directly onto `host.queue` (`card_before`,
//!    `relay_fetch`, `card_back`, synchronous `submit`, and `present`) calls
//!    [`wait_idle`] first so its work strictly follows every handed-over job.
//! 4. The send thread never acquires `QUEUE_ORDER`, `self.submitted`, `line()`,
//!    `CARD_WORK`, or `self.presenter`, so `wait_idle()` cannot deadlock.
//! 5. Every job signals its fence even on error or panic so waiters never hang.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Instant;

use super::*;

/// Whether submissions sent ahead are dispatched by the dedicated `victory-send`
/// thread (`VICTORY_SEND_THREAD=0` disables and sends inline on the submitting
/// thread). Only active when [`late`] is enabled.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| late() && std::env::var_os("VICTORY_SEND_THREAD").is_none_or(|v| v != "0"))
}

/// The tail of a submission sent ahead: its closed lists and the fence value
/// to signal behind them.
pub(super) struct Job {
    lists: Vec<Option<ID3D12CommandList>>,
    fence: u64,
    sent_as: u64,
    signalled: bool,
}

// SAFETY: closed D3D12 command lists are free-threaded; nothing records into
// them again before the fence completes.
unsafe impl Send for Job {}

impl Job {
    pub(super) fn new(lists: Vec<Option<ID3D12CommandList>>, fence: u64, sent_as: u64) -> Self {
        Self {
            lists,
            fence,
            sent_as,
            signalled: false,
        }
    }

    /// Executes the lists on `host.queue`, signals `self.fence`, and updates
    /// the card clock in the exact order `Host::submit` used inline.
    pub(super) fn run(&mut self, host: &Host) -> windows::core::Result<()> {
        // SAFETY: closed lists; the fence and queue belong to `host`.
        let signal = unsafe {
            host.queue.ExecuteCommandLists(&self.lists);
            host.queue.Signal(&host.fence, self.fence)
        };
        self.signalled = true;
        host.card_sent();
        signal
    }
}

struct State {
    jobs: VecDeque<Job>,
    handed: u64,
    done: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    jobs: VecDeque::new(),
    handed: 0,
    done: 0,
});
static WORK: Condvar = Condvar::new();
static IDLE: Condvar = Condvar::new();

/// [0] jobs sent, [1] ns busy in `Job::run`, [2] blocking waits in `wait_idle`,
/// [3] ns waited in `wait_idle`, [4] out-of-order jobs observed.
static STATS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Queues `job` for the `victory-send` thread, spawning it on the first call.
/// Returns `Err(job)` if thread creation failed so the caller can send inline.
pub(super) fn hand_over(job: Job) -> Result<(), Job> {
    static STARTED: OnceLock<bool> = OnceLock::new();
    let started = *STARTED.get_or_init(|| {
        match std::thread::Builder::new()
            .name("victory-send".into())
            .spawn(worker)
        {
            Ok(_) => {
                tracing::info!("submissions sent ahead are sent by a dedicated thread (VICTORY_SEND_THREAD=1)");
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not spawn victory-send thread; sending inline");
                false
            }
        }
    });
    if !started {
        return Err(job);
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.jobs.push_back(job);
    state.handed += 1;
    drop(state);
    WORK.notify_one();
    Ok(())
}

/// Blocks until every job previously handed to [`hand_over`] has executed on
/// `host.queue` and signalled its fence. Fast no-op when idle or disabled.
pub(super) fn wait_idle() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.handed == state.done {
        return;
    }
    let began = Instant::now();
    while state.handed != state.done {
        state = IDLE.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    drop(state);
    STATS[2].fetch_add(1, Ordering::Relaxed);
    STATS[3].fetch_add(began.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

/// Checks monotonic fence and card-plan ordering; updates `last` regardless.
fn in_order(last: &mut (u64, u64), fence: u64, sent_as: u64) -> bool {
    let ok = fence > last.0 && (sent_as == 0 || sent_as >= last.1);
    last.0 = fence;
    last.1 = last.1.max(sent_as);
    ok
}

fn worker() {
    let Some(host) = host() else {
        fail(format_args!("no host device for victory-send thread (VICTORY_SEND_THREAD=1)"));
    };
    let mut last = (0u64, 0u64);
    loop {
        let mut job = {
            let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = state.jobs.pop_front() {
                    break job;
                }
                state = WORK.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        if !in_order(&mut last, job.fence, job.sent_as) {
            let seen = STATS[4].fetch_add(1, Ordering::Relaxed);
            if seen < 4 {
                tracing::warn!(
                    fence = job.fence,
                    sent_as = job.sent_as,
                    "submission reached victory-send out of order (VICTORY_SEND_THREAD=1)"
                );
            }
        }
        let began = Instant::now();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| job.run(host)));
        let busy = began.elapsed().as_nanos() as u64;
        if outcome.is_err() && !job.signalled {
            // SAFETY: our queue and fence; `job.fence` was reserved for this job.
            let _ = unsafe { host.queue.Signal(&host.fence, job.fence) };
        }
        STATS[0].fetch_add(1, Ordering::Relaxed);
        STATS[1].fetch_add(busy, Ordering::Relaxed);
        {
            let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.done += 1;
        }
        IDLE.notify_all();
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => fail(format_args!(
                "a submission sent ahead could not be followed by a signal ({e})"
            )),
            Err(_) => fail(format_args!("victory-send thread panicked")),
        }
    }
}

/// Logs per-frame send-thread statistics when any jobs were sent.
pub(super) fn line(frames: f64) {
    let frames = frames.max(1.0);
    let s: [f64; 5] = std::array::from_fn(|i| STATS[i].swap(0, Ordering::Relaxed) as f64);
    if s[0] <= 0.0 && s[2] <= 0.0 && s[4] <= 0.0 {
        return;
    }
    tracing::info!(
        "victory-send per frame (VICTORY_SEND_THREAD=1): {:.1} jobs, {:.2} ms in ExecuteCommandLists+Signal, \
         {:.2} ms in {:.1} waits, {:.0} out of order",
        s[0] / frames,
        s[1] / 1e6 / frames,
        s[3] / 1e6 / frames,
        s[2] / frames,
        s[4] / frames,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rising_fences_are_in_order() {
        let mut last = (0, 0);
        assert!(in_order(&mut last, 1, 0));
        assert!(in_order(&mut last, 2, 0));
        assert!(in_order(&mut last, 5, 0));
        assert_eq!(last.0, 5);
    }

    #[test]
    fn a_plan_number_of_zero_is_ignored() {
        let mut last = (0, 0);
        assert!(in_order(&mut last, 1, 7));
        assert!(in_order(&mut last, 2, 0));
        assert!(in_order(&mut last, 3, 7));
        assert!(in_order(&mut last, 4, 9));
    }

    #[test]
    fn a_lower_plan_number_is_flagged() {
        let mut last = (0, 0);
        assert!(in_order(&mut last, 1, 9));
        assert!(!in_order(&mut last, 2, 8));
        assert_eq!(last.1, 9);
        assert!(!in_order(&mut last, 3, 8));
    }

    #[test]
    fn a_repeated_or_lower_fence_is_flagged() {
        let mut last = (0, 0);
        assert!(in_order(&mut last, 4, 0));
        assert!(!in_order(&mut last, 4, 0));
        assert!(!in_order(&mut last, 3, 0));
        assert!(in_order(&mut last, 5, 0));
    }
}
