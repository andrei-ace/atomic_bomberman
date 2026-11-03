use super::{
    prepare::{check_cancel, Cancelled},
    resources,
};
use anyhow::{ensure, Result};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Retains completed results and permits one smaller-pool retry, after all
/// workers from the failed attempt have stopped. Cancellation never retries.
pub(super) fn resilient<T: Send>(
    count: usize,
    workers: usize,
    cancel: &AtomicBool,
    operation: impl Fn(usize, &AtomicBool, usize) -> Result<T> + Sync,
    mut complete: impl FnMut(usize, T) -> Result<()>,
    retryable: impl Fn(&anyhow::Error) -> bool,
    mut retrying: impl FnMut(),
) -> Result<usize> {
    let mut done = vec![false; count];
    let mut attempt = |workers| {
        let pending: Vec<_> = done
            .iter()
            .enumerate()
            .filter_map(|(i, done)| (!done).then_some(i))
            .collect();
        run(
            pending.len(),
            workers,
            cancel,
            |slot, stop| operation(pending[slot], stop, workers),
            |slot, value| {
                let index = pending[slot];
                complete(index, value)?;
                done[index] = true;
                Ok(())
            },
        )
    };
    if let Err(error) = attempt(workers) {
        check_cancel(cancel)?;
        if workers == 1 || !retryable(&error) {
            return Err(error);
        }
        retrying();
        attempt(1)?;
        return Ok(1);
    }
    Ok(workers)
}

