use std::hint::black_box;
use std::sync::Arc;
use criterion::{criterion_group, criterion_main, Criterion};
use rust_disruptor::Graph;
use rust_disruptor::test_utils::{setup_data, test_produce_consume, TestConsumer};


fn criterion_benchmark(c: &mut Criterion) {
    let (_, test_data, total_n) = setup_data();

    c.bench_function("single producer 3 consumers", |b| {
        b.iter (|| {
            let mut g: Graph<i32, TestConsumer> = Graph::new();
            let handler = g.register_producer();

            let consumer0 = TestConsumer::new(0);
            let consumer1 = TestConsumer::new(1);
            let consumer2 = TestConsumer::new(2);
            handler.register_consumer(&mut g, consumer0);
            handler.register_consumer(&mut g, consumer1);
            handler.register_consumer(&mut g, consumer2);

            black_box(test_produce_consume(Arc::new(g), &test_data, total_n))
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);