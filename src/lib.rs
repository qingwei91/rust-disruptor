#![feature(sync_unsafe_cell)]

use std::cell::SyncUnsafeCell;
use std::cmp::min;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use env_logger;
use log;
pub mod test_utils;

const RING_SIZE: usize = 1024;

pub struct Graph<T: Copy, CS: Consumer<T> + Send> {
    // all indices
    consumable_indices: Vec<AtomicUsize>,
    // we can use another generic with trait bound to model consumer + transformer, not sure if better
    consumers: Vec<SyncUnsafeCell<Box<CS>>>,
    // these 2 vecs represent the dependency of consumer/transformer respectively
    // eg consumer_deps = vec![0,2] means 1st consumer depends on index on 0, 2nd consumer depends on index 2
    // the general logic
    consumer_idx_upstream: Vec<(usize, usize)>,
    inner: Arc<SyncUnsafeCell<[MaybeUninit<T>; RING_SIZE]>>,
}

#[derive(Copy, Clone)]
pub enum BackoffStrategy {
    Spin,
    FixedSleep(Duration),
    SpinThenSleep(u8, Duration),
}

impl<T: Copy + Send + Sync, CS: Consumer<T> + Send + Sync> Graph<T, CS> {
    pub fn new() -> Graph<T, CS> {
        let data: [MaybeUninit<T>; RING_SIZE] = [const { MaybeUninit::uninit() }; RING_SIZE];
        let inner = SyncUnsafeCell::new(data);
        Graph {
            consumable_indices: vec![],
            consumers: vec![],
            consumer_idx_upstream: vec![],
            inner: Arc::new(inner),
        }
    }
    pub fn register_producer(&mut self) -> RegistrationHandle<T> {
        self.consumable_indices.push(AtomicUsize::new(0));
        RegistrationHandle {
            upstream_index: 0,
            d: PhantomData {},
        }
    }

    fn produce(&self, data: &Vec<T>) -> () {
        /*
        We can assume single producer, enforced by exclusion ref

        caller might give us a batch larger than we can process, we might have to wrap the buffer in
        write, and be careful not to lap consumer on each write
        we should break input into chunks

        this will try to write data in a loop:
        1. Find out available space to write
            - cannot overlap any consumer
            - cannot lap the buffer array
        1. Determine the slice to write, starting from 0..n where n is the largest index that's empty
        1. slice progress by n on each loop, break when start = n
        */

        // by convention, 1st index is producer's
        let mut producer_idx = self.consumable_indices.get(0).unwrap().load(Ordering::Acquire);
        // no of element written, cannot exceed data.len()
        let mut data_written = 0;
        loop {
            // todo: bug, if data is len 1 this does not work
            if data_written >= data.len() {
                log::debug!("Finished written {:?} data", data_written);
                break;
            } else {
                let mut consumer_low_watermark = *&self.consumable_indices[1].load(Ordering::Acquire);
                for ci in &self.consumable_indices[2..] {
                    let i = ci.load(Ordering::Acquire);
                    if consumer_low_watermark > i {
                        consumer_low_watermark = i;
                    }
                }

                // every write can not write beyond the buffer, we can wrap but that should be broken into
                // separate write
                let space_until_end_of_buffer = RING_SIZE - (producer_idx % RING_SIZE);

                let space_until_low_watermark = consumer_low_watermark + RING_SIZE - producer_idx;

                let available_write_size = min(space_until_end_of_buffer, space_until_low_watermark);

                let write_range_end = min(data.len(), data_written + available_write_size);
                self.write(producer_idx % RING_SIZE, &data[data_written..write_range_end]);
                log::debug!("Written {:?}", write_range_end);
                let written_slice_size = write_range_end - data_written;

                self.consumable_indices
                    .get(0)
                    .unwrap()
                    .fetch_add(written_slice_size, Ordering::Release);

                //todo: does avoiding Atomic load make it faster?
                producer_idx = producer_idx + written_slice_size;
                data_written += written_slice_size;
                // thread::sleep(Duration::from_millis(1))
            }
        }
    }
    fn write(&self, from_idx: usize, data: &[T]) -> () {
        // caller ensure the range (ie. from_idx .. from_idx + data.len()) is mutually exclusive
        // with other access
        let ptr = self.inner.get();
        unsafe {
            let inner_ptr = (*ptr).as_mut_ptr();
            let dest_ptr = inner_ptr.add(from_idx) as *mut T;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dest_ptr, data.len())
        }
    }

    fn read_data(&self, from_idx: usize, read_ln: usize) -> &[T] {
        let ptr = self.inner.get();
        unsafe {
            let slice = (*ptr).as_ptr().add(from_idx) as *const T;
            std::slice::from_raw_parts(slice, read_ln)
        }
    }

    // making a choice of static graph, once you run nothing can be done
    fn run_with_backoff(&self, stop_after_consume: Option<usize>, backoff_strategy: BackoffStrategy) -> () {
        thread::scope(|s| {
            /*
                For each consumer, we need
                1. check if upstream idx is higher than my idx
                2. if so, take the extra slice and consume
                3. else sleep
            */
            for i in 0..self.consumers.len() {
                let (c_idx, c_ups_idx) = self.consumer_idx_upstream.get(i).unwrap();
                let upstream = self.consumable_indices.get(*c_ups_idx).unwrap();
                let consumer: &mut CS = unsafe { &mut *self.consumers.get(i).unwrap().get() };
                // let con_idx = self.consumer_indices[i];
                s.spawn(move || {
                    let mut backoff_count = 0;
                    loop {
                        let consumer_idx = &self.consumable_indices[*c_idx].load(Ordering::Acquire);
                        if let Some(i) = stop_after_consume {
                            if *consumer_idx >= i {
                                break;
                            }
                        }
                        let upstream_written = upstream.load(Ordering::Acquire);
                        if *consumer_idx < upstream_written {
                            let starting: usize = consumer_idx % RING_SIZE;
                            let end: usize = upstream_written % RING_SIZE;

                            if end <= starting {
                                let slic = self.read_data(starting, RING_SIZE - starting);
                                consumer.consume(slic);
                                let mut consumed = slic.len();
                                if end != 0 {
                                    let slic = self.read_data(0, end);
                                    consumer.consume(slic);
                                    consumed += slic.len();
                                }
                                self.consumable_indices[*c_idx].fetch_add(consumed, Ordering::Release);
                            } else {
                                let slic = self.read_data(starting, end - starting);
                                consumer.consume(slic);
                                self.consumable_indices[*c_idx].fetch_add(slic.len(), Ordering::Release);
                            }

                            log::debug!("{:?} read until {:?}", i, upstream_written);
                            backoff_count = 0;
                        } else {
                            match backoff_strategy {
                                BackoffStrategy::Spin => {}
                                BackoffStrategy::FixedSleep(sleep_dur) => {
                                    thread::sleep(sleep_dur);
                                }
                                BackoffStrategy::SpinThenSleep(count, sleep_dur) => {
                                    if count > backoff_count {
                                        backoff_count += 1;
                                    } else {
                                        thread::sleep(sleep_dur);
                                    }
                                }
                            }
                        }
                    }
                });
            }
        })
    }

    fn run(&self, stop_after_consume: Option<usize>) -> () {
        self.run_with_backoff(stop_after_consume, BackoffStrategy::Spin)
    }
}

