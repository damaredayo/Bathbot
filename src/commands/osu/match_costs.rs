use crate::{
    arguments::{Args, MatchArgs},
    embeds::{EmbedData, MatchCostEmbed},
    util::{constants::OSU_API_ISSUE, MessageExt},
    BotResult, Context,
};

use futures::future::{try_join_all, TryFutureExt};
use itertools::Itertools;
use rosu::{
    model::{GameMods, Match, Team, TeamType},
    OsuError,
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::Write,
    sync::Arc,
};
use twilight_model::channel::Message;

const TOO_MANY_PLAYERS_TEXT: &str = "Too many players, cannot display message :(";

#[command]
#[short_desc("Display performance ratings for a multiplayer match")]
#[long_desc(
    "Calculate a performance rating for each player \
     in the given multiplayer match. The optional second \
     argument is the amount of played warmups, defaults to 2.\n\
     Here's the current [formula](https://i.imgur.com/7KFwcUS.png).\n\
     Keep in mind that all bots use different formulas so comparing \
     with values from other bots makes no sense."
)]
#[usage("[match url / match id] [amount of warmups]")]
#[example("58320988 1", "https://osu.ppy.sh/community/matches/58320988")]
#[aliases("mc", "matchcost")]
async fn matchcosts(ctx: Arc<Context>, msg: &Message, args: Args) -> BotResult<()> {
    let args = match MatchArgs::new(args) {
        Ok(args) => args,
        Err(err_msg) => return msg.error(&ctx, err_msg).await,
    };

    let match_id = args.match_id;
    let warmups = args.warmups;

    // Retrieve the match
    let osu_match = match ctx.osu().osu_match(match_id).await {
        Ok(mut osu_match) => {
            osu_match.games.retain(|game| !game.scores.is_empty());

            osu_match
        }
        Err(OsuError::InvalidMultiplayerMatch) => {
            let content = "Either the mp id was invalid or the match was private";

            return msg.error(&ctx, content).await;
        }
        Err(why) => {
            let _ = msg.error(&ctx, OSU_API_ISSUE).await;

            return Err(why.into());
        }
    };

    let mode = osu_match
        .games
        .first()
        .map(|game| game.mode)
        .unwrap_or_default();

    // Retrieve all users of the match
    let requests: Vec<_> = osu_match
        .games
        .iter()
        .map(|game| game.scores.iter())
        .flatten()
        .filter(|s| s.score > 0)
        .map(|s| s.user_id)
        .unique()
        .map(|id| ctx.osu().user(id).mode(mode).map_ok(move |user| (id, user)))
        .collect();

    // Prematurely abort if its too many players to display in a message
    if requests.len() > 50 {
        return msg.error(&ctx, TOO_MANY_PLAYERS_TEXT).await;
    }

    let users: HashMap<_, _> = match try_join_all(requests).await {
        Ok(users) => users
            .into_iter()
            .map(|(id, user)| {
                user.map_or_else(
                    || (id, id.to_string()),
                    |user| (user.user_id, user.username),
                )
            })
            .collect(),
        Err(why) => {
            let _ = msg.error(&ctx, OSU_API_ISSUE).await;

            return Err(why.into());
        }
    };

    // Process match
    let (description, match_result) = if osu_match.games.len() <= warmups {
        let mut description = String::from("No games played yet");

        if !osu_match.games.is_empty() && warmups > 0 {
            let _ = write!(
                description,
                " beyond the {} warmup{}",
                warmups,
                if warmups > 1 { "s" } else { "" }
            );
        }

        (Some(description), None)
    } else {
        let result = process_match(users.clone(), &osu_match, warmups);

        (None, Some(result))
    };

    // Accumulate all necessary data
    let data = match MatchCostEmbed::new(osu_match.clone(), description, match_result) {
        Ok(data) => data,
        Err(_) => return msg.error(&ctx, TOO_MANY_PLAYERS_TEXT).await,
    };

    // Creating the embed
    let embed = data.build_owned().build()?;

    msg.build_response(&ctx, |mut m| {
        if warmups > 0 {
            let mut content = String::from("Ignoring the first ");

            if warmups == 1 {
                content.push_str("map");
            } else {
                let _ = write!(content, "{} maps", warmups);
            }

            content.push_str(" as warmup:");
            m = m.content(content)?;
        }
        m.embed(embed)
    })
    .await?;

    Ok(())
}

macro_rules! sort {
    ($slice:expr) => {
        $slice.sort_unstable_by(|(.., a), (.., b)| b.partial_cmp(a).unwrap_or(Ordering::Equal));
    };
}

// flat additive bonus for each participated game
const FLAT_PARTICIPATION_BONUS: f32 = 0.5;

// exponent base, the higher - the higher is the difference
// between players who played a lot and players who played fewer
const BASE_PARTICIPATION_BONUS: f32 = 1.4;

// exponent, low: logithmically ~ high: linear
const EXP_PARTICIPATION_BONUS: f32 = 0.6;

// instead of considering tb score once, consider it this many times
const TIEBREAKER_BONUS: f32 = 2.0;

// global multiplier per combination (if at least 3)
const MOD_BONUS: f32 = 0.02;

