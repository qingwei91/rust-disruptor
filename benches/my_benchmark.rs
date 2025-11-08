use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_disruptor::test_utils::{setup_data, test_produce_consume, TestConsumer};
use rust_disruptor::Graph;
use std::hint::black_box;
use std::sync::Arc;

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("broadcast 3 consumers");
    let batch_sizes = vec![1, 16, 1024, 2048, 5155];
    let test_sizes = batch_sizes.into_iter().map(|b| {
        let n = (1_000_000 / b);
        (n, b)
    });

    for (no_of_b, batch_size) in test_sizes {
        let (_, test_data, total_n) = setup_data(no_of_b, batch_size);
        group.bench_with_input(BenchmarkId::new("Broadcast", format!("{:?}", (no_of_b, batch_size))), &test_data, |b, td| {
            b.iter(|| {
                let mut g: Graph<i32, TestConsumer> = Graph::new();
                let handler = g.register_producer();

                let consumer0 = TestConsumer::new(0);
                let consumer1 = TestConsumer::new(1);
                let consumer2 = TestConsumer::new(2);
                handler.register_consumer(&mut g, consumer0);
                handler.register_consumer(&mut g, consumer1);
                handler.register_consumer(&mut g, consumer2);

                black_box(test_produce_consume(Arc::new(g), td, total_n))
            })
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