pub struct RegistrationHandle<T> {
    upstream_index: usize,
    d: PhantomData<T>,
}

impl<T: Copy> RegistrationHandle<T> {
    pub fn register_consumer<CS: Consumer<T> + Send>(&self, graph: &mut Graph<T, CS>, consumer: CS) -> RegistrationHandle<T> {
        graph.consumable_indices.push(AtomicUsize::new(0));
        graph.consumers.push(SyncUnsafeCell::new(Box::new(consumer)));
        graph.consumer_idx_upstream.push((graph.consumable_indices.len() - 1, self.upstream_index));
        RegistrationHandle {
            upstream_index: graph.consumable_indices.len() - 1,
            d: PhantomData,
        }
    }
}

pub trait Consumer<T> {
    // we are giving a slice, meaning actual consume might need to dereference and cause copy
    // remained to be seen if its okay
    // the contract also expect consume method to advance the idx
    fn consume(&mut self, data: &[T]) -> ();
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};
    use std::{thread, time};
    use std::collections::HashMap;
    use test_utils::*;

    use super::*;

    #[test]
    pub fn single_prod_multi_cons() -> () {
        env_logger::init();
        single_prod_multi_cons_run(1, 1);
        single_prod_multi_cons_run(100, 100);
    }

    #[test]
    pub fn single_prod_multi_cons_with_deps() -> () {
        env_logger::init();
        let a = setup_consumer_deps(1,1);
        test_produce_consume(Arc::new(a.0), &a.1, a.2);

        let a = setup_consumer_deps(100,100);
        test_produce_consume(Arc::new(a.0), &a.1, a.2);

        let log = a.3.lock().unwrap();
        let mut exp = HashMap::new();
        exp.insert(0, 0);
        exp.insert(1, 0);
        exp.insert(2, 0);
        println!("{:?}", log);
        for (cid, idx) in log.iter() {

            if *cid == 1 {
                assert!(*idx <= *exp.get(&0).unwrap(), "cid 1 idx {} exp {}", idx, exp.get(&0).unwrap())
            }
            if *cid == 2 {
                assert!(*idx <= *exp.get(&1).unwrap(), "cid 2 idx {} exp {}", idx, exp.get(&1).unwrap())
            }
            exp.insert(*cid, *idx);
        }
    }


    struct TestStatic {
        i: &'static str,
    }
    impl Drop for TestStatic {
        fn drop(&mut self) {
            println!("{}", self.i);
            println!("Dropped");
        }
    }

    #[test]
    fn test_static_lifetime() -> () {
        {
            TestStatic { i: "Inner dropped 1st" };
        }
        let _ = TestStatic { i: "out dropped last" };
        println!("In bettweeen");
    }

    #[test]
    fn test_arc_timing() -> () {
        let data = vec![1, 2, 3];
        let d1 = Arc::new(Mutex::new(data));
        let d2 = Arc::clone(&d1);
        let t1 = thread::spawn(move || {
            thread::sleep(time::Duration::from_millis(50));
            d1.lock().unwrap().push(4);
        });

        let t2 = thread::spawn(move || {
            loop {
                // scope mutex so that it get drop and not block
                {
                    let dtt = d2.lock().unwrap();
                    println!("Observed new data {:?}", dtt.len());
                    if dtt.len() >= 4 {
                        break;
                    }
                }
                thread::sleep(time::Duration::from_millis(10));
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();
    }
}
