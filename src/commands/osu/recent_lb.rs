use crate::{
    arguments::{ModSelection, NameModArgs},
    database::MySQL,
    embeds::BasicEmbedData,
    scraper::{Scraper, ScraperScore},
    util::globals::{AVATAR_URL, OSU_API_ISSUE},
    DiscordLinks, Osu,
};

use rosu::{
    backend::requests::RecentRequest,
    models::{
        ApprovalStatus::{Loved, Ranked},
        Beatmap, GameMode, GameMods,
    },
};
use serenity::{
    cache::CacheRwLock,
    collector::{ReactionAction, ReactionCollectorBuilder},
    framework::standard::{macros::command, Args, CommandError, CommandResult},
    model::channel::{Message, ReactionType},
    prelude::{Context, RwLock, ShareMap},
};
use std::{sync::Arc, time::Duration};

#[allow(clippy::cognitive_complexity)]
async fn recent_lb_send(
    mode: GameMode,
    national: bool,
    ctx: &mut Context,
    msg: &Message,
    args: Args,
) -> CommandResult {
    let init_name = {
        let data = ctx.data.read().await;
        let links = data
            .get::<DiscordLinks>()
            .expect("Could not get DiscordLinks");
        links.get(msg.author.id.as_u64()).cloned()
    };
    let args = NameModArgs::new(args);
    let (mods, selection) = args
        .mods
        .unwrap_or_else(|| (GameMods::default(), ModSelection::None));
    let name = if let Some(name) = args.name {
        name
    } else {
        let data = ctx.data.read().await;
        let links = data
            .get::<DiscordLinks>()
            .expect("Could not get DiscordLinks");
        match links.get(msg.author.id.as_u64()) {
            Some(name) => name.clone(),
            None => {
                msg.channel_id
                    .say(
                        &ctx.http,
                        "Either specify an osu name or link your discord \
                     to an osu profile via `<link osuname`",
                    )
                    .await?;
                return Ok(());
            }
        }
    };

    // Retrieve the recent scores
    let score = {
        let request = RecentRequest::with_username(&name).mode(mode).limit(1);
        let data = ctx.data.read().await;
        let osu = data.get::<Osu>().expect("Could not get osu client");
        match request.queue(osu).await {
            Ok(mut score) => {
                if let Some(score) = score.pop() {
                    score
                } else {
                    msg.channel_id
                        .say(
                            &ctx.http,
                            format!("No recent plays found for user `{}`", name),
                        )
                        .await?;
                    return Ok(());
                }
            }
            Err(why) => {
                msg.channel_id.say(&ctx.http, OSU_API_ISSUE).await?;
                return Err(CommandError::from(why.to_string()));
            }
        }
    };
    let map_id = score.beatmap_id.unwrap();

    // Retrieving the score's beatmap
    let (map_to_db, map) = {
        let data = ctx.data.read().await;
        let mysql = data.get::<MySQL>().expect("Could not get MySQL");
        match mysql.get_beatmap(map_id) {
            Ok(map) => (false, map),
            Err(_) => {
                let osu = data.get::<Osu>().expect("Could not get osu client");
                let map = match score.get_beatmap(osu).await {
                    Ok(m) => m,
                    Err(why) => {
                        msg.channel_id.say(&ctx.http, OSU_API_ISSUE).await?;
                        return Err(CommandError::from(why.to_string()));
                    }
                };
                (
                    map.approval_status == Ranked || map.approval_status == Loved,
                    map,
                )
            }
        }
    };

    // Retrieve the map's leaderboard
    let scores = {
        let data = ctx.data.read().await;
        let scraper = data.get::<Scraper>().expect("Could not get Scraper");
        let scores_future = scraper.get_leaderboard(
            map_id,
            national,
            match selection {
                ModSelection::Excludes | ModSelection::None => None,
                _ => Some(&mods),
            },
        );
        match scores_future.await {
            Ok(scores) => scores,
            Err(why) => {
                msg.channel_id.say(&ctx.http, OSU_API_ISSUE).await?;
                return Err(CommandError::from(why.to_string()));
            }
        }
    };
    let amount = scores.len();

    // Accumulate all necessary data
    let map_copy = if map_to_db { Some(map.clone()) } else { None };
    let author_icon = scores
        .first()
        .map(|s| format!("{}{}", AVATAR_URL, s.user_id));
    let data = match BasicEmbedData::create_leaderboard(
        &init_name.as_deref(),
        &map,
        if scores.is_empty() {
            None
        } else {
            Some(scores.iter().take(10))
        },
        &author_icon,
        0,
        &ctx,
    )
    .await
    {
        Ok(data) => data,
        Err(why) => {
            msg.channel_id
                .say(
                    &ctx.http,
                    "Some issue while calculating leaderboard data, blame bade",
                )
                .await?;
            return Err(CommandError::from(why.to_string()));
        }
    };

    // Sending the embed
    let response = msg
        .channel_id
        .send_message(&ctx.http, |m| {
            let mut content = format!(
                "I found {} scores with the specified mods on the map's leaderboard",
                amount
            );
            if amount > 10 {
                content.push_str(", here's the top 10 of them:");
            } else {
                content.push(':');
            }
            m.content(content).embed(|e| data.build(e))
        })
        .await;

    // Add map to database if its not in already
    if let Some(map) = map_copy {
        let data = ctx.data.read().await;
        let mysql = data.get::<MySQL>().expect("Could not get MySQL");
        if let Err(why) = mysql.insert_beatmap(&map) {
            warn!("Could not add map of recent command to database: {}", why);
        }
    }
    let mut response = response?;

    // Collect reactions of author on the response
    let mut collector = ReactionCollectorBuilder::new(&ctx)
        .author_id(msg.author.id)
        .message_id(response.id)
        .timeout(Duration::from_secs(60))
        .await;
    let mut idx = 0;

    // Add initial reactions
    let reactions = ["⏮️", "⏪", "⏩", "⏭️"];
    for &reaction in reactions.iter() {
        response.react(&ctx.http, reaction).await?;
    }

    // Check if the author wants to edit the response
    let http = Arc::clone(&ctx.http);
    let cache = ctx.cache.clone();
    let data = Arc::clone(&ctx.data);
    tokio::spawn(async move {
        let author_name = init_name.as_deref();
        while let Some(reaction) = collector.receive_one().await {
            if let ReactionAction::Added(reaction) = &*reaction {
                if let ReactionType::Unicode(reaction_name) = &reaction.emoji {
                    if reaction_name.as_str() == "❌" {
                        response.delete((&cache, &*http)).await?;
                    } else if !scores.is_empty() {
                        let reaction_data = reaction_data(
                            reaction_name.as_str(),
                            &mut idx,
                            &map,
                            &scores,
                            &author_name,
                            &author_icon,
                            &cache,
                            &data,
                        );
                        match reaction_data.await {
                            ReactionData::None => {}
                            ReactionData::Data(data) => {
                                response
                                    .edit((&cache, &*http), |m| m.embed(|e| data.build(e)))
                                    .await?
                            }
                        }
                    }
                }
            }
        }
        for &reaction in reactions.iter() {
            response
                .channel_id
                .delete_reaction(&http, response.id, None, reaction)
                .await?;
        }
        Ok::<_, serenity::Error>(())
    });
    Ok(())
}

