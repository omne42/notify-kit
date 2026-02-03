use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::event::Event;
use crate::sinks::Sink;

const DEFAULT_MAX_INFLIGHT_EVENTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryNotifyError {
    NoTokioRuntime,
}

impl std::fmt::Display for TryNotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTokioRuntime => write!(f, "no tokio runtime"),
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
    /// `per_sink_timeout` >= that value, otherwise `Hub` may time out first.
    pub per_sink_timeout: Duration,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            enabled_kinds: None,
            per_sink_timeout: Duration::from_secs(2),
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
        let kind = event.kind.clone();
        if let Err(err) = self.try_notify(event) {
            tracing::warn!(sink = "hub", kind = %kind, "notify dropped: {err}");
        }
    }

    /// Attempt to enqueue a fire-and-forget notification.
    ///
    /// Returns `Err(TryNotifyError::NoTokioRuntime)` if called outside a Tokio runtime.
    pub fn try_notify(&self, event: Event) -> Result<(), TryNotifyError> {
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        let inner = self.inner.clone();
        let kind = event.kind.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return Err(TryNotifyError::NoTokioRuntime);
        };

        let permit = match inner.inflight.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(sink = "hub", kind = %kind, "notify dropped: overloaded");
                return Ok(());
            }
        };

        handle.spawn(async move {
            let _permit = permit;
            if let Err(err) = inner.send(event).await {
                tracing::warn!(sink = "hub", kind = %kind, "notify failed: {err}");
            }
        });

        Ok(())
    }

    pub async fn send(&self, event: Event) -> anyhow::Result<()> {
        if !self.is_kind_enabled(event.kind.as_str()) {
            return Ok(());
        }

        tokio::runtime::Handle::try_current().map_err(|_| TryNotifyError::NoTokioRuntime)?;
        let _permit = self
            .inner
            .inflight
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("hub inflight semaphore closed"))?;
        self.inner.clone().send(event).await
    }

    fn is_kind_enabled(&self, kind: &str) -> bool {
        let Some(enabled) = &self.inner.enabled_kinds else {
            return true;
        };
        enabled.contains(kind)
    }
}

impl HubInner {
    async fn send(self: Arc<Self>, event: Event) -> anyhow::Result<()> {
        let mut join_set = tokio::task::JoinSet::<(String, anyhow::Result<()>)>::new();
        let event = Arc::new(event);

        for sink in &self.sinks {
            let sink = sink.clone();
            let event = event.clone();
            let timeout = self.per_sink_timeout;
            join_set.spawn(async move {
                let name = sink.name().to_string();
                let res = match tokio::time::timeout(timeout, sink.send(&event)).await {
                    Ok(inner) => inner,
                    Err(_) => Err(anyhow::anyhow!("timeout after {timeout:?}")),
                };
                (name, res)
            });
        }

        let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((_name, Ok(()))) => {}
                Ok((name, Err(err))) => failures.push((name, err)),
                Err(err) => failures.push(("join".to_string(), err.into())),
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        let mut msg = String::from("one or more sinks failed:");
        for (name, err) in failures {
            msg.push_str(&format!("\n- {name}: {err:#}"));
        }
        Err(anyhow::anyhow!(msg))
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
    }

    impl Sink for TestSink {
        fn name(&self) -> &'static str {
            self.name
        }

        fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                match self.behavior {
                    TestSinkBehavior::Ok => Ok(()),
                    TestSinkBehavior::Err => Err(anyhow::anyhow!("boom")),
                    TestSinkBehavior::Sleep(d) => {
                        tokio::time::sleep(d).await;
                        Ok(())
                    }
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

            fn send<'a>(&'a self, _event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
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
            hub.try_notify(Event::new("kind", Severity::Info, "t2"))
                .expect("second notify ok (dropped)");

            tokio::time::sleep(Duration::from_millis(80)).await;
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        });
    }
}
