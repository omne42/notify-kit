use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::event::Event;
use crate::sinks::Sink;

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Optional allow-list for event kinds.
    ///
    /// - `None`: allow all event kinds.
    /// - `Some(set)`: only allow event kinds present in the set.
    pub enabled_kinds: Option<BTreeSet<String>>,
    /// Per-sink timeout to ensure notifications never block the caller.
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
}

impl Hub {
    pub fn new(config: HubConfig, sinks: Vec<Arc<dyn Sink>>) -> Self {
        let inner = HubInner {
            enabled_kinds: config.enabled_kinds,
            sinks,
            per_sink_timeout: config.per_sink_timeout,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn notify(&self, event: Event) {
        if let Some(enabled) = &self.inner.enabled_kinds {
            if !enabled.contains(event.kind.as_str()) {
                return;
            }
        }

        let inner = self.inner.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                sink = "hub",
                "notify dropped (no tokio runtime): kind={:?} title={:?}",
                event.kind,
                event.title
            );
            return;
        };

        handle.spawn(async move {
            if let Err(err) = inner.send(event).await {
                tracing::warn!(sink = "hub", "notify failed: {err:#}");
            }
        });
    }
}

impl HubInner {
    async fn send(self: Arc<Self>, event: Event) -> anyhow::Result<()> {
        let mut join_set = tokio::task::JoinSet::<(String, anyhow::Result<()>)>::new();

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
