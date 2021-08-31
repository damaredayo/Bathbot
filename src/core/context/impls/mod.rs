mod background_loop;
mod bg_game;
mod configs;
mod match_live;
mod shutdown;
mod twitch;

pub use background_loop::GarbageCollectMap;
pub use match_live::{MatchLiveChannels, MatchTrackResult};

use crate::{util::CountryCode, BotResult, Context, OsuTracking};

use twilight_model::{
    channel::{Message, Reaction},
    id::{ChannelId, MessageId, RoleId},
};

impl Context {
    /// Returns whether a message was sent by us
    pub fn is_own(&self, other: &Message) -> bool {
        self.cache
            .current_user()
            .map_or(false, |user| user.id == other.author.id)
    }

    pub fn get_role_assign(&self, reaction: &Reaction) -> Option<RoleId> {
        self.data
            .role_assigns
            .get(&(reaction.channel_id.0, reaction.message_id.0))
            .map(|guard| RoleId(*guard.value()))
    }

    pub fn tracking(&self) -> &OsuTracking {
        &self.data.osu_tracking
    }

    pub async fn retrieve_channel_history(&self, channel_id: ChannelId) -> BotResult<Vec<Message>> {
        self.http
            .channel_messages(channel_id)
            .limit(50)
            .unwrap()
            .exec()
            .await?
            .models()
            .await
            .map_err(|e| e.into())
    }

    pub fn store_msg(&self, msg: MessageId) {
        self.data.msgs_to_process.insert(msg);
    }

    pub fn remove_msg(&self, msg: MessageId) -> bool {
        self.data.msgs_to_process.remove(&msg).is_some()
    }

    #[cold]
    pub fn clear_msgs_to_process(&self) {
        self.data.msgs_to_process.clear();
    }

    pub fn add_country(&self, country: String, code: CountryCode) {
        self.data.snipe_countries.insert(code, country);
    }

    pub fn contains_country(&self, code: &str) -> bool {
        self.data.snipe_countries.contains_key(code)
    }

    pub fn get_country(&self, code: &str) -> Option<String> {
        self.data
            .snipe_countries
            .get(code)
            .map(|entry| entry.value().to_owned())
    }

    #[cold]
    pub fn add_role_assign(&self, channel_id: ChannelId, msg_id: MessageId, role_id: RoleId) {
        self.data
            .role_assigns
            .insert((channel_id.0, msg_id.0), role_id.0);
    }
}
