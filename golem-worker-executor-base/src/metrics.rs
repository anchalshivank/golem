// Module holding all the metrics used by the server.
// Collecting them in one place makes it easier to look them up and to share
// common metrics between different layers of the application.

use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref VERSION_INFO: IntCounterVec = register_int_counter_vec!(
        "version_info",
        "Version info of the server",
        &["version", "wasmtime"]
    )
    .unwrap();
}

pub fn register_all() -> Registry {
    VERSION_INFO
        .with_label_values(&[version(), wasmtime_runtime::VERSION])
        .inc();

    default_registry().clone()
}

fn version() -> &'static str {
    option_env!("CARGO_VERSION_INFO").unwrap_or(env!("CARGO_PKG_VERSION"))
}

const FUEL_BUCKETS: &[f64; 11] = &[
    1000.0, 10000.0, 25000.0, 50000.0, 100000.0, 250000.0, 500000.0, 1000000.0, 2500000.0,
    5000000.0, 10000000.0,
];

const MEMORY_SIZE_BUCKETS: &[f64; 11] = &[
    1024.0,
    4096.0,
    16384.0,
    65536.0,
    262144.0,
    1048576.0,
    4194304.0,
    16777216.0,
    67108864.0,
    268435456.0,
    1073741824.0,
];

pub mod component {
    use std::time::Duration;

    use golem_common::metrics::DEFAULT_TIME_BUCKETS;
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        pub static ref COMPILATION_TIME_SECONDS: Histogram = register_histogram!(
            "compilation_time_seconds",
            "Time to compile a WASM component to native code",
            DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_compilation_time(duration: Duration) {
        COMPILATION_TIME_SECONDS.observe(duration.as_secs_f64());
    }
}

pub mod events {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref EVENT_TOTAL: CounterVec = register_counter_vec!(
            "event_total",
            "Number of events produced by the server",
            &["event"]
        )
        .unwrap();
        static ref EVENT_BROADCAST_TOTAL: CounterVec = register_counter_vec!(
            "event_broadcast_total",
            "Number of events broadcast by the server",
            &["event"]
        )
        .unwrap();
    }

    pub fn record_event(event: &'static str) {
        EVENT_TOTAL.with_label_values(&[event]).inc();
    }

    pub fn record_broadcast_event(event: &'static str) {
        EVENT_BROADCAST_TOTAL.with_label_values(&[event]).inc();
    }
}

pub mod grpc {
    use lazy_static::lazy_static;
    use prometheus::*;
    use tracing::{error, info};

    use crate::error::GolemError;

    lazy_static! {
        static ref GRPC_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
            "grpc_success_seconds",
            "Time taken for successfully serving gRPC requests",
            &["api"],
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref GRPC_FAILURE_SECONDS: HistogramVec = register_histogram_vec!(
            "grpc_failure_seconds",
            "Time taken for serving failed gRPC requests",
            &["api", "error"],
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref GRPC_ACTIVE_STREAMS: Gauge =
            register_gauge!("grpc_active_streams", "Number of active gRPC streams").unwrap();
    }

    pub fn record_grpc_success(api_name: &'static str, duration: std::time::Duration) {
        GRPC_SUCCESS_SECONDS
            .with_label_values(&[api_name])
            .observe(duration.as_secs_f64());
    }

    pub fn record_grpc_failure(
        api_name: &'static str,
        error_kind: &'static str,
        duration: std::time::Duration,
    ) {
        GRPC_FAILURE_SECONDS
            .with_label_values(&[api_name, error_kind])
            .observe(duration.as_secs_f64());
    }

    pub fn record_new_grpc_active_stream() {
        GRPC_ACTIVE_STREAMS.inc();
    }

    pub fn record_closed_grpc_active_stream() {
        GRPC_ACTIVE_STREAMS.dec();
    }

    pub struct RecordedGrpcRequest {
        api_name: &'static str,
        start_time: Option<std::time::Instant>,
        details_to_log: String,
    }

    impl RecordedGrpcRequest {
        pub fn new(api_name: &'static str, details_to_log: String) -> Self {
            Self {
                api_name,
                start_time: Some(std::time::Instant::now()),
                details_to_log,
            }
        }

        pub fn succeed<T>(mut self, result: T) -> T {
            match self.start_time.take() {
                Some(start) => {
                    let elapsed = start.elapsed();
                    info!(
                        "{} ({}) succeeded in {}ms",
                        self.api_name,
                        self.details_to_log,
                        elapsed.as_millis()
                    );

                    record_grpc_success(self.api_name, elapsed);
                    result
                }
                None => result,
            }
        }

        pub fn fail<T>(mut self, result: T, error: &GolemError) -> T {
            match self.start_time.take() {
                Some(start) => {
                    let elapsed = start.elapsed();
                    error!(
                        "{} ({}) failed in {}ms with error {:?}",
                        self.api_name,
                        self.details_to_log,
                        elapsed.as_millis(),
                        error
                    );

                    record_grpc_failure(self.api_name, error.kind(), elapsed);
                    result
                }
                None => result,
            }
        }
    }

    impl Drop for RecordedGrpcRequest {
        fn drop(&mut self) {
            if let Some(start) = self.start_time.take() {
                record_grpc_failure(self.api_name, "Drop", start.elapsed());
            }
        }
    }
}

pub mod workers {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref INSTANCE_SVC_CALL_TOTAL: CounterVec = register_counter_vec!(
            "instance_svc_call_total",
            "Number of calls to the worker management service",
            &["api"]
        )
        .unwrap();
    }