enum ReactionData {
    Data(Box<BasicEmbedData>),
    None,
}

#[allow(clippy::too_many_arguments)]
async fn reaction_data(
    reaction: &str,
    idx: &mut usize,
    map: &Beatmap,
    scores: &[ScraperScore],
    author_name: &Option<&str>,
    author_icon: &Option<String>,
    cache: &CacheRwLock,
    data: &Arc<RwLock<ShareMap>>,
) -> ReactionData {
    let amount = scores.len();
    match reaction {
        "⏮️" => {
            if *idx > 0 {
                *idx = 0;
                BasicEmbedData::create_leaderboard(
                    author_name,
                    map,
                    Some(scores.iter().take(10)),
                    author_icon,
                    *idx,
                    (cache, data),
                )
                .await
                .map(|data| ReactionData::Data(Box::new(data)))
                .unwrap_or_else(|why| {
                    warn!("Error editing rlb data at idx {}/{}: {}", idx, amount, why);
                    ReactionData::None
                })
            } else {
                ReactionData::None
            }
        }
        "⏪" => {
            if *idx > 0 {
                *idx = idx.saturating_sub(10);
                BasicEmbedData::create_leaderboard(
                    author_name,
                    map,
                    Some(scores.iter().skip(*idx).take(10)),
                    author_icon,
                    *idx,
                    (cache, data),
                )
                .await
                .map(|data| ReactionData::Data(Box::new(data)))
                .unwrap_or_else(|why| {
                    warn!("Error editing rlb data at idx {}/{}: {}", idx, amount, why);
                    ReactionData::None
                })
            } else {
                ReactionData::None
            }
        }
        "⏩" => {
            let limit = amount.saturating_sub(10);
            if *idx < limit {
                *idx = limit.min(*idx + 10);
                BasicEmbedData::create_leaderboard(
                    author_name,
                    map,
                    Some(scores.iter().skip(*idx).take(10)),
                    author_icon,
                    *idx,
                    (cache, data),
                )
                .await
                .map(|data| ReactionData::Data(Box::new(data)))
                .unwrap_or_else(|why| {
                    warn!("Error editing rlb data at idx {}/{}: {}", idx, amount, why);
                    ReactionData::None
                })
            } else {
                ReactionData::None
            }
        }
        "⏭️" => {
            let limit = amount.saturating_sub(10);
            if *idx < limit {
                *idx = limit;
                BasicEmbedData::create_leaderboard(
                    author_name,
                    map,
                    Some(scores.iter().skip(*idx).take(10)),
                    author_icon,
                    *idx,
                    (cache, data),
                )
                .await
                .map(|data| ReactionData::Data(Box::new(data)))
                .unwrap_or_else(|why| {
                    warn!("Error editing rlb data at idx {}/{}: {}", idx, amount, why);
                    ReactionData::None
                })
            } else {
                ReactionData::None
            }
        }
        _ => ReactionData::None,
    }
}

