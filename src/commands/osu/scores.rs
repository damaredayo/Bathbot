use crate::{
    arguments::{Args, NameMapArgs},
    bail,
    embeds::{EmbedData, ScoresEmbed},
    pagination::{Pagination, ScoresPagination},
    util::{
        constants::{GENERAL_ISSUE, OSU_API_ISSUE},
        osu::{cached_message_extract, map_id_from_history, MapIdType},
        MessageExt,
    },
    BotResult, Context,
};

use rosu::backend::requests::{BeatmapRequest, ScoreRequest};
use std::sync::Arc;
use twilight_model::channel::Message;

#[command]
#[short_desc("Each mod's top score from a player on a map")]
#[long_desc(
    "Display a user's top score for each mod on a given map. \n\
     If no map is given, I will choose the last map \
     I can find in my embeds of this channel"
)]
#[usage("[username] [map url / map id]")]
#[example(
    "badewanne3",
    "badewanne3 2240404",
    "badewanne3 https://osu.ppy.sh/beatmapsets/902425#osu/2240404"
)]
#[aliases("c", "compare")]
async fn scores(ctx: Arc<Context>, msg: &Message, args: Args) -> BotResult<()> {
    let args = NameMapArgs::new(&ctx, args);
    let map_id = if let Some(id) = args.map_id {
        match id {
            MapIdType::Map(id) => id,
            MapIdType::Set(_) => {
                let content = "Looks like you gave me a mapset id, I need a map id though";
                return msg.error(&ctx, content).await;
            }
        }
    } else if let Some(id) = ctx
        .cache
        .message_extract(msg.channel_id, cached_message_extract)
    {
        id.id()
    } else {
        let req = ctx.http.channel_messages(msg.channel_id).limit(40).unwrap();
        let msg_results = if let Some(earliest_cached) = ctx.cache.first_message(msg.channel_id) {
            req.before(earliest_cached).await
        } else {
            req.await
        };
        let msgs = match msg_results {
            Ok(msgs) => msgs,
            Err(why) => {
                let _ = msg.error(&ctx, GENERAL_ISSUE).await;
                bail!("error while retrieving messages: {}", why);
            }
        };
        match map_id_from_history(msgs) {
            Some(MapIdType::Map(id)) => id,
            Some(MapIdType::Set(_)) => {
                let content = "Looks like you gave me a mapset id, I need a map id though";
                return msg.error(&ctx, content).await;
            }
            None => {
                let content = "No beatmap specified and none found in recent channel history. \
                    Try specifying a map either by url to the map, or just by map id.";
                return msg.error(&ctx, content).await;
            }
        }
    };
    let name = match args.name.or_else(|| ctx.get_link(msg.author.id.0)) {
        Some(name) => name,
        None => return super::require_link(&ctx, msg).await,
    };

    // Retrieving the beatmap
    let map = match ctx.psql().get_beatmap(map_id).await {
        Ok(map) => map,
        Err(_) => {
            let map_req = BeatmapRequest::new().map_id(map_id);
            match map_req.queue_single(ctx.osu()).await {
                Ok(Some(map)) => map,
                Ok(None) => {
                    let content = format!(
                        "Could not find beatmap with id `{}`. \
                        Did you give me a mapset id instead of a map id?",
                        map_id
                    );
                    return msg.error(&ctx, content).await;
                }
                Err(why) => {
                    let _ = msg.error(&ctx, OSU_API_ISSUE).await;
                    return Err(why.into());
                }
            }
        }
    };

    // Retrieve user and their scores on the map
    let score_req = ScoreRequest::with_map_id(map_id)
        .username(&name)
        .mode(map.mode);
    let join_result = tokio::try_join!(ctx.osu_user(&name, map.mode), score_req.queue(ctx.osu()));
    let (user, scores) = match join_result {
        Ok((Some(user), scores)) => (user, scores),
        Ok((None, _)) => {
            let content = format!("Could not find user `{}`", name);
            return msg.error(&ctx, content).await;
        }
        Err(why) => {
            let _ = msg.error(&ctx, OSU_API_ISSUE).await;
            return Err(why.into());
        }
    };
    let init_scores = scores.iter().take(10);

    // Accumulate all necessary data
    let data = ScoresEmbed::new(&ctx, &user, &map, init_scores, 0).await;

    // Sending the embed
    let embed = data.build().build()?;
    let response = ctx
        .http
        .create_message(msg.channel_id)
        .embed(embed)?
        .await?;

    // Add map to database if its not in already
    if let Err(why) = ctx.clients.psql.insert_beatmap(&map).await {
        warn!("Error while adding new map to DB: {}", why);
    }

    // Skip pagination if too few entries
    if scores.len() <= 10 {
        response.reaction_delete(&ctx, msg.author.id);
        return Ok(());
    }

    // Pagination
    let pagination = ScoresPagination::new(Arc::clone(&ctx), response, user, map, scores);
    let owner = msg.author.id;
    tokio::spawn(async move {
        if let Err(why) = pagination.start(&ctx, owner, 60).await {
            warn!("Pagination error (scores): {}", why)
        }
    });
    Ok(())
}
