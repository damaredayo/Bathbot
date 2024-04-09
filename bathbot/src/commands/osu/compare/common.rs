use std::{borrow::Cow, cmp::Ordering, collections::HashMap, fmt::Write, iter, sync::Arc};

use bathbot_macros::{command, SlashCommand};
use bathbot_model::rosu_v2::user::User;
use bathbot_util::{
    constants::{GENERAL_ISSUE, OSU_API_ISSUE},
    matcher, IntHasher,
};
use eyre::{Report, Result};
use rkyv::{Deserialize, Infallible};
use rosu_v2::{
    prelude::{GameMode, OsuError, Score, Username},
    request::UserId,
    OsuResult,
};
use time::OffsetDateTime;
use twilight_interactions::command::{CommandModel, CreateCommand};
use twilight_model::{
    guild::Permissions,
    id::{marker::UserMarker, Id},
};

use super::{CompareTop, AT_LEAST_ONE};
use crate::{
    active::{impls::CompareTopPagination, ActiveMessages},
    commands::{
        osu::{user_not_found, UserExtraction},
        GameModeOption,
    },
    core::commands::{prefix::Args, CommandOrigin},
    manager::redis::{osu::UserArgs, RedisData},
    util::{interaction::InteractionCommand, osu::get_combined_thumbnail, InteractionCommandExt},
    Context,
};

#[derive(CommandModel, CreateCommand, Default, SlashCommand)]
#[command(
    name = "ct",
    desc = "Compare common top scores",
    help = "Compare common top scores between players and see who did better on them"
)]
#[allow(unused)]
pub struct Ct<'a> {
    #[command(desc = "Specify a gamemode")]
    mode: Option<GameModeOption>,
    #[command(desc = "Specify a username")]
    name1: Option<Cow<'a, str>>,
    #[command(desc = "Specify a username")]
    name2: Option<Cow<'a, str>>,
    #[command(
        desc = "Specify a linked discord user",
        help = "Instead of specifying an osu! username with the `name1` option, \
        you can use this option to choose a discord user.\n\
        Only works on users who have used the `/link` command."
    )]
    discord1: Option<Id<UserMarker>>,
    #[command(desc = "Specify a linked discord user")]
    discord2: Option<Id<UserMarker>>,
}

async fn slash_ct(ctx: Arc<Context>, mut command: InteractionCommand) -> Result<()> {
    let args = CompareTop::from_interaction(command.input_data())?;

    top(ctx, (&mut command).into(), args).await
}

#[command]
#[desc("Compare maps of two players' top100s")]
#[help("Compare the two users' top 100 and check which maps appear in each top list.")]
#[usage("[name1] [name2]")]
#[example("badewanne3 \"nathan on osu\"")]
#[group(Osu)]
#[alias("comparetop")]
async fn prefix_common(
    ctx: Arc<Context>,
    msg: &Message,
    args: Args<'_>,
    permissions: Option<Permissions>,
) -> Result<()> {
    let args = CompareTop::args(None, args);

    top(ctx, CommandOrigin::from_msg(msg, permissions), args).await
}

#[command]
#[desc("Compare maps of two players' top100s")]
#[help("Compare the mania users' top 100 and check which maps appear in each top list")]
#[usage("[name1] [name2]")]
#[example("badewanne3 \"nathan on osu\"")]
#[alias("commonm", "comparetopmania")]
#[group(Mania)]
async fn prefix_commonmania(
    ctx: Arc<Context>,
    msg: &Message,
    args: Args<'_>,
    permissions: Option<Permissions>,
) -> Result<()> {
    let args = CompareTop::args(Some(GameModeOption::Mania), args);

    top(ctx, CommandOrigin::from_msg(msg, permissions), args).await
}

#[command]
#[desc("Compare maps of two players' top100s")]
#[help("Compare the taiko users' top 100 and check which maps appear in each top list")]
#[usage("[name1] [name2]")]
#[example("badewanne3 \"nathan on osu\"")]
#[alias("commont", "comparetoptaiko")]
#[group(Taiko)]
async fn prefix_commontaiko(
    ctx: Arc<Context>,
    msg: &Message,
    args: Args<'_>,
    permissions: Option<Permissions>,
) -> Result<()> {
    let args = CompareTop::args(Some(GameModeOption::Taiko), args);

    top(ctx, CommandOrigin::from_msg(msg, permissions), args).await
}

#[command]
#[desc("Compare maps of two players' top100s")]
#[help("Compare the ctb users' top 100 and check which maps appear in each top list")]
#[usage("[name1] [name2]")]
#[example("badewanne3 \"nathan on osu\"")]
#[alias("commonc", "commoncatch", "comparetopctb", "comparetopcatch")]
#[group(Catch)]
async fn prefix_commonctb(
    ctx: Arc<Context>,
    msg: &Message,
    args: Args<'_>,
    permissions: Option<Permissions>,
) -> Result<()> {
    let args = CompareTop::args(Some(GameModeOption::Catch), args);

    top(ctx, CommandOrigin::from_msg(msg, permissions), args).await
}

