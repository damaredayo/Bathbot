use std::future::ready;
use std::{fmt::Write, sync::Arc};

use bathbot_cache::Cache;
use bathbot_macros::command;
use bathbot_psql::model::configs::{Authorities, GuildConfig};
use bathbot_util::{constants::GENERAL_ISSUE, matcher, MessageBuilder};
use eyre::Report;
use eyre::Result;
use futures::TryStreamExt;
use twilight_model::id::marker::{GuildMarker, UserMarker};
use twilight_model::{
    guild::Permissions,
    id::{marker::RoleMarker, Id},
};

use crate::{
    core::{
        commands::{prefix::Args, CommandOrigin},
        BotConfig, Context,
    },
    util::ChannelExt,
};

#[command]
#[desc("Adjust authority roles for a server")]
#[help(
    "Decide which roles should be considered authority roles. \n\
    Authority roles enable the usage of certain commands like \
    `addstream` or `prune`.\n\
    Roles can be given as mention or as role id (up to 10 roles possible).\n\
    If you want to see the current authority roles, just pass \
    `-show` as argument"
)]
#[usage("[@role1] [id of role2] ...")]
#[example("-show", "@Moderator @Mod 83794728403223 @BotCommander")]
#[alias("authority")]
#[flags(AUTHORITY, ONLY_GUILDS, SKIP_DEFER)]
#[group(Utility)]
async fn prefix_authorities(ctx: Arc<Context>, msg: &Message, mut args: Args<'_>) -> Result<()> {
    match AuthorityCommandKind::args(&mut args) {
        Ok(args) => authorities(ctx, msg.into(), args).await,
        Err(content) => {
            msg.error(&ctx, content).await?;

            Ok(())
        }
    }
}

pub async fn authorities(
    ctx: Arc<Context>,
    orig: CommandOrigin<'_>,
    args: AuthorityCommandKind,
) -> Result<()> {
    let guild_id = orig.guild_id().unwrap();

    let mut content = match args {
        AuthorityCommandKind::Add(role_id) => {
            let roles = ctx
                .guild_config()
                .peek(guild_id, |config| config.authorities.clone())
                .await;

            if roles.len() >= 10 {
                let content = "You can have at most 10 roles per server setup as authorities.";

                return orig.error_callback(&ctx, content).await;
            }

            let f = |config: &mut GuildConfig| {
                if !config.authorities.contains(&role_id) {
                    config.authorities.push(role_id);
                }
            };

            if let Err(err) = ctx.guild_config().update(guild_id, f).await {
                let _ = orig.error_callback(&ctx, GENERAL_ISSUE).await;

                return Err(err.wrap_err("failed to update guild config"));
            }

            "Successfully added authority role. Authority roles now are: ".to_owned()
        }
        AuthorityCommandKind::List => "Current authority roles for this server: ".to_owned(),
        AuthorityCommandKind::Remove(role_id) => {
            let author_id = orig.user_id()?;
            let auth_roles = ctx
                .guild_config()
                .peek(guild_id, |config| config.authorities.clone())
                .await;

            if auth_roles.iter().all(|&id| id != role_id) {
                let content = "The role was no authority role anyway";
                let builder = MessageBuilder::new().embed(content);
                orig.callback(&ctx, builder).await?;

                return Ok(());
            }

            // Make sure the author is still an authority after applying new roles
            if !(author_id == BotConfig::get().owner
                || ctx
                    .cache
                    .guild(guild_id)
                    .await?
                    .map_or(false, |guild| guild.owner_id == author_id))
            {
                let is_still_auth_fut =
                    is_still_authority(&ctx.cache, guild_id, author_id, &auth_roles, Some(role_id));

                let still_authority: bool = match is_still_auth_fut.await {
                    Ok(still_authority) => still_authority,
                    Err(err) => {
                        let _ = orig.error_callback(&ctx, GENERAL_ISSUE).await;

                        return Err(err);
                    }
                };

                if !still_authority {
                    let content = "You cannot set authority roles to something \
                                that would make you lose authority status.";

                    return orig.error_callback(&ctx, content).await;
                }
            }

            let f = |config: &mut GuildConfig| config.authorities.retain(|id| *id != role_id);

            if let Err(err) = ctx.guild_config().update(guild_id, f).await {
                let _ = orig.error_callback(&ctx, GENERAL_ISSUE).await;

                return Err(err.wrap_err("failed to update guild config"));
            }

            "Successfully removed authority role. Authority roles now are: ".to_owned()
        }
        AuthorityCommandKind::Replace(auth_roles) => {
            let author_id = orig.user_id()?;

            // Make sure the author is still an authority after applying new roles
            if !(author_id == BotConfig::get().owner
                || ctx
                    .cache
                    .guild(guild_id)
                    .await?
                    .map_or(false, |guild| guild.owner_id == author_id))
            {
                let is_still_auth_fut =
                    is_still_authority(&ctx.cache, guild_id, author_id, &auth_roles, None);

                let still_authority: bool = match is_still_auth_fut.await {
                    Ok(still_authority) => still_authority,
                    Err(err) => {
                        let _ = orig.error_callback(&ctx, GENERAL_ISSUE).await;

                        return Err(err);
                    }
                };

                if !still_authority {
                    let content = "You cannot set authority roles to something \
                        that would make you lose authority status.";

                    return orig.error_callback(&ctx, content).await;
                }
            }

            let f =
                |config: &mut GuildConfig| config.authorities = auth_roles.into_iter().collect();

            if let Err(err) = ctx.guild_config().update(guild_id, f).await {
                let _ = orig.error_callback(&ctx, GENERAL_ISSUE).await;

                return Err(err.wrap_err("failed to update guild config"));
            }

            "Successfully changed the authority roles to: ".to_owned()
        }
    };

    // Send the message
    let roles = ctx
        .guild_config()
        .peek(guild_id, |config| config.authorities.clone())
        .await;
    role_string(&roles, &mut content);
    let builder = MessageBuilder::new().embed(content);
    orig.callback(&ctx, builder).await?;

    Ok(())
}

