//! Implements various forms of ring buffer.
//! 
//! These implementations are tuned for x86-64 architectures (64 byte cache line size alignments to avoid false sharing).

use std::cell::UnsafeCell;
use std::ptr::copy_nonoverlapping;
use std::sync::atomic::{AtomicU32, Ordering, AtomicUsize};

/// Ring buffer element count
const BUFFER_SIZE: usize = 1024;

/// Size (count, number of elements) of a "catch up" block when reading many elements at once without allocating
const BLOCK_SIZE_CATCH_UP: usize = 64;

// --------------------------------- V1 ---------------------------------------

/// `BufferIndex` is a helper type to force memory alignment (to avoid false sharing of cache lines). In Rust, unlike in C++, we cannot call `alignas(bytes)` on individual struct fields.
#[repr(align(64))]
struct BufferIndex {
    value: AtomicUsize
}

impl BufferIndex {
    fn new(value: usize) -> BufferIndex {
        BufferIndex { value: AtomicUsize::new(value) }
    }
}

/// Ring Buffer, Version 1 with Double Index on the header
/// 
/// Typically the array is bytes. Writer updates pending index, then data, then commit index.
/// 
/// It's fast, but according to [the talk](https://www.youtube.com/watch?v=8uAW5FQtcvE), it's not optimal because this structure has lots of cache contention on the indices under load.
/// 
/// # Unsafe
/// 
/// This is unsafe because we are hacking around Rust's mutability/referencing rules to avoid needing a lock (a Mutex). Hence why the signature for a "mutable" operation `write()` is `&self`.
/// 
/// # Warnings
/// 
/// - Writers can overrun the readers. There is no lock or wait, writers will just keep pushing data through the buffer.
#[repr(align(64))]
pub struct RingBufferV1<T: Copy + Default> {
    buf: UnsafeCell<[T; BUFFER_SIZE]>,
    index_pending: BufferIndex,
    index_committed: BufferIndex,
}

unsafe impl<T: Copy + Default> Send for RingBufferV1<T> {}
unsafe impl<T: Copy + Default> Sync for RingBufferV1<T> {}

impl <T: Copy + Default> RingBufferV1<T> {
    pub fn new() -> RingBufferV1<T> {
        RingBufferV1 {
            buf: [T::default(); BUFFER_SIZE].into(),
            index_pending: BufferIndex::new(0),
            index_committed: BufferIndex::new(0)
        }
    }

    pub fn default() -> Self {
        Self::new()
    }

    /// Puts new data into the buffer with a `memcpy`, handling wrapping
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it interacts with the `UnsafeCell<Buffer>` field. We `memcpy` to write to the buffer through the cell to avoid a lock.
    pub unsafe fn write(&self, data: &[T]) {
        // Push pending index forward
        let mut new_p_index: usize = self.index_pending.value.load(Ordering::Relaxed);
        let index_c = self.index_committed.value.load(Ordering::Relaxed);
        new_p_index = (new_p_index + data.len()) % BUFFER_SIZE;
        // Notify consumers using `release`
        self.index_pending.value.store(new_p_index, Ordering::Release);
        // Copy into buffer
        let buffer = (*self.buf.get()).as_mut_ptr();
        if new_p_index < index_c {
            // CASE where index wraps around the "back" of the buffer, so we split the write
            let n_to_tail = BUFFER_SIZE - 1 - index_c;
            let ptr_to_back_of_buffer = buffer.offset(index_c as isize);
            copy_nonoverlapping(data[..n_to_tail].as_ptr(), ptr_to_back_of_buffer, n_to_tail);
            copy_nonoverlapping(data[n_to_tail..].as_ptr(), buffer, data.len() - n_to_tail);
        } else {
            // CASE where we have a linear write
            copy_nonoverlapping(data.as_ptr(), buffer.offset(index_c as isize), new_p_index - index_c);
        }
        // Push index forward and notify consumers
        self.index_committed.value.store(new_p_index, Ordering::Release);
    }

    /// Copy out the next new data point to the reader 
    /// 
    /// None implies no new data, we Box because we need a size to make Rust happy (and data is copied so `Arc` is overkill)
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it interacts with the `UnsafeCell<Buffer>` field. We `memcpy` to read from the buffer through the cell to avoid a lock.
    pub unsafe fn read_single(&self, reader_index: usize) -> (usize, Option<T>) {
        let index_c: usize = self.index_committed.value.load(Ordering::Acquire);
        if index_c == reader_index || reader_index > BUFFER_SIZE {
            // CASE reader is up to date, nothing to return
            return (index_c, None)
        } else {
            // CASE reader is behind, send a single value
            let buffer_dereferenced = *self.buf.get();
            if (reader_index) < BUFFER_SIZE {
                return (index_c, Some(buffer_dereferenced[reader_index]));
            } else {
                return (index_c, Some(buffer_dereferenced[0]));
            }
        }
    }

    /// Copy out a block of messages, essentially a windowing pattern to avoid allocation.
    /// 
    /// If the block extends "past" the written messages the extra values will be `T::default()`, and the third position of the output will mark the (starting) offset of these values.
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it interacts with the `UnsafeCell<Buffer>` field. We `memcpy` to read from the buffer through the cell to avoid a lock.
    pub unsafe fn read_block(&self, reader_index: usize) -> (usize, Option<[T; BLOCK_SIZE_CATCH_UP]>, Option<usize>) {
        unimplemented!();
    }

