//! SysEx out (#440): whole System Exclusive messages to hardware, a DX7
//! voice to a Volca FM, colours for a controller's pads, a setting that
//! has no CC.
//!
//! A [`Message`](super::midi::Message) is at most three bytes, and a SysEx
//! message is as long as it needs to be: a DX7 voice is 163 bytes, a bank
//! of 32 is 4,104. So there are two ways in, and both end in the sender,
//! which sends each message whole, in order, within its port's budget
//! (see `midi_sender`):
//!
//! - **From the UI**, the common case (a voice dump, pad colours, a
//!   `.syx` file sent from the Rig), through
//!   [`MidiBus::send_sysex`](super::midi::MidiBus::send_sysex): the UI may
//!   allocate, so the bytes go to the sender as they are, any length up to
//!   [`MAX_UI`].
//! - **From the audio thread**, for a send timed to a sample, through a
//!   [`SysexPool`]: [`SLOTS`] buffers of [`SLOT_BYTES`], made before the
//!   audio starts. A module claims a free slot, fills it, and sends the
//!   slot's number down its ring
//!   ([`MidiWriter::sysex`](super::midi_writer::MidiWriter::sysex)); the
//!   sender sends the bytes when their sample is heard and frees the slot.
//!   Claiming is a compare-and-swap, so nothing allocates, locks or waits.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The first byte of every SysEx message.
pub const START: u8 = 0xF0;
/// The last.
pub const END: u8 = 0xF7;

/// Slots in the audio thread's pool: more messages than a block sends.
pub const SLOTS: usize = 32;
/// The longest message a slot holds: room for a DX7 bank of 32 voices
/// (4,104 bytes), the longest dump the Rig's gear takes in one message.
pub const SLOT_BYTES: usize = 8192;
/// The longest message the UI may send: far more than any dump the Rig's
/// gear takes, and a guard against a file that isn't SysEx at all.
pub const MAX_UI: usize = 1 << 20;

/// Why a SysEx message wasn't sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysexError {
    /// It doesn't begin with F0 and end with F7, or a byte between them
    /// has its top bit set.
    NotSysex,
    /// It's longer than a slot, or than the UI may send.
    TooLong,
    /// Every slot of the pool is in use: the sender is still sending them.
    Full,
    /// No sender drains the bus (the offline renderer), so nothing would
    /// send it.
    NoSender,
}

impl std::fmt::Display for SysexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotSysex => "it isn't SysEx: a message begins with F0 and ends with F7",
            Self::TooLong => "it's too long to send",
            Self::Full => "the SysEx waiting to go has filled the queue",
            Self::NoSender => "MIDI out isn't running here",
        })
    }
}

/// Whether `bytes` is one whole SysEx message: F0, data bytes (top bit
/// clear), F7. REAL-TIME SAFE.
pub fn check(bytes: &[u8]) -> Result<(), SysexError> {
    match bytes {
        [START, data @ .., END] if data.iter().all(|&b| b < 0x80) => Ok(()),
        _ => Err(SysexError::NotSysex),
    }
}

/// The messages in a `.syx` file, in order: everything from each F0 to
/// the F7 after it. Real-time bytes (clock, a recorder's Active Sensing)
/// caught between are left out, as a device would ignore them. Anything
/// else outside a message, or a message cut short, means it isn't SysEx.
pub fn split(file: &[u8]) -> Result<Vec<&[u8]>, SysexError> {
    let mut messages = Vec::new();
    let mut at = 0;
    while at < file.len() {
        match file[at] {
            START => {
                let length = file[at..].iter().position(|&b| b == END).ok_or(SysexError::NotSysex)? + 1;
                let message = &file[at..at + length];
                check(message)?;
                messages.push(message);
                at += length;
            }
            byte if byte >= 0xF8 => at += 1,
            _ => return Err(SysexError::NotSysex),
        }
    }
    if messages.is_empty() {
        return Err(SysexError::NotSysex);
    }
    Ok(messages)
}

/// What a slot holds while it's in use.
struct Bytes {
    len: usize,
    bytes: [u8; SLOT_BYTES],
}

struct Slot {
    /// Whether a module or the sender holds it. Taken with Acquire and
    /// given back with Release, so whoever takes it next sees the bytes
    /// the last holder left and writes over nothing still being read.
    used: AtomicBool,
    /// Touched only by the slot's holder.
    bytes: Box<UnsafeCell<Bytes>>,
}

/// The audio thread's SysEx buffers: [`SLOTS`] of [`SLOT_BYTES`], made on
/// the UI thread before any module claims one.
pub struct SysexPool {
    slots: Box<[Slot]>,
    /// Where the next claim starts looking, so slots take turns.
    next: AtomicUsize,
}

// SAFETY: a slot's bytes are touched only by whoever holds it, and it's
// held by one at a time: claimed with a compare-and-swap, given back by
// the holder alone (see `Slot::used`).
unsafe impl Sync for SysexPool {}

impl SysexPool {
    /// The pool, allocated here: a quarter of a megabyte.
    pub fn new() -> Self {
        let slots = (0..SLOTS).map(|_| Slot { used: AtomicBool::new(false), bytes: Box::new(UnsafeCell::new(Bytes { len: 0, bytes: [0; SLOT_BYTES] })) }).collect();
        Self { slots, next: AtomicUsize::new(0) }
    }

