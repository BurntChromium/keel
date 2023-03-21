mod ringbuffers;

use std::thread;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ringbuffers::RingBufferV1;

fn main() {
    // UTF-8 "hello" as decimal
    let garbage_data: [u8; 5] = [104, 101, 108, 108, 11];
    let rb: Arc<RingBufferV1<u8>> = Arc::new(RingBufferV1::new());

    const ITERS: usize = 1000;
    let mut timings: Vec<u128> = Vec::with_capacity(ITERS);
    let mut reads: Vec<u32> = Vec::with_capacity(ITERS);

    const WRITE_COUNT: u32 = 25000;

    for _ in 0..ITERS {
        let writer = rb.clone();
        let reader = rb.clone();
        let start = Instant::now();

        let thread_writer = thread::spawn(move || {
            for _ in 0..WRITE_COUNT {
                // let data: [u8; 5] = garbage_data.map(|x| x + i);
                let data: [u8; 5] = garbage_data;
                unsafe {
                    writer.write(&data);
                }
            }
        });

        let mut r_index: usize = 0;
        let mut read_count: u32 = 0;
        while r_index < 50 {
            unsafe {
                let (index, _) = reader.read_single(r_index);
                r_index = index;
            }
            read_count += 1;
        }
        thread_writer.join().unwrap();

        let duration = start.elapsed();
        timings.push(duration.as_micros());
        reads.push(read_count);
    }
    
    let mean_time: f64 = timings.iter().sum::<u128>() as f64 / timings.len() as f64;
    let mean_reads = reads.iter().sum::<u32>() as f32 / reads.len() as f32;

    println!("Simulations run: {}", ITERS);
    println!("Mean time elapsed in simultaneous reading/writing is: {:.0} microseconds", mean_time);
    println!("Write count per simulation is {}", WRITE_COUNT);
    println!("Mean read count is {}", mean_reads);
    println!("Implied mean writes/second: {:.0}", ((WRITE_COUNT as f64) * 1000000f64) / mean_time);
    println!("Implied write throughput is {:.0} GB/s", ((WRITE_COUNT as f64) * 1000000f64 * garbage_data.len() as f64) / (1024 * 1024 * 1024) as f64);
    println!("Implied mean reads/second: {:.0}", ((mean_reads as f64) * 1000000f64) / mean_time);
}