async fn extract_user_id(ctx: &Context, args: &mut CompareTop<'_>) -> UserExtraction {
    if let Some(name) = args.name1.take().or_else(|| args.name2.take()) {
        UserExtraction::Id(UserId::Name(name.as_ref().into()))
    } else if let Some(discord) = args.discord1.take().or_else(|| args.discord2.take()) {
        match ctx.user_config().osu_id(discord).await {
            Ok(Some(user_id)) => UserExtraction::Id(UserId::Id(user_id)),
            Ok(None) => {
                UserExtraction::Content(format!("<@{discord}> is not linked to an osu!profile"))
            }
            Err(err) => UserExtraction::Err(err),
        }
    } else {
        UserExtraction::None
    }
}

pub(super) async fn top(
    ctx: Arc<Context>,
    orig: CommandOrigin<'_>,
    mut args: CompareTop<'_>,
) -> Result<()> {
    let owner = orig.user_id()?;

    let user_id1 = match extract_user_id(&ctx, &mut args).await {
        UserExtraction::Id(user_id) => user_id,
        UserExtraction::Err(err) => {
            let _ = orig.error(&ctx, GENERAL_ISSUE).await;

            return Err(err);
        }
        UserExtraction::Content(content) => return orig.error(&ctx, content).await,
        UserExtraction::None => return orig.error(&ctx, AT_LEAST_ONE).await,
    };

    let user_id2 = match extract_user_id(&ctx, &mut args).await {
        UserExtraction::Id(user_id) => user_id,
        UserExtraction::Err(err) => {
            let _ = orig.error(&ctx, GENERAL_ISSUE).await;

            return Err(err);
        }
        UserExtraction::Content(content) => return orig.error(&ctx, content).await,
        UserExtraction::None => match ctx.user_config().osu_id(owner).await {
            Ok(Some(user_id)) => UserId::Id(user_id),
            Ok(None) => {
                let content =
                    "Since you're not linked with the `/link` command, you must specify two names.";

                return orig.error(&ctx, content).await;
            }
            Err(err) => {
                let _ = orig.error(&ctx, GENERAL_ISSUE).await;

                return Err(err);
            }
        },
    };

    if user_id1 == user_id2 {
        return orig.error(&ctx, "Give two different names").await;
    }

    let mode = match args.mode {
        Some(mode) => mode.into(),
        None => match ctx.user_config().mode(owner).await {
            Ok(mode) => mode.unwrap_or(GameMode::Osu),
            Err(err) => {
                let _ = orig.error(&ctx, GENERAL_ISSUE).await;

                return Err(err);
            }
        },
    };

    let fut1 = get_user_and_scores(&ctx, &user_id1, mode);
    let fut2 = get_user_and_scores(&ctx, &user_id2, mode);

    let (user1, scores1, user2, scores2) = match tokio::join!(fut1, fut2) {
        (Ok((user1, scores1)), Ok((user2, scores2))) => (user1, scores1, user2, scores2),
        (Err(OsuError::NotFound), _) => {
            let content = user_not_found(&ctx, user_id1).await;

            return orig.error(&ctx, content).await;
        }
        (_, Err(OsuError::NotFound)) => {
            let content = user_not_found(&ctx, user_id2).await;

            return orig.error(&ctx, content).await;
        }
        (Err(err), _) | (_, Err(err)) => {
            let _ = orig.error(&ctx, OSU_API_ISSUE).await;
            let err = Report::new(err).wrap_err("failed to get scores");

            return Err(err);
        }
    };

    let user1 = CommonUser::new(user1);
    let user2 = CommonUser::new(user2);

    let content = if scores1.is_empty() {
        Some(format!("No scores data for user `{}`", user1.name))
    } else if scores2.is_empty() {
        Some(format!("No scores data for user `{}`", user2.name))
    } else {
        None
    };

    if let Some(content) = content {
        return orig.error(&ctx, content).await;
    }

    // Check if different names that both belong to the same user were given
    if user1.id() == user2.id() {
        let content = "You must specify two different users";

        return orig.error(&ctx, content).await;
    }

    let indices: HashMap<_, _, IntHasher> = scores2
        .iter()
        .enumerate()
        .map(|(i, score)| (score.map_id, i))
        .collect();

    let mut wins = [0, 0];

    let maps: HashMap<_, _, IntHasher> = scores1
        .into_iter()
        .filter_map(|mut score1| {
            let map = score1.map.take()?;
            let mapset = score1.mapset.take()?;

            let score1 = CommonScore::from(&score1);

            let idx = indices.get(&map.map_id)?;
            let score2 = CommonScore::from(&scores2[*idx]);

            match score1.cmp(&score2) {
                Ordering::Less => wins[1] += 1,
                Ordering::Equal => {}
                Ordering::Greater => wins[0] += 1,
            }

            let map_id = map.map_id;

            let map = CompareTopMap {
                title: mapset.title.into_boxed_str(),
                version: map.version.into_boxed_str(),
            };

            Some((map_id, ([score1, score2], map)))
        })
        .collect();

    // Sort the maps by their score's avg pp values
    let mut map_pps: Box<[_]> = maps
        .iter()
        .map(|(map_id, ([a, b], _))| (*map_id, a.pp + b.pp))
        .collect();

    map_pps.sort_unstable_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(Ordering::Equal));

    // Accumulate all necessary data
    let mut content = format!("`{}` and `{}` ", user1.name, user2.name);

    let amount_common = maps.len();

    if amount_common == 0 {
        content.push_str("have no common scores");
    } else {
        let _ = write!(
            content,
            "have {amount_common} common beatmap{} in their top 100",
            if amount_common > 1 { "s" } else { "" }
        );
    }

    // Create the combined profile pictures
    let urls = iter::once(user1.avatar_url()).chain(iter::once(user2.avatar_url()));

    let thumbnail = match get_combined_thumbnail(&ctx, urls, 2, None).await {
        Ok(thumbnail) => Some(thumbnail),
        Err(err) => {
            warn!(?err, "Failed to combine avatars");

            None
        }
    };

    let pagination = CompareTopPagination::builder()
        .name1(user1.name)
        .name2(user2.name)
        .maps(maps)
        .map_pps(map_pps)
        .wins(wins)
        .msg_owner(owner)
        .build();

    ActiveMessages::builder(pagination)
        .start_by_update(true)
        .attachment(thumbnail.map(|bytes| ("avatar_fuse.png".to_owned(), bytes)))
        .begin(ctx, orig)
        .await
}

