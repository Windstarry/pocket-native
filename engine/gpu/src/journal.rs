//! guest memory written since the GPU last read it. a title flushes its CPU's
//! data cache over anything it wrote before the GPU reads it, services and
//! DMA write memory themselves, and the GPU knows what it wrote: a texture,
//! mesh or buffer nothing in here touched since it was read is still what it
//! was, without reading it again. most titles also flush everything every
//! frame or two, which says nothing about what changed, those are kept
//! apart.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct Log {
    writes: Vec<(u32, u32)>,
    /// the stamp of the first write kept.
    start: u64,
}

static LOG: Mutex<Log> = Mutex::new(Log { writes: Vec::new(), start: 0 });

/// the stamp of now, read without taking the log, which most checks find
/// unchanged since their last.
static NOW: AtomicU64 = AtomicU64::new(0);

/// writes kept before the oldest go, what was read before them is read
/// again.
const KEPT: usize = 1 << 14;

/// a flush this big is a title flushing everything, which tells nothing
/// about what changed.
pub const BULK: u32 = 16 << 20;

/// len bytes at addr were written.
pub fn note(addr: u32, len: u32) {
    if len == 0 {
        return;
    }
    let mut log = LOG.lock().unwrap();
    if log.writes.last() == Some(&(addr, len)) {
        return;
    }
    if log.writes.len() == KEPT {
        log.start += KEPT as u64;
        log.writes.clear();
    }
    log.writes.push((addr, len));
    NOW.store(log.start + log.writes.len() as u64, Ordering::Release);
}

/// the stamp of now, to take before reading what it is to cover.
pub fn now() -> u64 {
    NOW.load(Ordering::Acquire)
}

/// what wrote over len bytes at addr since stamp, and the stamp of now.
pub fn written_since(stamp: u64, addr: u32, len: u32) -> (Written, u64) {
    if NOW.load(Ordering::Acquire) == stamp {
        return (Written::Nothing, stamp);
    }
    let log = LOG.lock().unwrap();
    let now = log.start + log.writes.len() as u64;
    if stamp < log.start {
        return (Written::Something, now);
    }
    let end = addr.saturating_add(len);
    let mut written = Written::Nothing;
    for &(at, size) in &log.writes[(stamp - log.start) as usize..] {
        if at < end && addr < at.saturating_add(size) {
            if size < BULK {
                return (Written::Something, now);
            }
            written = Written::Everything;
        }
    }
    (written, now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    Nothing,
    /// a flush of everything, it may have changed or not.
    Everything,
    /// a write over it.
    Something,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_over_a_range_since_a_stamp() {
        let stamp = now();
        assert_eq!(written_since(stamp, 0x7000_0000, 0x100).0, Written::Nothing);
        note(0x7000_0080, 4);
        let (written, later) = written_since(stamp, 0x7000_0000, 0x100);
        assert_eq!(written, Written::Something);
        assert_eq!(written_since(later, 0x7000_0000, 0x100).0, Written::Nothing);
        note(0x7000_0100, 4);
        assert_eq!(written_since(later, 0x7000_0000, 0x100).0, Written::Nothing, "just past it");
        note(0x6F00_0000, BULK + 0x0100_0000);
        assert_eq!(written_since(later, 0x7000_0000, 0x100).0, Written::Everything);
    }
}