fn process_match(
    mut users: HashMap<u32, String>,
    osu_match: &Match,
    warmups: usize,
) -> MatchResult {
    let games = &osu_match.games[warmups..];
    let mut teams = HashMap::new();
    let mut point_costs = HashMap::new();
    let mut mods = HashMap::new();
    let team_vs = games[0].team_type == TeamType::TeamVS;
    let mut match_scores = MatchScores(0, 0);

    // Calculate point scores for each score in each game
    for game in games.iter() {
        let score_sum: u32 = game.scores.iter().map(|s| s.score).sum();
        let avg = score_sum as f32 / game.scores.iter().filter(|s| s.score > 0).count() as f32;
        let mut team_scores = HashMap::with_capacity(team_vs as usize + 1);

        for score in game.scores.iter().filter(|s| s.score > 0) {
            mods.entry(score.user_id)
                .or_insert_with(HashSet::new)
                .insert(score.enabled_mods.map(|mods| mods - GameMods::NoFail));

            let point_cost = score.score as f32 / avg + FLAT_PARTICIPATION_BONUS;

            point_costs
                .entry(score.user_id)
                .or_insert_with(Vec::new)
                .push(point_cost);

            teams.entry(score.user_id).or_insert(score.team);

            team_scores
                .entry(score.team)
                .and_modify(|e| *e += score.score)
                .or_insert(score.score);
        }

        let (winner_team, _) = team_scores
            .into_iter()
            .max_by_key(|(_, score)| *score)
            .unwrap_or((Team::None, 0));

        match_scores.incr(winner_team);
    }

    // Tiebreaker bonus
    if osu_match.end_time.is_some() && games.len() > 2 && match_scores.difference() == 1 {
        let game = games.last().unwrap();

        point_costs
            .iter_mut()
            .filter(|(&user_id, _)| game.scores.iter().any(|score| score.user_id == user_id))
            .map(|(_, costs)| costs.last_mut().unwrap())
            .for_each(|value| *value = (*value * TIEBREAKER_BONUS) - FLAT_PARTICIPATION_BONUS);
    }

    // Mod combinations bonus
    let mods_count = mods
        .into_iter()
        .filter(|(_, mods)| mods.len() > 2)
        .map(|(id, mods)| (id, mods.len() - 2));

    for (user_id, count) in mods_count {
        let multiplier = 1.0 + count as f32 * MOD_BONUS;

        point_costs.entry(user_id).and_modify(|point_scores| {
            point_scores
                .iter_mut()
                .for_each(|point_score| *point_score *= multiplier);
        });
    }

    // Calculate match costs by combining point costs
    let mut data = HashMap::with_capacity(team_vs as usize + 1);
    let mut highest_cost = 0.0;
    let mut mvp_id = 0;

    for (user_id, point_costs) in point_costs {
        let name = match users.remove(&user_id) {
            Some(name) => name,
            None => {
                warn!("No user `{}` in matchcost users", user_id);

                continue;
            }
        };

        let sum: f32 = point_costs.iter().sum();
        let costs_len = point_costs.len() as f32;
        let mut match_cost = sum / costs_len;

        let exp = match games.len() {
            1 => 0.0,
            len => (costs_len - 1.0) / (len as f32 - 1.0),
        };

        match_cost *= BASE_PARTICIPATION_BONUS.powf(exp.powf(EXP_PARTICIPATION_BONUS));

        data.entry(*teams.get(&user_id).unwrap())
            .or_insert_with(Vec::new)
            .push((user_id, name, match_cost));

        if match_cost > highest_cost {
            highest_cost = match_cost;
            mvp_id = user_id;
        }
    }

    if team_vs {
        let blue = match data.remove(&Team::Blue) {
            Some(mut team) => {
                sort!(team);

                team
            }
            None => Vec::new(),
        };

        let red = match data.remove(&Team::Red) {
            Some(mut team) => {
                sort!(team);

                team
            }
            None => Vec::new(),
        };

        MatchResult::team(mvp_id, match_scores, blue, red)
    } else {
        let mut players = data.remove(&Team::None).unwrap_or_default();
        sort!(players);

        MatchResult::solo(mvp_id, players)
    }
}

type PlayerResult = (u32, String, f32);
type TeamResult = Vec<PlayerResult>;

pub enum MatchResult {
    TeamVS {
        blue: TeamResult,
        red: TeamResult,
        mvp: u32,
        match_scores: MatchScores,
    },
    HeadToHead {
        players: TeamResult,
        mvp: u32,
    },
}

impl MatchResult {
    #[inline]
    fn team(mvp: u32, match_scores: MatchScores, blue: TeamResult, red: TeamResult) -> Self {
        Self::TeamVS {
            mvp,
            match_scores,
            blue,
            red,
        }
    }

    #[inline]
    fn solo(mvp: u32, players: TeamResult) -> Self {
        Self::HeadToHead { mvp, players }
    }

    #[inline]
    pub fn mvp_id(&self) -> u32 {
        match self {
            MatchResult::TeamVS { mvp, .. } => *mvp,
            MatchResult::HeadToHead { mvp, .. } => *mvp,
        }
    }
}

#[derive(Copy, Clone)]
pub struct MatchScores(u8, u8);

impl MatchScores {
    #[inline]
    fn incr(&mut self, team: Team) {
        match team {
            Team::Blue => self.0 = self.0.saturating_add(1),
            Team::Red => self.1 = self.1.saturating_add(1),
            Team::None => {}
        }
    }

    #[inline]
    pub fn blue(self) -> u8 {
        self.0
    }

    #[inline]
    pub fn red(self) -> u8 {
        self.1
    }

    #[inline]
    fn difference(&self) -> u8 {
        let min = self.0.min(self.1);
        let max = self.0.max(self.1);

        max - min
    }
}
