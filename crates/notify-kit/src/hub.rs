use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::join_all;

use crate::event::Event;
use crate::sinks::Sink;

const DEFAULT_MAX_INFLIGHT_EVENTS: usize = 128;
const DEFAULT_MAX_SINK_SENDS_IN_PARALLEL: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryNotifyError {
    NoTokioRuntime,
    Overloaded,
}

impl std::fmt::Display for TryNotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTokioRuntime => write!(f, "no tokio runtime"),
            Self::Overloaded => write!(f, "hub is overloaded"),
        }
    }
}

impl std::error::Error for TryNotifyError {}

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Optional allow-list for event kinds.
    ///
    /// - `None`: allow all event kinds.
    /// - `Some(set)`: only allow event kinds present in the set.
    pub enabled_kinds: Option<BTreeSet<String>>,
    /// Per-sink timeout to ensure notifications never block the caller.
    ///
    /// This is a **hard upper bound** enforced by `Hub` (via `tokio::time::timeout`) around each
    /// `Sink::send`. If a sink has its own internal timeout (e.g. an HTTP request timeout), keep
    /// `per_sink_timeout` >= that value (and ideally leave some slack for preflight work like DNS
    /// checks), otherwise `Hub` may time out first.
    pub per_sink_timeout: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            enabled_kinds: None,
            per_sink_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
}

struct HubInner {
    enabled_kinds: Option<BTreeSet<String>>,
    sinks: Vec<Arc<dyn Sink>>,
    per_sink_timeout: Duration,
    inflight: Arc<tokio::sync::Semaphore>,
    max_sink_sends_in_parallel: usize,
}

impl Hub {
    pub fn new(config: HubConfig, sinks: Vec<Arc<dyn Sink>>) -> Self {
        Self::new_with_inflight_limit(config, sinks, DEFAULT_MAX_INFLIGHT_EVENTS)
    }

    pub fn new_with_inflight_limit(
        config: HubConfig,
        sinks: Vec<Arc<dyn Sink>>,
        max_inflight_events: usize,
    ) -> Self {
        let max_inflight_events = max_inflight_events.max(1);
        let inner = HubInner {
            enabled_kinds: config.enabled_kinds,
            sinks,
            per_sink_timeout: config.per_sink_timeout,
            inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight_events)),
            max_sink_sends_in_parallel: DEFAULT_MAX_SINK_SENDS_IN_PARALLEL,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Fire-and-forget notification.
    ///
    /// - Requires a Tokio runtime; if none is present, the notification is dropped and a warning is
    ///   logged.
    /// - Concurrency is bounded; if overloaded, notifications are dropped (with a warning).
    pub fn notify(&self, event: Event) {
        if !self.is_kind_enabled(event.kind.as_str()) {
            return;
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                sink = "hub",
                kind = %event.kind,
                "notify dropped: no tokio runtime"
            );
            return;
        };

        let kind = event.kind.clone();
        if !self.try_notify_spawn(handle, event) {
            tracing::warn!(sink = "hub", kind = %kind, "notify dropped: overloaded");
        }
    }

    /// Attempt to enqueue a fire-and-forget notification.
    ///
    /// Returns:
    /// - `Err(TryNotifyError::NoTokioRuntime)` if called outside a Tokio runtime.
    /// - `Err(TryNotifyError::Overloaded)` when Hub inflight capacity is full.
    pub fn try_notify(&self, event: Event) -> Result<(), TryNotifyError> {
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return Err(TryNotifyError::NoTokioRuntime);
        };

        if self.try_notify_spawn(handle, event) {
            Ok(())
        } else {
            Err(TryNotifyError::Overloaded)
        }
    }

    pub async fn send(&self, event: Event) -> crate::Result<()> {
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow::Error::from(TryNotifyError::NoTokioRuntime))?;
        let _permit = self
            .inner
            .inflight
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("hub inflight semaphore closed"))?;
        self.inner.clone().send(Arc::new(event)).await
    }

    fn is_kind_enabled(&self, kind: &str) -> bool {
        let Some(enabled) = &self.inner.enabled_kinds else {
            return true;
        };
        enabled.contains(kind)
    }

    fn try_notify_spawn(&self, handle: tokio::runtime::Handle, event: Event) -> bool {
        let inner = self.inner.clone();

        let permit = match inner.inflight.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return false,
        };

        handle.spawn(async move {
            let _permit = permit;
            let event = Arc::new(event);
            if let Err(err) = inner.send(event.clone()).await {
                tracing::warn!(sink = "hub", kind = %event.kind, "notify failed: {err}");
            }
        });
        true
    }
}