    /// Copies `message` into a free slot, and holds it until the lease is
    /// dropped or sent ([`Lease::into_slot`]). REAL-TIME SAFE: no locks, no
    /// allocation, never waits.
    pub fn claim(&self, message: &[u8]) -> Result<Lease<'_>, SysexError> {
        check(message)?;
        if message.len() > SLOT_BYTES {
            return Err(SysexError::TooLong);
        }
        let first = self.next.fetch_add(1, Ordering::Relaxed);
        for k in 0..SLOTS {
            let index = (first + k) % SLOTS;
            let slot = &self.slots[index];
            if slot.used.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                // SAFETY: the swap made this slot ours alone
                let held = unsafe { &mut *slot.bytes.get() };
                held.bytes[..message.len()].copy_from_slice(message);
                held.len = message.len();
                return Ok(Lease { pool: self, index });
            }
        }
        Err(SysexError::Full)
    }

    /// Takes up the slot a module sent down its ring. The sender's side:
    /// `index` came from [`Lease::into_slot`], once.
    pub fn adopt(&self, index: u8) -> Option<Lease<'_>> {
        let slot = self.slots.get(index as usize)?;
        slot.used.load(Ordering::Acquire).then_some(Lease { pool: self, index: index as usize })
    }

    /// Slots in use now.
    pub fn in_use(&self) -> usize {
        self.slots.iter().filter(|slot| slot.used.load(Ordering::Relaxed)).count()
    }
}

impl Default for SysexPool {
    fn default() -> Self {
        Self::new()
    }
}

/// A slot held, with its message. Dropping it frees the slot.
pub struct Lease<'a> {
    pool: &'a SysexPool,
    index: usize,
}

impl Lease<'_> {
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: the slot is held by this lease alone
        let held = unsafe { &*self.pool.slots[self.index].bytes.get() };
        &held.bytes[..held.len]
    }

    /// The slot's number, to send down a ring, still held: the sender
    /// takes it up with [`SysexPool::adopt`] and frees it once it's sent.
    pub fn into_slot(self) -> u8 {
        let index = self.index as u8;
        std::mem::forget(self);
        index
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.pool.slots[self.index].used.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_f0_data_f7() {
        assert_eq!(check(&[0xF0, 0x43, 0x10, 0xF7]), Ok(()));
        assert_eq!(check(&[0xF0, 0xF7]), Ok(()), "empty, but whole");
        assert_eq!(check(&[0xF0, 0x43, 0x10]), Err(SysexError::NotSysex), "cut short");
        assert_eq!(check(&[0x90, 60, 100]), Err(SysexError::NotSysex));
        assert_eq!(check(&[0xF0, 0x90, 0xF7]), Err(SysexError::NotSysex), "a status byte inside");
        assert_eq!(check(&[]), Err(SysexError::NotSysex));
    }

    #[test]
    fn a_file_splits_into_its_messages() {
        let file = [0xF0, 1, 2, 0xF7, 0xFE, 0xF0, 3, 0xF7];
        assert_eq!(split(&file), Ok(vec![&[0xF0, 1, 2, 0xF7][..], &[0xF0, 3, 0xF7]]), "Active Sensing between is left out");
        assert_eq!(split(&[0xF0, 1, 0xF7, 0x90, 60, 100]), Err(SysexError::NotSysex), "a note isn't SysEx");
        assert_eq!(split(&[0xF0, 1, 2]), Err(SysexError::NotSysex), "cut short");
        assert_eq!(split(b"{\"patch\": 1}"), Err(SysexError::NotSysex));
        assert_eq!(split(&[]), Err(SysexError::NotSysex), "nothing to send");
    }

    #[test]
    fn slots_are_claimed_sent_and_freed() {
        let pool = SysexPool::new();
        let voice: Vec<u8> = std::iter::once(START).chain((0..161).map(|i| (i % 128) as u8)).chain(std::iter::once(END)).collect();
        let lease = pool.claim(&voice).unwrap();
        assert_eq!(lease.bytes(), &voice[..]);
        let slot = lease.into_slot();
        assert_eq!(pool.in_use(), 1, "held while it's on its way");
        let adopted = pool.adopt(slot).unwrap();
        assert_eq!(adopted.bytes(), &voice[..], "the sender reads what was written");
        drop(adopted);
        assert_eq!(pool.in_use(), 0);
        assert!(pool.adopt(slot).is_none(), "a free slot isn't anyone's");
        assert!(pool.adopt(200).is_none());
    }

    #[test]
    fn a_full_pool_says_so_and_a_long_message_doesnt_fit() {
        let pool = SysexPool::new();
        let leases: Vec<_> = (0..SLOTS).map(|_| pool.claim(&[START, 1, END]).unwrap()).collect();
        assert_eq!(pool.claim(&[START, 2, END]).err(), Some(SysexError::Full));
        drop(leases);
        assert!(pool.claim(&[START, 2, END]).is_ok(), "free again");
        let mut long = vec![0; SLOT_BYTES + 1];
        long[0] = START;
        long[SLOT_BYTES] = END;
        assert_eq!(pool.claim(&long).err(), Some(SysexError::TooLong));
        let mut bank = vec![0; 4104];
        bank[0] = START;
        bank[4103] = END;
        assert!(pool.claim(&bank).is_ok(), "a DX7 bank fits");
    }

    #[test]
    fn two_threads_claim_without_sharing_a_slot() {
        let pool: &'static SysexPool = Box::leak(Box::new(SysexPool::new()));
        let threads: Vec<_> = (0..4u8)
            .map(|t| {
                std::thread::spawn(move || {
                    for k in 0..2000u32 {
                        let fill = (t * 16 + (k % 16) as u8) & 0x7F;
                        if let Ok(lease) = pool.claim(&[START, fill, fill, fill, END]) {
                            assert_eq!(lease.bytes(), &[START, fill, fill, fill, END], "no one wrote over it");
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(pool.in_use(), 0);
    }
}
