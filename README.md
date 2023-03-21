# Keel: Low Level Concurrency Experiments

This repository holds experiments with implementing very performance concurrent data structures (lock free queues and similar) in Rust. This repository is currently unstructured, that structure will grow over time. 

References for design are the following [Java project](https://lmax-exchange.github.io/disruptor/disruptor.html#_design_of_the_lmax_disruptor) and this [C++ based talk](https://www.youtube.com/watch?v=8uAW5FQtcvE).

### Benchmarks

Compiling the code as given will run an unscientific benchmark that writes (currently) 5 bytes (the `u8` encoding of `hello`) in a loop to a ring buffer, then reads each value individually. No attempt is made to control for writers overrunning readers, core-pinning, or anything else that would be required for "production" software (yet).

As an example of a run on my laptop for RingBufferV1 (with `cargo run --release`):
```
Simulations run: 1000
Mean time elapsed in simultaneous reading/writing is: 378 microseconds
Write count per simulation is 25000
Mean read count is 5377.772
Implied mean writes/second: 66060670
Implied write throughput is 116 GB/s
Implied mean reads/second: 14210369
```