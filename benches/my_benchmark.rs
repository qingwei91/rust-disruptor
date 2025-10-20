use std::hint::black_box;
use criterion::{criterion_group, criterion_main, Criterion};
use rust_disruptor::test_utils::single_prod_multi_cons_run;


fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("single producer 2 consumers", |b| b.iter(|| single_prod_multi_cons_run()));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);