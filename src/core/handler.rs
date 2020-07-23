use crate::{
    bail,
    commands::help::{failed_help, help, help_command},
    core::{Command, CommandGroups, Context},
    util::MessageExt,
    BotResult, Error,
};

use std::{
    borrow::Cow,
    fmt::Write,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use twilight::gateway::Event;
use twilight::model::{channel::Message, id::RoleId};
use uwl::Stream;

pub async fn handle_event(
    shard_id: u64,
    event: Event,
    ctx: Arc<Context>,
    cmds: Arc<CommandGroups>,
) -> BotResult<()> {
    match event {
        // ####################
        // ## Gateway status ##
        // ####################
        Event::ShardReconnecting(_) => info!("Shard {} is attempting to reconnect", shard_id),
        Event::ShardResuming(_) => info!("Shard {} is resuming", shard_id),
        Event::Ready(_) => info!("Shard {} ready to go!", shard_id),
        Event::Resumed => info!("Shard {} successfully resumed", shard_id),
        Event::GatewayReconnect => info!("Gateway requested shard {} to reconnect", shard_id),
        Event::GatewayInvalidateSession(recon) => {
            if recon {
                warn!(
                    "Gateway has invalidated session for shard {}, but its reconnectable",
                    shard_id
                );
            } else {
                return Err(Error::InvalidSession(shard_id));
            }
        }
        Event::GatewayHello(u) => {
            debug!("Registered with gateway {} on shard {}", u, shard_id);
        }

        // ##############
        // ## Reaction ##
        // ##############
        Event::ReactionAdd(reaction_add) => {
            let reaction = reaction_add.0;
            if let Some(guild_id) = reaction.guild_id {
                let key = (reaction.channel_id.0, reaction.message_id.0);
                if let Some(guard) = ctx.role_assigns.get(&key) {
                    let role_id = RoleId(*guard.value());
                    if let Err(why) = ctx.http.add_role(guild_id, reaction.user_id, role_id).await {
                        error!("Error while assigning react-role to user: {}", why);
                    }
                }
            }
        }

        Event::ReactionRemove(reaction_remove) => {
            let reaction = reaction_remove.0;
            if let Some(guild_id) = reaction.guild_id {
                let key = (reaction.channel_id.0, reaction.message_id.0);
                if let Some(guard) = ctx.role_assigns.get(&key) {
                    let role_id = RoleId(*guard.value());
                    if let Err(why) = ctx
                        .http
                        .remove_guild_member_role(guild_id, reaction.user_id, role_id)
                        .await
                    {
                        error!("Error while removing react-role from user: {}", why);
                    }
                }
            }
        }

        // #############
        // ## Message ##
        // #############
        Event::MessageCreate(mut msg) => {
            ctx.cache.stats.new_message(&ctx, msg.deref());
            if msg.author.bot || msg.webhook_id.is_some() {
                return Ok(());
            }
            let prefixes = match msg.guild_id {
                Some(guild) => {
                    if !ctx.guilds.contains_key(&guild) {
                        let config = ctx.clients.psql.insert_guild(guild.0).await?;
                        ctx.guilds.insert(guild, config);
                    }
                    ctx.guilds.get(&guild).unwrap().prefixes.clone()
                }
                None => vec!["<".to_owned(), "!!".to_owned()],
            };

            let (invoke, content) = {
                let mut stream = Stream::new(&msg.content);
                stream.take_while_char(|c| c.is_whitespace());
                if !(find_prefix(&prefixes, &mut stream) || msg.guild_id.is_none()) {
                    return Ok(());
                }
                stream.take_while_char(|c| c.is_whitespace());
                let invoke = parse_invoke(&mut stream, &cmds);
                let content = stream.rest().to_owned();
                (invoke, content)
            };
            let msg = msg.deref_mut();
            msg.content = content;
            let command_result = match &invoke {
                Invoke::Command(cmd) => process_command(cmd, ctx.clone(), msg).await,
                Invoke::SubCommand { sub, .. } => process_command(sub, ctx.clone(), msg).await,
                Invoke::Help(None) => help(&ctx, &cmds, msg).await,
                Invoke::Help(Some(cmd)) => help_command(&ctx, cmd, msg).await,
                Invoke::FailedHelp(arg) => failed_help(&ctx, arg, &cmds, msg).await,
                Invoke::UnrecognisedCommand(_name) => Ok(()),
            };
            let name = invoke.name();
            match invoke {
                Invoke::UnrecognisedCommand(_) => {}
                _ => {
                    ctx.cache.stats.inc_command(name.as_ref());
                    match command_result {
                        Ok(_) => info!("Processed command `{}`", name),
                        Err(why) => error!("Error while processing command `{}`: {}", name, why),
                    }
                }
            }
        }
        _ => (),
    }
    Ok(())
}

async fn process_command(cmd: &Command, ctx: Arc<Context>, msg: &Message) -> BotResult<()> {
    // Only in guilds?
    if (cmd.authority || cmd.only_guilds) && msg.guild_id.is_none() {
        msg.respond(&ctx, "That command is only available in guilds")
            .await?;
        return Ok(());
    }
    // Ratelimited?
    if let Some(bucket) = cmd.bucket {
        if let Some(cooldown) = check_ratelimit(&ctx, msg, bucket).await {
            debug!(
                "Ratelimiting user {} on command `{}` for {} seconds",
                msg.author.id, cmd.names[0], cooldown,
            );
            let content = format!("Command on cooldown, try again in {} seconds", cooldown);
            msg.respond(&ctx, content).await?;
            return Ok(());
        }
    }
    // Only for authorities?
    if cmd.authority {
        match check_authority(&ctx, msg) {
            Ok(None) => {}
            Ok(Some(content)) => {
                debug!(
                    "Non-authority user {} tried using command `{}`",
                    msg.author.id, cmd.names[0]
                );
                msg.respond(&ctx, content).await?;
                return Ok(());
            }
            Err(why) => {
                msg.respond(&ctx, "Error while checking authority status")
                    .await?;
                return Err(why);
            }
        }
    }
    // Call command function
    (cmd.fun)(ctx, msg).await
}

// Is authority -> Ok(None)
// No authority -> Ok(Some(message to user))
// Couldn't figure out -> Err()
fn check_authority(ctx: &Context, msg: &Message) -> BotResult<Option<String>> {
    let guild_id = msg.guild_id.unwrap();
    if let Some(true) = ctx.cache.has_admin_permission(msg.author.id, guild_id) {
        return Ok(None);
    }
    if let Some(guard) = ctx.guilds.get(&guild_id) {
        let config = guard.value();
        let auth_roles: Vec<_> = config.authorities.iter().map(|id| RoleId(*id)).collect();
        if auth_roles.is_empty() {
            let prefix = &config.prefixes[0];
            let content = format!(
                "You need admin permissions to use this command.\n\
                    (`{}help authorities` to adjust authority status for this guild)",
                prefix
            );
            return Ok(Some(content));
        } else if let Some(member) = ctx.cache.get_member(msg.author.id, guild_id) {
            if !member.roles.iter().any(|role| auth_roles.contains(role)) {
                let mut roles = Vec::with_capacity(auth_roles.len());
                for role in auth_roles {
                    match ctx.cache.get_role(role, guild_id) {
                        Some(role) => roles.push(role.name.clone()),
                        None => warn!("Role {} not cached for guild {}", role, guild_id),
                    }
                }
                let role_len: usize = roles.iter().map(|role| role.len()).sum();
                let mut content = String::from(
                    "You need either admin permissions or \
                    any of these roles to use this command:\n",
                );
                content.reserve_exact(role_len + (roles.len() - 1) * 2);
                let mut roles = roles.into_iter();
                content.push_str(&roles.next().unwrap());
                for role in roles {
                    let _ = write!(content, ", {}", role);
                }
                let prefix = &config.prefixes[0];
                let _ = write!(
                    content,
                    "\n(`{}help authorities` to adjust authority status for this guild)",
                    prefix
                );
                return Ok(Some(content));
            }
        } else {
            bail!("Member {} not cached for guild {}", msg.author.id, guild_id);
        }
    } else {
        bail!("Guild {} not in cache", guild_id);
    }
    Ok(None)
}

async fn check_ratelimit(ctx: &Context, msg: &Message, bucket: &str) -> Option<i64> {
    let rate_limit = {
        let guard = ctx.buckets.get(bucket).unwrap();
        let mutex = guard.value();
        let mut bucket = mutex.lock().await;
        bucket.take(msg.author.id.0)
    };
    if rate_limit > 0 {
        return Some(rate_limit);
    }
    None
}

pub fn find_prefix<'a>(prefixes: &[String], stream: &mut Stream<'a>) -> bool {
    let prefix = prefixes.iter().find_map(|p| {
        let peeked = stream.peek_for_char(p.chars().count());
        if p == peeked {
            Some(peeked)
        } else {
            None
        }
    });
    if let Some(prefix) = &prefix {
        stream.increment(prefix.chars().count());
    }
    prefix.is_some()
}

