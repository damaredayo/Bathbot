use super::song_send;
use crate::{Args, BotResult, Context};

use std::sync::Arc;
use twilight::model::channel::Message;

#[command]
#[short_desc("https://youtu.be/hjGZLnja1o8?t=41")]
#[bucket("songs")]
#[aliases("1273")]
pub async fn rockefeller(ctx: Arc<Context>, msg: &Message, _: Args) -> BotResult<()> {
    let lyrics = &[
        "1 - 2 - 7 - 3",
        "down the Rockefeller street.",
        "Life is marchin' on, do you feel that?",
        "1 - 2 - 7 - 3",
        "down the Rockefeller street.",
        "Everything is more than surreal",
    ];
    song_send(lyrics, 2250, ctx, msg).await
}