    /// Copy out all "unread" data points
    /// 
    /// Unread is determined by finding the delta between the provided reader index and the commit index, accounting for wrap around.
    /// 
    /// # Warning (Performance)
    /// 
    /// This allocates (given a Vec<T> return type). Rust doesn't like variable return sizes and I'm not sure of the best way to work around this currently.
    /// 
    /// # Safety
    /// 
    /// This function is unsafe because it interacts with the `UnsafeCell<Buffer>` field. We `memcpy` to read from the buffer through the cell to avoid a lock.
    pub unsafe fn read_unread(&self, reader_index: usize) -> (usize, Option<Vec<T>>) {
        let index_c: usize = self.index_committed.value.load(Ordering::Acquire);
        if index_c == reader_index {
            // CASE reader is up to date, nothing to return
            return (index_c, None)
        } else {
            let buffer_dereferenced = *self.buf.get();
            let n_to_tail = BUFFER_SIZE - reader_index;
            // May not be ideal, but we can allocate in the reader since it shouldn't block the writer
            if reader_index > index_c {
                // CASE when wrapping (measure from point of last read to end, then from beginning to commit index)
                let mut output: Vec<T> = vec![T::default(); index_c + n_to_tail];
                // Fill *front* out output vector with *back* of source buffer
                // We have to split here, indexing into dst with `copy_from_slice` isn't recognized
                let (left, right) = output.split_at_mut(n_to_tail);
                println!("Reader index: {}, index_c: {}, n_to_tail: {}", reader_index, index_c, n_to_tail);
                println!("Output: {}, left: {}, right: {}", index_c + n_to_tail, left.len(), right.len());
                left.copy_from_slice(&buffer_dereferenced[reader_index..BUFFER_SIZE]);
                // Now fill *back* of output vector with *front* of source buffer
                right.copy_from_slice(&buffer_dereferenced[..index_c]);
                return (index_c, Some(output));
            } else {
                // CASE when reading linear block
                // Here we have to fill the vector with something - allocating leaves the length as zero and causes a panic: "source slice length (4) does not match destination slice length (0)"
                let mut output: Vec<T> = vec![T::default(); index_c - reader_index];
                output.copy_from_slice(&buffer_dereferenced[reader_index..index_c]);
                return (index_c, Some(output));
            }
        }
    }
}

// --------------------------------- V2 ---------------------------------------

/// A Block loosely corresponds to a seqlock + byte array
/// 
/// Note the data array is not really 0 length, this is a hack to make the compiler happy (we're gonna unsafely/manually allocate all this memory anyways...)
struct BufferBlock {
    seq: AtomicU32,
    size: AtomicU32,
    data: [u8; 0]
}

/// `BlockArray` is another hack for alignment.
#[repr(align(64))]
struct BlockArray {
    data: [BufferBlock]
}

/// RingBuffer, Version 2 with each (outer) array item being a byte array "protected" by a seqlock
/// 
/// This header is only used when a reader joins the queue.
/// 
/// Puts less cache contention on headers by spreading out atomic counters -> one per element
struct RingBufferV2 {
    seq: BufferIndex,
    blocks: BlockArray
}

// --------------------------------- TESTS ------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_new() {
        let rb = RingBufferV1::<u8>::new();
        unsafe {
            let first_value: u8 = (*rb.buf.get())[0];
            assert_eq!(first_value, 0);
        }
        assert_eq!(rb.index_committed.value.load(Ordering::Relaxed), 0);
        assert_eq!(rb.index_pending.value.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn v1_writes() {
        let rb = RingBufferV1::<u8>::new();
        unsafe {
            // Check newly initialized buffer
            let (index, value) = rb.read_single(0);
            assert_eq!(index, 0usize);
            assert_eq!(value, None);
            // Now push data
            rb.write(&[1,2,3,4]);
            assert_eq!(rb.index_committed.value.load(Ordering::Relaxed), 4);
            assert_eq!(rb.index_pending.value.load(Ordering::Relaxed), 4);
            let first_block: &[u8] = &(*rb.buf.get())[0..4];
            assert_eq!(first_block, [1,2,3,4]);
            // Now push enough data to force a split write
            // We want to end up with an array with the first two values overwritten [5,5,3,4,5,5,5,5, ...]
            let big_chunk: [u8; BUFFER_SIZE - 3] = [5; BUFFER_SIZE - 3];
            rb.write(&big_chunk);
            let first_block: &[u8] = &(*rb.buf.get())[0..8];
            assert_eq!(first_block, [5,5,3,4,5,5,5,5]);
        }
    }

    #[test]
    fn v1_read_single() {
        let rb = RingBufferV1::<u8>::new();
        // Now push data
        unsafe {
            assert_eq!(rb.read_single(0).1, None);
            rb.write(&[1,2,3,4]);
            assert_eq!(rb.read_single(0).1.unwrap(), 1);
            assert_eq!(rb.read_single(1).1.unwrap(), 2);
            assert_eq!(rb.read_single(2).1.unwrap(), 3);
            assert_eq!(rb.read_single(3).1.unwrap(), 4);
            assert_eq!(rb.read_single(4).1, None);
        }
    }

    #[test]
    fn v1_read_many() {
        let rb = RingBufferV1::<u8>::new();
        // Now push data
        unsafe {
            assert_eq!(rb.read_single(0).1, None);
            // Write then read a single block
            rb.write(&[1,2,3,4]);
            let d = rb.read_unread(0);
            assert_eq!(d.0, 4);
            assert_eq!(d.1.unwrap(), [1,2,3,4]);
            // Write twice then read
            rb.write(&[5,6]);
            rb.write(&[7]);
            let d2 = rb.read_unread(d.0);
            assert_eq!(d2.0, 7);
            assert_eq!(d2.1.unwrap(), [5,6,7]);
            // Write with wrapping
            rb.write(&[8; BUFFER_SIZE-4]);
            let d3 = rb.read_unread(d2.0);
            assert_eq!(d3.0, 3);
            assert_eq!(d3.1.unwrap().len(), BUFFER_SIZE - 4);
        }
    }
}