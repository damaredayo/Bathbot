use std::sync::Arc;

use bathbot_util::{numbers::WithComma, EmbedBuilder, FooterBuilder, MessageBuilder};
use eyre::{Result, WrapErr};

use crate::{
    util::{interaction::InteractionCommand, InteractionCommandExt},
    Context,
};

pub async fn cache(ctx: Arc<Context>, command: InteractionCommand) -> Result<()> {
    let mut stats = ctx.cache.stats();

    // TODO: different api in redlight?
    let guilds = stats
        .guilds()
        .await
        .wrap_err("Failed to fetch guilds count")?;

    let unavailable_guilds = stats
        .unavailable_guilds()
        .await
        .wrap_err("Failed to fetch unavailable_guilds count")?;

    let users = stats
        .users()
        .await
        .wrap_err("Failed to fetch users count")?;

    let roles = stats
        .roles()
        .await
        .wrap_err("Failed to fetch roles count")?;

    let channels = stats
        .channels()
        .await
        .wrap_err("Failed to fetch channels count")?;

    let description = format!(
        "Guilds: {guilds}\n\
        Unavailable guilds: {unavailable_guilds}\n\
        Users: {users}\n\
        Roles: {roles}\n\
        Channels: {channels}",
        guilds = WithComma::new(guilds),
        unavailable_guilds = WithComma::new(unavailable_guilds),
        users = WithComma::new(users),
        roles = WithComma::new(roles),
        channels = WithComma::new(channels),
    );

    let embed = EmbedBuilder::new()
        .description(description)
        .footer(FooterBuilder::new("Boot time"))
        .timestamp(ctx.stats.start_time);

    let builder = MessageBuilder::new().embed(embed);
    command.callback(&ctx, builder, false).await?;

    Ok(())
}