fn role_string(roles: &Authorities, content: &mut String) {
    let mut iter = roles.iter();

    if let Some(first) = iter.next() {
        content.reserve(roles.len() * 20);
        let _ = write!(content, "<@&{first}>");

        for role in iter {
            let _ = write!(content, ", <@&{role}>");
        }
    } else {
        content.push_str("None");
    }
}

pub enum AuthorityCommandKind {
    Add(Id<RoleMarker>),
    List,
    Remove(Id<RoleMarker>),
    Replace(Vec<Id<RoleMarker>>),
}

fn parse_role(arg: &str) -> Result<Id<RoleMarker>, String> {
    matcher::get_mention_role(arg)
        .ok_or_else(|| format!("Expected role mention or role id, got `{arg}`"))
}

impl AuthorityCommandKind {
    fn args(args: &mut Args<'_>) -> Result<Self, String> {
        let mut roles = match args.next() {
            Some("-show") | Some("show") => return Ok(Self::List),
            Some(arg) => vec![parse_role(arg)?],
            None => return Ok(Self::Replace(Vec::new())),
        };

        for arg in args.take(9) {
            roles.push(parse_role(arg)?);
        }

        Ok(Self::Replace(roles))
    }
}

async fn is_still_authority(
    cache: &Cache,
    guild_id: Id<GuildMarker>,
    member_id: Id<UserMarker>,
    auth_roles: &[Id<RoleMarker>],
    removed_role: Option<Id<RoleMarker>>,
) -> Result<bool> {
    let member_roles = match cache.member(guild_id, member_id).await? {
        Some(member) => member.roles.to_vec(),
        None => Vec::new(),
    };

    cache
        .iter()
        .guild_roles(guild_id)
        .await?
        .try_filter(|role| ready(member_roles.contains(&role.id)))
        .try_fold(false, |mut still_auth, role| {
            still_auth |= Permissions::from_bits_truncate(role.permissions)
                .contains(Permissions::ADMINISTRATOR)
                || auth_roles.iter().any(|&new| {
                    new == role.id && removed_role.map_or(true, |removed| new != removed)
                });

            ready(Ok(still_auth))
        })
        .await
        .map_err(Report::new)
}
