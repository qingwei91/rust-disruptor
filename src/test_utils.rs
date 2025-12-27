use crate::{Consumer, Graph};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[derive(Debug)]
pub struct TestConsumer {
    id: i32,
    idx: Arc<AtomicUsize>,
}

impl TestConsumer {
    pub fn new(consumer_id: i32) -> TestConsumer {
        TestConsumer {
            id: consumer_id,
            idx: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Consumer<i32> for TestConsumer {
    fn consume(&mut self, data: &[i32]) -> () {
        let ln = data.len();
        self.idx.fetch_add(ln, Ordering::Release);
    }
}
unsafe impl Send for TestConsumer {}

pub fn setup_data(
    no_of_batch: usize,
    batch_size: usize,
) -> (Graph<i32, TestConsumer>, Vec<Vec<i32>>, usize) {
    let no_of_rec: usize = no_of_batch * batch_size;
    let mut g: Graph<i32, TestConsumer> = Graph::new();
    let handler = g.register_producer();

    let consumer0 = TestConsumer::new(0);
    let consumer1 = TestConsumer::new(1);
    let consumer2 = TestConsumer::new(2);
    handler.register_consumer(&mut g, consumer0);
    handler.register_consumer(&mut g, consumer1);
    handler.register_consumer(&mut g, consumer2);

    let mut test_data = Vec::with_capacity(no_of_batch);
    for n in 0..no_of_batch {
        test_data.push(vec![n as i32; batch_size]);
    }

    (g, test_data, no_of_rec)
}

pub fn setup_consumer_deps(
    no_of_batch: usize,
    batch_size: usize,
) -> (Graph<i32, TestConsumer>, Vec<Vec<i32>>, usize) {
    let no_of_rec: usize = no_of_batch * batch_size;
    let mut g: Graph<i32, TestConsumer> = Graph::new();
    let handler = g.register_producer();

    let consumer0 = TestConsumer::new(0);
    let consumer1 = TestConsumer::new(1);
    let consumer2 = TestConsumer::new(2);
    handler.register_consumer(&mut g, consumer0);
    handler.register_consumer(&mut g, consumer1).register_consumer(&mut g, consumer2);

    let mut test_data = Vec::with_capacity(no_of_batch);
    for n in 0..no_of_batch {
        test_data.push(vec![n as i32; batch_size]);
    }

    return (g, test_data, no_of_rec);
}


pub fn single_prod_multi_cons_run(no_of_batch: usize, batch_size: usize) -> () {
    /*
    Flow
    Register Producer gives R[T], which allows register consumers,
    can have concurrent consumers
    when start:
        each producer run on 1 thread
        each consumer run on 1 thread
    */
    let a = setup_data(no_of_batch, batch_size);
    test_produce_consume(Arc::new(a.0), &a.1, a.2);
}

pub fn test_produce_consume(
    graph: Arc<Graph<i32, TestConsumer>>,
    test_data: &Vec<Vec<i32>>,
    total_n: usize,
) -> () {
    let g2 = Arc::clone(&graph);

    thread::scope(|s| {
        let producer = s.spawn(|| {
            for vec in test_data {
                graph.produce(vec);
            }
        });
        let handle2 = s.spawn(move || {
            g2.run(Some(total_n));
        });

        producer.join().unwrap();
        handle2.join().unwrap();
    })
}