#[command]
#[description = "Display the belgian leaderboard of a map \
                 that a user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rlb")]
pub async fn recentleaderboard(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    recent_lb_send(GameMode::STD, true, ctx, msg, args).await
}

#[command]
#[description = "Display the belgian leaderboard of a map \
                 that a mania user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rmlb")]
pub async fn recentmanialeaderboard(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    recent_lb_send(GameMode::MNA, true, ctx, msg, args).await
}

#[command]
#[description = "Display the belgian leaderboard of a map \
                 that a taiko user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rtlb")]
pub async fn recenttaikoleaderboard(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    recent_lb_send(GameMode::TKO, true, ctx, msg, args).await
}

#[command]
#[description = "Display the belgian leaderboard of a map \
                 that a ctb user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rclb")]
pub async fn recentctbleaderboard(ctx: &mut Context, msg: &Message, args: Args) -> CommandResult {
    recent_lb_send(GameMode::CTB, true, ctx, msg, args).await
}

#[command]
#[description = "Display the global leaderboard of a map \
                 that a user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rglb")]
pub async fn recentgloballeaderboard(
    ctx: &mut Context,
    msg: &Message,
    args: Args,
) -> CommandResult {
    recent_lb_send(GameMode::STD, false, ctx, msg, args).await
}

#[command]
#[description = "Display the global leaderboard of a map \
                 that a mania user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rmglb")]
pub async fn recentmaniagloballeaderboard(
    ctx: &mut Context,
    msg: &Message,
    args: Args,
) -> CommandResult {
    recent_lb_send(GameMode::MNA, false, ctx, msg, args).await
}

#[command]
#[description = "Display the global leaderboard of a map \
                 that a taiko user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rtglb")]
pub async fn recenttaikogloballeaderboard(
    ctx: &mut Context,
    msg: &Message,
    args: Args,
) -> CommandResult {
    recent_lb_send(GameMode::TKO, false, ctx, msg, args).await
}

#[command]
#[description = "Display the global leaderboard of a map \
                 that a ctb user recently played. Mods can be specified"]
#[usage = "[username] [+mods]"]
#[example = "badewanne3 +hdhr"]
#[aliases("rcglb")]
pub async fn recentctbgloballeaderboard(
    ctx: &mut Context,
    msg: &Message,
    args: Args,
) -> CommandResult {
    recent_lb_send(GameMode::CTB, false, ctx, msg, args).await
}
