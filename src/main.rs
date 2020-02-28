mod commands;
mod messages;
pub mod util;
#[macro_use]
mod macros;
pub mod database;
mod scraper;
mod streams;

#[macro_use]
extern crate log;
#[macro_use]
extern crate diesel;

use crate::scraper::Scraper;
use commands::{fun::*, osu::*, streams::*, utility::*};
use database::{MySQL, Platform, StreamTrack};
use messages::BasicEmbedData;
use streams::{Twitch, TwitchStream};
pub use util::Error;

use chrono::{DateTime, Utc};
use log::{error, info};
use rosu::backend::Osu as OsuClient;
use serenity::{
    framework::{standard::DispatchError, StandardFramework},
    model::{
        channel::{Channel, Reaction},
        event::ResumedEvent,
        gateway::Ready,
        guild::Guild,
        id::{ChannelId, GuildId, MessageId, RoleId},
        voice::VoiceState,
    },
    prelude::*,
};
use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
};
use strfmt::strfmt;
use tokio::runtime::Runtime;
use white_rabbit::{DateResult, Duration, Scheduler, Utc as UtcWR};

fn setup() -> Result<(), Error> {
    kankyo::load()?;
    env_logger::init();
    Ok(())
}

fn main() -> Result<(), Error> {
    setup()?;
    // -----------------
    // Data preparations
    // -----------------

    // Discord
    let discord_token = env::var("DISCORD_TOKEN")?;
    let mut discord = Client::new(&discord_token, Handler)?;

    // Database
    let database_url = env::var("DATABASE_URL")?;
    let mysql = MySQL::new(&database_url)?;

    // Osu
    let osu_token = env::var("OSU_TOKEN")?;
    let osu = OsuClient::new(osu_token);
    let discord_links = mysql.get_discord_links()?;

    // Scraper
    let mut rt = Runtime::new().expect("Could not create runtime");
    let scraper = rt.block_on(Scraper::new())?;

    // Stream tracking
    let twitch_users = mysql.get_twitch_users()?;
    let stream_tracks = mysql.get_stream_tracks()?;
    let twitch_client_id = env::var("TWITCH_CLIENT_ID")?;
    let twitch_token = env::var("TWITCH_TOKEN")?;
    let twitch = Twitch::new(&twitch_client_id, &twitch_token)?;

    // General
    let owners = match discord.cache_and_http.http.get_current_application_info() {
        Ok(info) => {
            let mut set = HashSet::new();
            set.insert(info.owner.id);
            set
        }
        Err(why) => {
            return Err(Error::Custom(format!(
                "Couldn't get application info: {:?}",
                why
            )))
        }
    };
    let scheduler = Scheduler::new(4);
    let now = Utc::now();

    // Insert everything
    {
        let mut data = discord.data.write();
        data.insert::<CommandCounter>(HashMap::default());
        data.insert::<Osu>(osu);
        data.insert::<Scraper>(scraper);
        data.insert::<MySQL>(mysql);
        data.insert::<DiscordLinks>(discord_links);
        data.insert::<BootTime>(now);
        data.insert::<PerformanceCalculatorLock>(Arc::new(Mutex::new(())));
        data.insert::<SchedulerKey>(Arc::new(RwLock::new(scheduler)));
        data.insert::<TwitchUsers>(twitch_users);
        data.insert::<StreamTracks>(stream_tracks);
        data.insert::<OnlineTwitch>(HashSet::new());
        data.insert::<Twitch>(twitch);
    }

    // ---------------
    // Framework setup
    // ---------------

    discord.with_framework(
        StandardFramework::new()
            .configure(|c| {
                c.prefixes(vec!["<", "!!"])
                    .owners(owners)
                    .delimiter(' ')
                    .case_insensitivity(true)
                    .ignore_bots(true)
                    .no_dm_prefix(true)
            })
            .on_dispatch_error(|ctx, msg, error| {
                if let DispatchError::Ratelimited(seconds) = error {
                    let _ = msg.channel_id.say(
                        &ctx.http,
                        &format!("Command on cooldown, try again in {} seconds", seconds),
                    );
                }
            })
            .help(&HELP)
            .group(&OSUGENERAL_GROUP)
            .group(&OSU_GROUP)
            .group(&MANIA_GROUP)
            .group(&TAIKO_GROUP)
            .group(&CATCHTHEBEAT_GROUP)
            .group(&STREAMS_GROUP)
            .group(&FUN_GROUP)
            .group(&UTILITY_GROUP)
            .bucket("two_per_thirty_cooldown", |b| {
                b.delay(5).time_span(30).limit(2)
            })
            .before(|ctx, msg, cmd_name| {
                let location = match msg.guild(&ctx) {
                    Some(guild) => {
                        let guild_name = guild.read().name.clone();
                        let channel_name = if let Channel::Guild(channel) =
                            msg.channel(&ctx).unwrap()
                        {
                            channel.read().name.clone()
                        } else {
                            panic!("Found non-Guild channel of msg despite msg being in a guild");
                        };
                        format!("{}:{}", guild_name, channel_name)
                    }
                    None => "Private".to_owned(),
                };
                info!("[{}] {}: {}", location, msg.author.name, msg.content,);
                match ctx.data.write().get_mut::<CommandCounter>() {
                    Some(counter) => *counter.entry(cmd_name.to_owned()).or_insert(0) += 1,
                    None => error!("Could not get CommandCounter"),
                }
                true
            })
            .after(|_, _, cmd_name, error| match error {
                Ok(()) => info!("Processed command '{}'", cmd_name),
                Err(why) => error!("Command '{}' returned error {:?}", cmd_name, why),
            }),
    );
    discord.start()?;
    Ok(())
}