impl HubInner {
    async fn send(self: Arc<Self>, event: Arc<Event>) -> crate::Result<()> {
        let mut failures: Vec<(&'static str, crate::Error)> = Vec::new();
        let max_parallel = self.max_sink_sends_in_parallel.max(1);
        for sinks_chunk in self.sinks.chunks(max_parallel) {
            let mut futures = Vec::with_capacity(sinks_chunk.len());
            for sink in sinks_chunk {
                let sink = Arc::clone(sink);
                let event = event.clone();
                let timeout = self.per_sink_timeout;
                let name = sink.name();
                futures.push(async move {
                    let result = AssertUnwindSafe(async move {
                        match tokio::time::timeout(timeout, sink.send(&event)).await {
                            Ok(inner) => inner,
                            Err(_) => Err(anyhow::anyhow!("timeout after {timeout:?}").into()),
                        }
                    })
                    .catch_unwind()
                    .await;

                    let result = match result {
                        Ok(inner) => inner,
                        Err(_) => Err(anyhow::anyhow!("sink panicked").into()),
                    };
                    (name, result)
                });
            }

            for (name, result) in join_all(futures).await {
                match result {
                    Ok(()) => {}
                    Err(err) => failures.push((name, err)),
                }
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        let mut msg = String::from("one or more sinks failed:");
        for (name, err) in failures {
            msg.push('\n');
            msg.push_str("- ");
            msg.push_str(name);
            msg.push_str(": ");
            if write!(&mut msg, "{err:#}").is_err() {
                return Err(anyhow::anyhow!("failed to format sink error").into());
            }
        }
        Err(anyhow::anyhow!(msg).into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::event::Severity;
    use crate::sinks::{BoxFuture, Sink};

    #[derive(Debug)]
    struct TestSink {
        name: &'static str,
        behavior: TestSinkBehavior,
    }

    #[derive(Debug, Clone, Copy)]
    enum TestSinkBehavior {
        Ok,
        Err,
        Sleep(Duration),
        Panic,
    }

    impl Sink for TestSink {
        fn name(&self) -> &'static str {
            self.name
        }

        fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
            Box::pin(async move {
                match self.behavior {
                    TestSinkBehavior::Ok => Ok(()),
                    TestSinkBehavior::Err => Err(anyhow::anyhow!("boom").into()),
                    TestSinkBehavior::Sleep(d) => {
                        tokio::time::sleep(d).await;
                        Ok(())
                    }
                    TestSinkBehavior::Panic => panic!("boom"),
                }
            })
        }
    }

    #[test]
    fn try_notify_errors_without_tokio_runtime() {
        let hub = Hub::new(HubConfig::default(), Vec::new());
        let event = Event::new("kind", Severity::Info, "title");
        assert_eq!(hub.try_notify(event), Err(TryNotifyError::NoTokioRuntime));
    }

    #[test]
    fn try_notify_is_noop_when_kind_disabled_even_without_runtime() {
        let mut enabled_kinds = BTreeSet::new();
        enabled_kinds.insert("enabled".to_string());

        let hub = Hub::new(
            HubConfig {
                enabled_kinds: Some(enabled_kinds),
                per_sink_timeout: Duration::from_secs(1),
            },
            Vec::new(),
        );

        let event = Event::new("disabled", Severity::Info, "title");
        assert_eq!(hub.try_notify(event), Ok(()));
    }

    #[test]
    fn send_aggregates_sink_failures() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![
                Arc::new(TestSink {
                    name: "ok",
                    behavior: TestSinkBehavior::Ok,
                }),
                Arc::new(TestSink {
                    name: "bad",
                    behavior: TestSinkBehavior::Err,
                }),
            ];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected sink failure");
            let msg = err.to_string();
            assert!(msg.contains("one or more sinks failed:"), "{msg}");
            assert!(msg.contains("- bad: boom"), "{msg}");
        });
    }

    #[test]
    fn send_times_out_slow_sinks() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "slow",
                behavior: TestSinkBehavior::Sleep(Duration::from_millis(50)),
            })];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_millis(5),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected timeout");
            let msg = err.to_string();
            assert!(msg.contains("timeout after"), "{msg}");
        });
    }

    #[test]
    fn try_notify_drops_when_overloaded() {
        #[derive(Debug)]
        struct CountingSink {
            counter: Arc<AtomicUsize>,
            sleep: Duration,
        }

        impl Sink for CountingSink {
            fn name(&self) -> &'static str {
                "counting"
            }

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
                Box::pin(async move {
                    self.counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(self.sleep).await;
                    Ok(())
                })
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let counter = Arc::new(AtomicUsize::new(0));
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(CountingSink {
                counter: counter.clone(),
                sleep: Duration::from_millis(50),
            })];

            let hub = Hub::new_with_inflight_limit(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
                1,
            );

            hub.try_notify(Event::new("kind", Severity::Info, "t1"))
                .expect("first notify ok");
            assert_eq!(
                hub.try_notify(Event::new("kind", Severity::Info, "t2")),
                Err(TryNotifyError::Overloaded)
            );

            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn send_includes_sink_name_on_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(TestSink {
                name: "panic",
                behavior: TestSinkBehavior::Panic,
            })];

            let hub = Hub::new(
                HubConfig {
                    enabled_kinds: None,
                    per_sink_timeout: Duration::from_secs(1),
                },
                sinks,
            );
            let event = Event::new("kind", Severity::Info, "title");

            let err = hub.send(event).await.expect_err("expected panic failure");
            let msg = err.to_string();
            assert!(msg.contains("- panic:"), "{msg}");
        });
    }
}
