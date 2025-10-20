#![feature(sync_unsafe_cell)]
#![feature(maybe_uninit_uninit_array)]

pub mod test_utils;

use std::cell::SyncUnsafeCell;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const RING_SIZE: usize = 1024;

pub struct Graph<T: Copy, CS: Consumer<T> + Send> {
    consumable_indices: Vec<AtomicUsize>,
    // we need consumer's indices shared because producer need to check for bounds
    consumer_indices: Vec<Arc<AtomicUsize>>,
    // we can use another generic with trait bound to model consumer + transformer, not sure if better
    consumers: Vec<Box<CS>>,
    transformers: Vec<Box<dyn Transformer<T> + Send + Sync>>,
    // these 2 vecs represent the dependency of consumer/transformer respectively
    // eg consumer_deps = vec![0,2] means 1st consumer depends on index on 0, 2nd consumer depends on index 2
    // the general logic
    consumers_deps: Vec<usize>,
    transformers_deps: Vec<usize>,
    inner: Arc<SyncUnsafeCell<[MaybeUninit<T>; RING_SIZE]>>,
}

impl<T: Copy + Send + Sync, CS: Consumer<T> + Send + Sync> Graph<T, CS> {
    fn new() -> Graph<T, CS> {
        let data: [MaybeUninit<T>; RING_SIZE] = MaybeUninit::uninit_array();
        let inner = SyncUnsafeCell::new(data);
        Graph {
            consumable_indices: vec![],
            consumer_indices: vec![],
            consumers: vec![],
            transformers: vec![],
            consumers_deps: vec![],
            transformers_deps: vec![],
            inner: Arc::new(inner),
        }
    }
    fn register_producer(&mut self) -> RegistrationHandle<T> {
        self.consumable_indices.push(AtomicUsize::new(0));
        RegistrationHandle {
            upstream_index: 0,
            d: PhantomData {},
        }
    }

    fn produce(&self, data: Vec<T>) -> () {
        /*
        We can assume single producer, enforced by exclusion ref
        1. Ensure we have enough space to write, valid if no other indices smaller than prod idx + data size - DATA_SIZE
        2. Write
        3. Advance index, this is safe as there's no other producer for now
        */
        // by convention, 1st index is producer's
        let producer_idx = self.consumable_indices.get(0);

        let mut cur_idx = producer_idx.unwrap().load(Ordering::Acquire);

        let new_idx = cur_idx + data.len();
        loop {
            let has_conflict = self.consumer_indices
                .iter()
                .any(|idx| new_idx - idx.load(Ordering::Acquire) >= RING_SIZE);
            if !has_conflict {
                // todo: bug when write wrap
                // given N data
                // given remaining slot = RING_SIZE - cur_idx
                // write slot data, update N = N - slot
                // we also need to consider about writing without lapping consumer

                let data_size = data.len();
                let start_idx = cur_idx % RING_SIZE;
                self.write(start_idx, data);
                self.consumable_indices
                    .get(0)
                    .unwrap()
                    .fetch_add(data_size, Ordering::Release);
                break;
            } else {
                // todo: backoff strategy
                // thread::sleep(Duration::from_millis(500));
            }
        }
    }
    fn write(&self, from_idx: usize, data: Vec<T>) -> () {
        // caller ensure the range (ie. from_idx .. from_idx + data.len()) is mutually exclusive
        // with other access
        let ptr = self.inner.get();
        unsafe {
            let inner_ptr = (*ptr).as_mut_ptr();
            let dest_ptr = inner_ptr.add(from_idx) as *mut T;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dest_ptr, data.len())
        }
    }

    fn read_data(&self, from_idx: usize, to_idx: usize) -> &[T] {
        let ptr = self.inner.get();
        unsafe {
            let slice = (*ptr).as_ptr().add(from_idx) as *const T;
            std::slice::from_raw_parts(slice, to_idx - from_idx)
        }
    }

    // making a choice of static graph, once you run nothing can be done
    fn run(&self, stop_after_consume: Option<usize>) -> () {
        thread::scope(|s| {
            /*
                For each consumer, we need
                1. check if upstream idx is higher than my idx
                2. if so, take the extra slice and consume
                3. else sleep
            */
            for i in 0..self.consumers.len() {
                let consumer_deps_idx: &usize = self.consumers_deps.get(i).unwrap();
                let upstream = self.consumable_indices.get(*consumer_deps_idx).unwrap();
                let consumer = self.consumers.get(i).unwrap();
                // let con_idx = self.consumer_indices[i];
                s.spawn(move || {
                    loop {
                        let consumer_idx = &self.consumer_indices[i].load(Ordering::Acquire);
                        if let Some(i) = stop_after_consume {
                            if *consumer_idx >= i {
                                break;
                            }
                        }
                        let upstream_load = upstream.load(Ordering::Acquire);
                        if *consumer_idx < upstream_load {
                            let starting: usize = consumer_idx % RING_SIZE;
                            let end: usize = upstream_load % RING_SIZE;

                            if end < starting {
                                let slic = self.read_data(starting, RING_SIZE - 1);
                                consumer.consume(slic);
                                let slic = self.read_data(0, end);
                                consumer.consume(slic);
                            } else {
                                let slic = self.read_data(starting, end);
                                consumer.consume(slic);
                            }
                        }
                        // todo: backoff strategy
                        // thread::sleep(Duration::from_millis(10));
                    }
                });
            }
        })
    }
}

pub struct RegistrationHandle<T> {
    upstream_index: usize,
    d: PhantomData<T>,
}

impl<T: Copy> RegistrationHandle<T> {
    fn register_consumer<CS: Consumer<T> + Send>(
        &self,
        graph: &mut Graph<T, CS>,
        consumer: CS,
    ) -> () {
        graph.consumer_indices.push(consumer.get_idx());
        graph.consumers.push(Box::new(consumer));
        graph.consumers_deps.push(self.upstream_index);
    }
    fn register_transformer<CS: Consumer<T> + Send>(
        &self,
        graph: &mut Graph<T, CS>,
        transformer: impl Transformer<T> + 'static + Sync + Send,
    ) -> Self {
        graph.transformers.push(Box::new(transformer));
        graph.transformers_deps.push(self.upstream_index);
        RegistrationHandle {
            upstream_index: graph.transformers.len() - 1,
            d: PhantomData {},
        }
    }
}

pub trait Consumer<T> {
    // we are giving a slice, meaning actual consume might need to dereference and cause copy
    // remained to be seen if its okay
    // the contract also expect consume method to advance the idx
    fn consume(&self, data: &[T]) -> ();
    fn get_idx(&self) -> Arc<AtomicUsize>;
}

pub trait Transformer<T> {
    fn transform(&self, data: &[T]) -> Vec<T>;
}

#[cfg(test)]
pub mod tests {
    use std::{thread, time};
    use std::sync::{Arc, Mutex};
    use test_utils::*;
    use super::*;
    #[test]
    pub fn single_prod_multi_cons() -> () {
        single_prod_multi_cons_run()
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
            TestStatic {
                i: "Inner dropped 1st",
            };
        }
        let _ = TestStatic {
            i: "out dropped last",
        };
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