// --------------
// Event handling
// --------------

struct Handler;
impl EventHandler for Handler {
    fn ready(&self, _: Context, ready: Ready) {
        info!("Connected as {}", ready.user.name);
    }

    fn resume(&self, _: Context, _: ResumedEvent) {
        info!("Resumed connection");
    }

    fn guild_create(&self, _: Context, guild: Guild, is_new: bool) {
        if is_new {
            info!("'guild_create' triggered for new server '{}'", guild.name);
        }
    }

    fn voice_state_update(
        &self,
        _ctx: Context,
        _guild: Option<GuildId>,
        _old: Option<VoiceState>,
        _new: VoiceState,
    ) {
        // TODO
    }

    fn cache_ready(&self, ctx: Context, _: Vec<GuildId>) {
        // Tracking streams
        #[allow(non_snake_case)]
        let WITH_STREAM_TRACK = true;
        if WITH_STREAM_TRACK {
            let track_delay = 10;
            let scheduler = {
                let mut data = ctx.data.write();
                data.get_mut::<SchedulerKey>()
                    .expect("Could not get SchedulerKey")
                    .clone()
            };
            let mut scheduler = scheduler.write();
            let http = ctx.http.clone();
            let data = ctx.data.clone();
            scheduler.add_task_duration(Duration::seconds(track_delay), move |_| {
                //debug!("Checking stream tracks...");
                let now_online = {
                    let reading = data.read();

                    // Get data about what needs to be tracked for which channel
                    let stream_tracks = reading
                        .get::<StreamTracks>()
                        .expect("Could not get StreamTracks");
                    let user_ids: Vec<_> = stream_tracks
                        .iter()
                        .filter(|track| track.platform == Platform::Twitch)
                        .map(|track| track.user_id)
                        .collect();
                    // Twitch provides up to 100 streams per request, otherwise its trimmed
                    if user_ids.len() > 100 {
                        warn!("Reached 100 twitch trackings, improve handling!");
                    }

                    // Get stream data about all streams that need to be tracked
                    let twitch = reading.get::<Twitch>().expect("Could not get Twitch");
                    let mut rt = Runtime::new().expect("Could not create runtime for streams");
                    let mut streams = match rt.block_on(twitch.get_streams(&user_ids)) {
                        Ok(streams) => streams,
                        Err(why) => {
                            warn!("Error while retrieving streams: {}", why);
                            return DateResult::Repeat(
                                UtcWR::now() + Duration::seconds(track_delay),
                            );
                        }
                    };

                    // Filter streams whether they're live
                    streams.retain(TwitchStream::is_live);
                    let online_streams = reading
                        .get::<OnlineTwitch>()
                        .expect("Could not get OnlineTwitch");
                    let now_online: HashSet<_> =
                        streams.iter().map(|stream| stream.user_id).collect();

                    // If there was no activity change since last time, don't do anything
                    if &now_online == online_streams {
                        //debug!("No activity change");
                        None
                    } else {
                        // Filter streams whether its already known they're live
                        streams.retain(|stream| !online_streams.contains(&stream.user_id));
                        let mut fmt_data = HashMap::new();
                        fmt_data.insert(String::from("width"), String::from("360"));
                        fmt_data.insert(String::from("height"), String::from("180"));

                        // Put streams into a more suitable data type and process the thumbnail url
                        let streams: HashMap<u64, TwitchStream> = streams
                            .into_iter()
                            .map(|mut stream| {
                                if let Ok(thumbnail) = strfmt(&stream.thumbnail_url, &fmt_data) {
                                    stream.thumbnail_url = thumbnail;
                                }
                                (stream.user_id, stream)
                            })
                            .collect();

                        // Process each tracking by notifying corresponding channels
                        for track in stream_tracks {
                            if streams.contains_key(&track.user_id) {
                                let stream = streams.get(&track.user_id).unwrap();
                                let data = BasicEmbedData::create_twitch_stream_notif(stream);
                                let _ = ChannelId(track.channel_id)
                                    .send_message(&http, |m| m.embed(|e| data.build(e)));
                            }
                        }
                        Some(now_online)
                    }
                };
                if let Some(now_online) = now_online {
                    let mut writing = data.write();
                    let online_twitch = writing
                        .get_mut::<OnlineTwitch>()
                        .expect("Could not get OnlineTwitch");
                    online_twitch.clear();
                    for id in now_online {
                        online_twitch.insert(id);
                    }
                }
                //debug!("Stream track check done");
                DateResult::Repeat(UtcWR::now() + Duration::seconds(track_delay))
            });
        }

        // Tracking reactions
        let reaction_tracker: HashMap<_, _> = match ctx.data.read().get::<MySQL>() {
            Some(mysql) => mysql
                .get_role_assigns()
                .expect("Could not get role assigns")
                .into_iter()
                .map(|((c, m), r)| ((ChannelId(c), MessageId(m)), RoleId(r)))
                .collect(),
            None => panic!("Could not get MySQL"),
        };
        {
            let mut data = ctx.data.write();
            data.insert::<ReactionTracker>(reaction_tracker);
        }
    }

    fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        let key = (reaction.channel_id, reaction.message_id);
        let role: Option<RoleId> = match ctx.data.read().get::<ReactionTracker>() {
            Some(tracker) => {
                if tracker.contains_key(&key) {
                    Some(*tracker.get(&key).unwrap())
                } else {
                    None
                }
            }
            None => {
                error!("Could not get ReactionTracker");
                return;
            }
        };
        if let Some(role) = role {
            let channel = match reaction.channel(&ctx) {
                Ok(channel) => channel,
                Err(why) => {
                    error!("Could not get Channel from reaction: {}", why);
                    return;
                }
            };
            let guild_lock = match channel.guild() {
                Some(guild_channel) => match guild_channel.read().guild(&ctx) {
                    Some(guild) => guild,
                    None => {
                        error!("Could not get Guild from reaction");
                        return;
                    }
                },
                None => {
                    error!("Could not get GuildChannel from reaction");
                    return;
                }
            };
            let guild = guild_lock.read();
            let mut member = match guild.member(&ctx, reaction.user_id) {
                Ok(member) => member,
                Err(why) => {
                    error!("Could not get Member from reaction: {}", why);
                    return;
                }
            };
            let role_name = role
                .to_role_cached(&ctx.cache)
                .expect("Role not found in cache")
                .name;
            if let Err(why) = member.add_role(&ctx.http, role) {
                error!("Could not add role to member for reaction: {}", why);
            } else {
                info!(
                    "Assigned role '{}' to member {}",
                    role_name,
                    member.user.read().name
                );
            }
        }
    }

    fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        let key = (reaction.channel_id, reaction.message_id);
        let role = match ctx.data.read().get::<ReactionTracker>() {
            Some(tracker) => {
                if tracker.contains_key(&key) {
                    Some(*tracker.get(&key).unwrap())
                } else {
                    None
                }
            }
            None => {
                error!("Could not get ReactionTracker");
                return;
            }
        };
        if let Some(role) = role {
            let channel = match reaction.channel(&ctx) {
                Ok(channel) => channel,
                Err(why) => {
                    error!("Could not get Channel from reaction: {}", why);
                    return;
                }
            };
            let guild_lock = match channel.guild() {
                Some(guild_channel) => match guild_channel.read().guild(&ctx) {
                    Some(guild) => guild,
                    None => {
                        error!("Could not get Guild from reaction");
                        return;
                    }
                },
                None => {
                    error!("Could not get GuildChannel from reaction");
                    return;
                }
            };
            let guild = guild_lock.read();
            let mut member = match guild.member(&ctx, reaction.user_id) {
                Ok(member) => member,
                Err(why) => {
                    error!("Could not get Member from reaction: {}", why);
                    return;
                }
            };
            let role_name = role
                .to_role_cached(&ctx.cache)
                .expect("Role not found in cache")
                .name;
            if let Err(why) = member.remove_role(&ctx.http, role) {
                error!("Could not remove role from member for reaction: {}", why);
            } else {
                info!(
                    "Removed role '{}' from member {}",
                    role_name,
                    member.user.read().name
                );
            }
        }
    }
}