/// All work is scoped and joined before returning, including errors/cancellation.
/// A stopped helper never outlives its private work directory or cache lock.
pub(super) fn run<T: Send>(
    count: usize,
    workers: usize,
    cancel: &AtomicBool,
    operation: impl Fn(usize, &AtomicBool) -> Result<T> + Sync,
    mut complete: impl FnMut(usize, T) -> Result<()>,
) -> Result<()> {
    ensure!(workers > 0 && workers <= 4, "Invalid HD worker count");
    check_cancel(cancel)?;
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let allowed = AtomicUsize::new(workers);
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        let mut failure = None;
        for worker in 0..workers {
            let sender = sender.clone();
            let (next, stop, allowed, operation) = (&next, &stop, &allowed, &operation);
            let handle = std::thread::Builder::new()
                .name(format!("hd-asset-{worker}"))
                .spawn_scoped(scope, move || loop {
                    if stop.load(Ordering::Acquire)
                        || cancel.load(Ordering::Acquire)
                        || next.load(Ordering::Acquire) >= count
                    {
                        break;
                    }
                    if worker >= allowed.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    let index = next.fetch_add(1, Ordering::AcqRel);
                    if index >= count {
                        break;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        operation(index, stop)
                    }))
                    .unwrap_or_else(|_| {
                        Err(anyhow::anyhow!(
                            "An HD preparation worker stopped unexpectedly"
                        ))
                    });
                    if result
                        .as_ref()
                        .is_err_and(|e| e.downcast_ref::<Cancelled>().is_some())
                        && stop.load(Ordering::Acquire)
                    {
                        break;
                    }
                    if result.is_err() {
                        stop.store(true, Ordering::Release);
                    }
                    if sender.send((index, result)).is_err() {
                        stop.store(true, Ordering::Release);
                        break;
                    }
                });
            match handle {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    failure = Some(anyhow::Error::new(error).context("start HD workers"));
                    break;
                }
            }
        }
        drop(sender);
        let mut last_memory_check = Instant::now();
        loop {
            if cancel.load(Ordering::Acquire) {
                stop.store(true, Ordering::Release);
            }
            if last_memory_check.elapsed() >= Duration::from_secs(2) {
                if let Some(bytes) = resources::available_memory() {
                    let limit = if bytes < 2 * 1024 * 1024 * 1024 {
                        1
                    } else if bytes < 4 * 1024 * 1024 * 1024 {
                        workers.min(2)
                    } else {
                        workers
                    };
                    allowed.store(limit, Ordering::Release);
                }
                last_memory_check = Instant::now();
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok((index, Ok(value))) => {
                    if let Err(error) = complete(index, value) {
                        stop.store(true, Ordering::Release);
                        failure.get_or_insert(error);
                    }
                }
                Ok((_, Err(error))) => {
                    stop.store(true, Ordering::Release);
                    failure.get_or_insert(error);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        for handle in handles {
            if handle.join().is_err() {
                failure.get_or_insert_with(|| anyhow::anyhow!("An HD worker could not shut down"));
            }
        }
        check_cancel(cancel)?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn bounds_real_parallel_work_and_merges_every_result_once() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let mut done = BTreeSet::new();
        run(
            12,
            3,
            &AtomicBool::new(false),
            |index, _| {
                let n = active.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(n, Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(20));
                active.fetch_sub(1, Ordering::AcqRel);
                Ok(index)
            },
            |index, value| {
                assert_eq!(index, value);
                assert!(done.insert(index));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(done.len(), 12);
        assert!((2..=3).contains(&peak.load(Ordering::Acquire)));
        assert_eq!(active.load(Ordering::Acquire), 0);
    }
    #[test]
    fn cancellation_stops_the_pool_before_claiming_all_assets() {
        let cancelled = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let result = run(
            100,
            3,
            &cancelled,
            |index, stop| {
                calls.fetch_add(1, Ordering::AcqRel);
                if index == 2 {
                    cancelled.store(true, Ordering::Release);
                }
                std::thread::sleep(Duration::from_millis(20));
                check_cancel(stop)?;
                Ok(index)
            },
            |_, _| Ok(()),
        );
        assert!(result.unwrap_err().downcast_ref::<Cancelled>().is_some());
        assert!(calls.load(Ordering::Acquire) < 100);
    }

    #[test]
    fn failure_stops_and_joins_other_workers_before_returning() {
        let started = std::sync::Barrier::new(3);
        let active = AtomicUsize::new(0);
        let result = run(
            100,
            3,
            &AtomicBool::new(false),
            |index, stop| {
                active.fetch_add(1, Ordering::AcqRel);
                started.wait();
                if index == 0 {
                    active.fetch_sub(1, Ordering::AcqRel);
                    anyhow::bail!("injected GPU failure");
                }
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                active.fetch_sub(1, Ordering::AcqRel);
                check_cancel(stop)?;
                Ok(())
            },
            |_, ()| Ok(()),
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("injected GPU failure"));
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn retry_keeps_completed_assets_and_only_repeats_unfinished_work() {
        let first_merged = AtomicBool::new(false);
        let calls: Vec<_> = (0..5).map(|_| AtomicUsize::new(0)).collect();
        let mut values = BTreeSet::new();
        let mut retries = 0;
        let workers = resilient(
            5,
            2,
            &AtomicBool::new(false),
            |index, stop, workers| {
                calls[index].fetch_add(1, Ordering::AcqRel);
                if index == 1 && workers > 1 {
                    while !first_merged.load(Ordering::Acquire) {
                        check_cancel(stop)?;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    anyhow::bail!("injected inference failure");
                }
                check_cancel(stop)?;
                Ok(index)
            },
            |index, value| {
                assert_eq!(index, value);
                assert!(values.insert(value));
                if index == 0 {
                    first_merged.store(true, Ordering::Release);
                }
                Ok(())
            },
            |e| e.to_string().contains("inference"),
            || retries += 1,
        )
        .unwrap();
        assert_eq!(workers, 1);
        assert_eq!(retries, 1);
        assert_eq!(values.len(), 5);
        assert_eq!(calls[0].load(Ordering::Acquire), 1);
        assert_eq!(calls[1].load(Ordering::Acquire), 2);
    }

    #[test]
    fn serial_failure_and_user_cancellation_do_not_keep_retrying() {
        let mut retries = 0;
        let result = resilient::<()>(
            8,
            2,
            &AtomicBool::new(false),
            |_, _, _| anyhow::bail!("GPU unavailable"),
            |_, ()| Ok(()),
            |_| true,
            || retries += 1,
        );
        assert!(result.unwrap_err().to_string().contains("GPU unavailable"));
        assert_eq!(retries, 1);
        let result = resilient::<()>(
            8,
            2,
            &AtomicBool::new(true),
            |_, _, _| unreachable!(),
            |_, ()| Ok(()),
            |_| true,
            || retries += 1,
        );
        assert!(result.unwrap_err().downcast_ref::<Cancelled>().is_some());
        assert_eq!(retries, 1);
    }
}
