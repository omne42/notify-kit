mod crypto;
mod dingtalk;
mod discord;
mod feishu;
mod github;
mod http;
mod slack;
mod sound;
mod telegram;
mod text;
mod wecom;

use std::future::Future;
use std::pin::Pin;

use crate::event::Event;

pub use dingtalk::{DingTalkWebhookConfig, DingTalkWebhookSink};
pub use discord::{DiscordWebhookConfig, DiscordWebhookSink};
pub use feishu::{FeishuWebhookConfig, FeishuWebhookSink};
pub use github::{GitHubCommentConfig, GitHubCommentSink};
pub use slack::{SlackWebhookConfig, SlackWebhookSink};
pub use sound::{SoundConfig, SoundSink};
pub use telegram::{TelegramBotConfig, TelegramBotSink};
pub use wecom::{WeComWebhookConfig, WeComWebhookSink};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Sink: Send + Sync {
    fn name(&self) -> &'static str;
    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>>;
}