async fn get_user_and_scores(
    ctx: &Context,
    user_id: &UserId,
    mode: GameMode,
) -> OsuResult<(RedisData<User>, Vec<Score>)> {
    let args = UserArgs::rosu_id(ctx, user_id).await.mode(mode);

    ctx.osu_scores()
        .top(false)
        .limit(100)
        .exec_with_user(args)
        .await
}

#[derive(PartialEq)]
pub struct CommonScore {
    pub pp: f32,
    score: u32,
    ended_at: OffsetDateTime,
}

impl Eq for CommonScore {}

impl From<&Score> for CommonScore {
    #[inline]
    fn from(score: &Score) -> Self {
        Self {
            pp: score.pp.unwrap_or(0.0),
            score: score.score,
            ended_at: score.ended_at,
        }
    }
}

impl Ord for CommonScore {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.pp
            .partial_cmp(&other.pp)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.score.cmp(&other.score))
            .then_with(|| other.ended_at.cmp(&self.ended_at))
    }
}

impl PartialOrd for CommonScore {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct CommonUser {
    name: Username,
    avatar_url: Box<str>,
    user_id: u32,
    pub first_count: usize,
}

impl CommonUser {
    fn new(user: RedisData<User>) -> Self {
        match user {
            RedisData::Original(user) => Self {
                name: user.username,
                avatar_url: user.avatar_url,
                user_id: user.user_id,
                first_count: 0,
            },
            RedisData::Archive(user) => Self {
                name: user.username.as_str().into(),
                avatar_url: user.avatar_url.deserialize(&mut Infallible).unwrap(),
                user_id: user.user_id,
                first_count: 0,
            },
        }
    }
}

impl CommonUser {
    pub fn id(&self) -> u32 {
        self.user_id
    }

    fn avatar_url(&self) -> &str {
        self.avatar_url.as_ref()
    }
}

impl<'m> CompareTop<'m> {
    fn args(mode: Option<GameModeOption>, args: Args<'m>) -> Self {
        let mut args_ = CompareTop {
            mode,
            ..Default::default()
        };

        for arg in args.take(2) {
            if let Some(id) = matcher::get_mention_user(arg) {
                if args_.discord1.is_none() {
                    args_.discord1 = Some(id);
                } else {
                    args_.discord2 = Some(id);
                }
            } else if args_.name1.is_none() {
                args_.name1 = Some(arg.into());
            } else {
                args_.name2 = Some(arg.into());
            }
        }

        args_
    }
}

pub struct CompareTopMap {
    pub title: Box<str>,
    pub version: Box<str>,
}
