# Overview

This is a practice project for me to learn Concurrency in Rust.

It mimics a small subset of Disruptor project in Java.

It is useful for in-memory high throughput pub sub pattern. 

# Technical details

## Graph<T, Consumer<T>> type

This is the core data structure that contain the state of your queue (the api is not exactly Queue-like). 

It contains:
1. A ring buffer holding messages/data
2. Track indices of producer and consumer
3. Dependency tracking between producer and consumers

It expresses producer consumer relationship as a static dag, meaning user need to register consumer up front
before producer starts.

At the moment it only supports 1 producer.

## Design notes

Disruptor is fast for many reasons, I think the most important ones are:

1. Cache friendly
2. Avoid contention

I did something similar here:

* The core ring buffer is an array, it is cache friendly
* Avoid contention by allowing writer and reader to simultaneously access the array as long as we can guarantee they are accessing difference indices

To maximize throughput, I had to use unsafe code to allow mutation without holding exclusive reference. 
Then rely on Atomics to sync operations across threads for correctness.

Disruptor also do something like padding to avoid false sharing, I've yet figure out how, I think this feature is implement in cross-beam.

I've modeled Consumer as a trait, as a result they are heap allocated. In my test it has very little effect.

## Performance

I havent done any in-depth analysis, but on my laptop, using criterion I think on average the current implementation 
can pushes 10000 batches of 2000 elements and consume them all in about 90ms.

Caveat is the current interface act on batch of data, ie. &Vec<T>, which is more efficient when copying in and out array 
but might not work as well for single element access pattern. 

# Next:

1. Implement bench with different variations on size of message, no of message
1. Implement chaining of processes
2. Implement padding to avoid false sharing