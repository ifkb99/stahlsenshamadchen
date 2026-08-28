//! Running a known batch of jobs across the machine, in job order.

/// How many battles may be in the air at once.
///
/// A static rather than a parameter threaded through five call sites, because
/// it is a property of the machine rather than of any table. `--jobs` is what
/// makes this harness usable *beside itself*: two sweeps at `--jobs 4` on an
/// eight-core box finish in about the time one at full width would, and
/// neither one's numbers move, because the folding is by seed either way.
pub static JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn thread_budget() -> usize {
    let asked = JOBS.load(std::sync::atomic::Ordering::Relaxed);
    if asked > 0 {
        return asked;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Run a known list of independent jobs across the machine, keeping the
/// results in the order the jobs were given in.
///
/// The order is the whole point and is why this is not a channel: every job
/// writes into the slot it owns, so a caller folding the results is folding
/// them in job order however the threads interleaved. A sweep hands this one
/// flat list holding *every* variant's battles rather than one batch per
/// variant, because batching would idle most of the machine at the end of
/// each batch while its slowest battle finished.
///
/// `std::thread::scope` rather than a work-stealing pool: the batch is known
/// up front, the jobs are within a factor of a few of each other, and it
/// costs no dependency. Chunked by slice rather than by index so the borrow
/// checker proves no two threads touch the same slot.
pub fn run_all<J: Sync, T: Send>(jobs: &[J], each: impl Fn(&J) -> T + Sync) -> Vec<T> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Option<T>> = (0..jobs.len()).map(|_| None).collect();
    let threads = thread_budget().min(jobs.len());
    let per = jobs.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let each = &each;
        let mut rest = out.as_mut_slice();
        let mut queue = jobs;
        while !rest.is_empty() {
            let take = per.min(rest.len());
            let (mine, tail) = rest.split_at_mut(take);
            let (theirs, others) = queue.split_at(take);
            scope.spawn(move || {
                for (slot, job) in mine.iter_mut().zip(theirs) {
                    *slot = Some(each(job));
                }
            });
            queue = others;
            rest = tail;
        }
    });
    out.into_iter()
        .map(|o| o.expect("every job in the batch ran"))
        .collect()
}