    pub fn record_worker_call(api_name: &'static str) {
        INSTANCE_SVC_CALL_TOTAL.with_label_values(&[api_name]).inc();
    }
}

pub mod invocation_keys {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref INVOCATION_KEYS_PENDING_COUNT: Gauge = register_gauge!(
            "invocation_keys_pending_count",
            "Number of pending invocation keys"
        )
        .unwrap();
        static ref INVOCATION_KEYS_CONFIRMED_COUNT: Gauge = register_gauge!(
            "invocation_keys_confirmed_count",
            "Number of confirmed invocation keys"
        )
        .unwrap();
    }

    pub fn record_pending_invocation_keys_count(count: usize) {
        INVOCATION_KEYS_PENDING_COUNT.set(count as f64);
    }

    pub fn record_confirmed_invocation_keys_count(count: usize) {
        INVOCATION_KEYS_CONFIRMED_COUNT.set(count as f64);
    }
}

pub mod promises {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref PROMISES_COUNT_TOTAL: Counter =
            register_counter!("promises_count_total", "Number of promises created").unwrap();
        static ref PROMISES_SCHEDULED_COMPLETE_TOTAL: Counter = register_counter!(
            "promises_scheduled_complete_total",
            "Number of scheduled promise completions"
        )
        .unwrap();
    }

    pub fn record_promise_created() {
        PROMISES_COUNT_TOTAL.inc();
    }

    pub fn record_scheduled_promise_completed() {
        PROMISES_SCHEDULED_COMPLETE_TOTAL.inc();
    }
}

pub mod sharding {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref ASSIGNED_SHARD_COUNT: Gauge =
            register_gauge!("assigned_shard_count", "Current number of assigned shards").unwrap();
    }

    pub fn record_assigned_shard_count(size: usize) {
        ASSIGNED_SHARD_COUNT.set(size as f64);
    }
}

pub mod wasm {
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;
    use tracing::debug;

    use crate::error::GolemError;

    lazy_static! {
        static ref CREATE_WORKER_SECONDS: Histogram = register_histogram!(
            "create_instance_seconds",
            "Time taken to create a worker",
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref CREATE_WORKER_FAILURE_TOTAL: CounterVec = register_counter_vec!(
            "create_instance_failure_total",
            "Number of failed worker creations",
            &["error"]
        )
        .unwrap();
        static ref INVOCATION_TOTAL: CounterVec = register_counter_vec!(
            "invocation_total",
            "Number of invocations",
            &["mode", "outcome"]
        )
        .unwrap();
        static ref INVOCATION_CONSUMPTION_TOTAL: Histogram = register_histogram!(
            "invocation_consumption_total",
            "Amount of fuel consumed by an invocation",
            crate::metrics::FUEL_BUCKETS.to_vec()
        )
        .unwrap();
        static ref ALLOCATED_MEMORY_BYTES: Histogram = register_histogram!(
            "allocated_memory_bytes",
            "Amount of memory allocated by a single memory.grow instruction",
            crate::metrics::MEMORY_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
    }


    lazy_static! {
        static ref HOST_FUNCTION_CALL_TOTAL: CounterVec = register_counter_vec!(
            "host_function_call_total",
            "Number of calls to specific host functions",
            &["interface", "name"]
        )
        .unwrap();
        static ref RESUME_WORKER_SECONDS: Histogram = register_histogram!(
            "resume_instance_seconds",
            "Time taken to resume a worker",
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref REPLAYED_FUNCTIONS_COUNT: Histogram = register_histogram!(
            "replayed_functions_count",
            "Number of functions replayed per worker resumption",
            golem_common::metrics::DEFAULT_COUNT_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_host_function_call(iface: &'static str, name: &'static str) {
        debug!("golem {iface}::{name} called");
        HOST_FUNCTION_CALL_TOTAL
            .with_label_values(&[iface, name])
            .inc();
    }

    pub fn record_resume_worker(duration: Duration) {
        RESUME_WORKER_SECONDS.observe(duration.as_secs_f64());
    }

    pub fn record_number_of_replayed_functions(count: usize) {
        REPLAYED_FUNCTIONS_COUNT.observe(count as f64);
    }

    pub fn record_create_worker(duration: Duration) {
        CREATE_WORKER_SECONDS.observe(duration.as_secs_f64());
    }

    pub fn record_create_worker_failure(error: &GolemError) {
        CREATE_WORKER_FAILURE_TOTAL
            .with_label_values(&[error.kind()])
            .inc();
    }

    pub fn record_invocation(is_live: bool, outcome: &'static str) {
        let mode: &'static str = if is_live { "live" } else { "replay" };
        INVOCATION_TOTAL.with_label_values(&[mode, outcome]).inc();
    }

    pub fn record_invocation_consumption(fuel: i64) {
        INVOCATION_CONSUMPTION_TOTAL.observe(fuel as f64);
    }

    pub fn record_allocated_memory(amount: usize) {
        ALLOCATED_MEMORY_BYTES.observe(amount as f64);
    }
}

pub mod oplog {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref OPLOG_SVC_CALL_TOTAL: CounterVec = register_counter_vec!(
            "oplog_svc_call_total",
            "Number of calls to the oplog service",
            &["api"]
        )
        .unwrap();
    }

    pub fn record_oplog_call(api_name: &'static str) {
        OPLOG_SVC_CALL_TOTAL.with_label_values(&[api_name]).inc();
    }
}

