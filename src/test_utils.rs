use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use crate::{Consumer, Graph};

#[derive(Debug)]
struct TestConsumer {
    id: i32,
    idx: Arc<AtomicUsize>,
}

impl TestConsumer {
    fn new(consumer_id: i32) -> TestConsumer {
        TestConsumer{id: consumer_id, idx: Arc::new(AtomicUsize::new(0))}
    }
}

impl Consumer<i32> for TestConsumer {
    fn consume(&self, data: &[i32]) -> () {
        let ln = data.len();
        self.idx.fetch_add(ln, Ordering::Release);
    }
    fn get_idx(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.idx)
    }
}
unsafe impl Send for TestConsumer {}



pub fn single_prod_multi_cons_run() -> () {
    /*
    Flow
    Register Producer gives R[T], which allows register consumers,
    can have concurrent consumers
    when start:
        each producer run on 1 thread
        each consumer run on 1 thread
    */
    const NO_OF_BATCH: usize = 100_000usize;
    const BATCH_SIZE: usize = 2043;
    const NO_OF_REC: usize = NO_OF_BATCH * BATCH_SIZE;
    let mut g: Graph<i32, TestConsumer> = Graph::new();
    let handler = g.register_producer();

    let consumer0 = TestConsumer::new(0);
    let consumer1 = TestConsumer::new(1);
    handler.register_consumer(&mut g, consumer0);
    handler.register_consumer(&mut g, consumer1);

    // println!("{:#?}", g.consumers);
    // println!("{:#?}", g.consumers_deps);
    // println!("{:#?}", g.consumable_indices);

    let g1 = Arc::new(g);
    let g2 = Arc::clone(&g1);

    let producer = thread::spawn(move || {
        for n in 0..NO_OF_BATCH {
            g1.produce(vec![n as i32; BATCH_SIZE]);
        }
    });
    let handle2 = thread::spawn(move || {
        g2.run(Some(NO_OF_REC));
    });

    producer.join().unwrap();
    handle2.join().unwrap();
}