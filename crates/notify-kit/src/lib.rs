#![forbid(unsafe_code)]

mod event;
mod hub;
mod sinks;

pub use crate::event::{Event, Severity};
pub use crate::hub::{Hub, HubConfig, TryNotifyError};
pub use crate::sinks::{
    DingTalkWebhookConfig, DingTalkWebhookSink, DiscordWebhookConfig, DiscordWebhookSink,
    FeishuWebhookConfig, FeishuWebhookSink, GitHubCommentConfig, GitHubCommentSink, Sink,
    SlackWebhookConfig, SlackWebhookSink, SoundConfig, SoundSink, TelegramBotConfig,
    TelegramBotSink, WeComWebhookConfig, WeComWebhookSink,
};
