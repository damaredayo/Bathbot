use std::time::Duration;

use redlight::config::{CacheConfig, Ignore};

use crate::{
    channel::CachedChannel, current_user::CachedCurrentUser, guild::CachedGuild,
    member::CachedMember, role::CachedRole, user::CachedUser,
};

pub struct Config;

impl CacheConfig for Config {
    type Channel<'a> = CachedChannel<'a>;
    type CurrentUser<'a> = CachedCurrentUser<'a>;
    type Emoji<'a> = Ignore;
    type Guild<'a> = CachedGuild;
    type Integration<'a> = Ignore;
    type Member<'a> = CachedMember;
    type Message<'a> = Ignore;
    type Presence<'a> = Ignore;
    type Role<'a> = CachedRole<'a>;
    type StageInstance<'a> = Ignore;
    type Sticker<'a> = Ignore;
    type User<'a> = CachedUser<'a>;
    type VoiceState<'a> = Ignore;

    const METRICS_INTERVAL_DURATION: Duration = Duration::from_secs(30);
}