fn parse_invoke(stream: &mut Stream<'_>, groups: &CommandGroups) -> Invoke {
    let name = stream.peek_until_char(|c| c.is_whitespace()).to_lowercase();
    stream.increment(name.chars().count());
    stream.take_while_char(|c| c.is_whitespace());
    match name.as_str() {
        "h" | "help" => {
            let name = stream.peek_until_char(|c| c.is_whitespace()).to_lowercase();
            stream.increment(name.chars().count());
            stream.take_while_char(|c| c.is_whitespace());
            if name.is_empty() {
                Invoke::Help(None)
            } else if let Some(cmd) = groups.get(name.as_str()) {
                Invoke::Help(Some(cmd))
            } else {
                Invoke::FailedHelp(name)
            }
        }
        _ => {
            if let Some(cmd) = groups.get(name.as_str()) {
                let name = stream.peek_until_char(|c| c.is_whitespace()).to_lowercase();
                for sub_cmd in cmd.sub_commands {
                    if sub_cmd.names.contains(&name.as_str()) {
                        stream.increment(name.chars().count());
                        stream.take_while_char(|c| c.is_whitespace());
                        // TODO: Check permissions & co
                        // check_discrepancy(ctx, msg, config, &cmd.options)?;
                        return Invoke::SubCommand {
                            main: cmd,
                            sub: sub_cmd,
                        };
                    }
                }
                // TODO: Check permissions & co
                // check_discrepancy(ctx, msg, config, &cmd.options)?;
                Invoke::Command(cmd)
            } else {
                Invoke::UnrecognisedCommand(name)
            }
        }
    }
}

#[derive(Debug)]
pub enum Invoke {
    Command(&'static Command),
    SubCommand {
        main: &'static Command,
        sub: &'static Command,
    },
    Help(Option<&'static Command>),
    FailedHelp(String),
    UnrecognisedCommand(String),
}

impl Invoke {
    fn name(&self) -> Cow<str> {
        match self {
            Invoke::Command(cmd) => Cow::Borrowed(cmd.names[0]),
            Invoke::SubCommand { main, sub } => {
                Cow::Owned(format!("{}-{}", main.names[0], sub.names[0]))
            }
            Invoke::Help(_) | Invoke::FailedHelp(_) => Cow::Borrowed("help"),
            Invoke::UnrecognisedCommand(arg) => Cow::Borrowed(arg),
        }
    }
}
