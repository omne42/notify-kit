mod feishu;
mod sound;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

pub use feishu::FeishuWebhookSink;
pub use sound::SoundSink;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>>;
}
