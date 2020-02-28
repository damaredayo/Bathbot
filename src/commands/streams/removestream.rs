use crate::{
    commands::checks::*,
    database::{Platform, StreamTrack},
    MySQL, StreamTracks, TwitchUsers,
};

use serenity::{
    framework::standard::{macros::command, Args, CommandResult},
    model::prelude::Message,
    prelude::Context,
};

#[command]
#[checks(Authority)]
#[description = "Let me no longer notify this channel when the given stream comes online"]
#[aliases("streamremove")]
#[usage = "twitch/mixer [stream name]"]
fn removestream(ctx: &mut Context, msg: &Message, mut args: Args) -> CommandResult {
    // Parse the platform and stream name
    let result = if args.len() < 2 {
        msg.channel_id.say(
            &ctx.http,
            "The first argument must be either `twitch` or `mixer`. \
             The next argument must be the name of the stream.",
        )?;
        return Ok(());
    } else {
        let platform = match args.single::<String>()?.to_lowercase().as_str() {
            "twitch" => Platform::Twitch,
            "mixer" => Platform::Mixer,
            _ => {
                msg.channel_id.say(
                    &ctx.http,
                    "The first argument must be either `twitch` or `mixer`. \
                     The next argument must be the name of the stream.",
                )?;
                return Ok(());
            }
        };
        let name = args.single::<String>()?.to_lowercase();
        match platform {
            Platform::Mixer => Some((platform, "TODO".to_string())),
            Platform::Twitch => {
                let data = ctx.data.read();
                let twitch_users = data
                    .get::<TwitchUsers>()
                    .expect("Could not get TwitchUsers");
                if twitch_users.contains_key(&name) {
                    let twitch_id = *twitch_users.get(&name).unwrap();
                    std::mem::drop(data);
                    let mut data = ctx.data.write();
                    let stream_tracks = data
                        .get_mut::<StreamTracks>()
                        .expect("Could not get StreamTracks");
                    let track = StreamTrack::new(msg.channel_id.0, twitch_id, platform);
                    if stream_tracks.remove(&track) {
                        let mysql = data.get::<MySQL>().expect("Could not get MySQL");
                        if let Err(why) =
                            mysql.remove_stream_track(msg.channel_id.0, twitch_id, platform)
                        {
                            warn!("Error while removing stream track: {}", why);
                        }
                    }
                    Some((platform, name))
                } else {
                    None
                }
            }
        }
    };
    let content = if let Some((platform, name)) = result {
        format!(
            "I'm no longer tracking {}'s {:?} stream in this channel",
            name, platform
        )
    } else {
        format!("That stream wasn't tracked anyway")
    };

    // Sending the msg
    msg.channel_id.say(&ctx.http, content)?;
    Ok(())
}