// ------------------
// Struct definitions
// ------------------

pub struct CommandCounter;
impl TypeMapKey for CommandCounter {
    type Value = HashMap<String, u32>;
}

pub struct Osu;
impl TypeMapKey for Osu {
    type Value = OsuClient;
}

impl TypeMapKey for Scraper {
    type Value = Scraper;
}

impl TypeMapKey for MySQL {
    type Value = MySQL;
}

pub struct DiscordLinks;
impl TypeMapKey for DiscordLinks {
    type Value = HashMap<u64, String>;
}

pub struct BootTime;
impl TypeMapKey for BootTime {
    type Value = DateTime<Utc>;
}

pub struct PerformanceCalculatorLock;
impl TypeMapKey for PerformanceCalculatorLock {
    type Value = Arc<Mutex<()>>;
}

pub struct SchedulerKey;
impl TypeMapKey for SchedulerKey {
    type Value = Arc<RwLock<Scheduler>>;
}

pub struct ReactionTracker;
impl TypeMapKey for ReactionTracker {
    type Value = HashMap<(ChannelId, MessageId), RoleId>;
}

pub struct TwitchUsers;
impl TypeMapKey for TwitchUsers {
    type Value = HashMap<String, u64>;
}

pub struct StreamTracks;
impl TypeMapKey for StreamTracks {
    type Value = HashSet<StreamTrack>;
}

pub struct OnlineTwitch;
impl TypeMapKey for OnlineTwitch {
    type Value = HashSet<u64>;
}

impl TypeMapKey for Twitch {
    type Value = Twitch;
}
