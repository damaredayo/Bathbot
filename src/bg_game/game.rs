use super::{util, GameResult, Hints, ImageReveal};
use crate::{
    database::MapsetTagWrapper,
    unwind_error,
    util::{constants::OSU_BASE, error::BgGameError},
    BotResult, Context, CONFIG,
};

use cow_utils::CowUtils;
use image::GenericImageView;
use rosu::model::GameMode;
use std::{collections::VecDeque, sync::Arc};
use tokio::{fs, sync::RwLock};
use tokio_stream::StreamExt;
use twilight_model::id::ChannelId;
use twilight_standby::WaitForMessageStream;

pub struct Game {
    pub title: String,
    pub artist: String,
    pub mapset_id: u32,
    hints: Arc<RwLock<Hints>>,
    reveal: Arc<RwLock<ImageReveal>>,
}

impl Game {
    pub async fn new(
        ctx: &Context,
        mapsets: &[MapsetTagWrapper],
        previous_ids: &mut VecDeque<u32>,
    ) -> (Self, Vec<u8>) {
        loop {
            match Game::_new(ctx, mapsets, previous_ids).await {
                Ok(game) => {
                    let sub_image_result = {
                        let reveal = game.reveal.read().await;
                        reveal.sub_image()
                    };
                    match sub_image_result {
                        Ok(img) => return (game, img),
                        Err(why) => unwind_error!(
                            warn,
                            why,
                            "Could not create initial bg image for id {}: {}",
                            game.mapset_id
                        ),
                    }
                }
                Err(why) => unwind_error!(warn, why, "Error creating bg game: {}"),
            }
        }
    }

    async fn _new(
        ctx: &Context,
        mapsets: &[MapsetTagWrapper],
        previous_ids: &mut VecDeque<u32>,
    ) -> GameResult<Self> {
        let mut path = CONFIG.get().unwrap().bg_path.clone();
        match mapsets[0].mode {
            GameMode::STD => path.push("osu"),
            GameMode::MNA => path.push("mania"),
            _ => return Err(BgGameError::Mode(mapsets[0].mode)),
        }
        let mapset = util::get_random_mapset(mapsets, previous_ids).await;
        debug!("Next BG mapset id: {}", mapset.mapset_id);
        let (title, artist) = util::get_title_artist(ctx, mapset.mapset_id).await?;
        let filename = format!("{}.{}", mapset.mapset_id, mapset.filetype);
        path.push(filename);
        let img_vec = fs::read(path).await?;
        let mut img = image::load_from_memory(&img_vec)?;
        let (w, h) = img.dimensions();
        // 800*600 (4:3)
        if w * h > 480_000 {
            img = img.thumbnail(800, 600);
        }
        Ok(Self {
            hints: Arc::new(RwLock::new(Hints::new(&title, mapset.tags))),
            title,
            artist,
            mapset_id: mapset.mapset_id,
            reveal: Arc::new(RwLock::new(ImageReveal::new(img))),
        })
    }

    #[inline]
    pub async fn sub_image(&self) -> GameResult<Vec<u8>> {
        let mut reveal = self.reveal.write().await;
        reveal.increase_radius();

        reveal.sub_image()
    }

    #[inline]
    pub async fn hint(&self) -> String {
        let mut hints = self.hints.write().await;

        hints.get(&self.title, &self.artist)
    }

    pub async fn resolve(
        &self,
        ctx: &Context,
        channel: ChannelId,
        content: String,
    ) -> BotResult<()> {
        let reveal_result = {
            let reveal = self.reveal.read().await;
            reveal.full()
        };

        match reveal_result {
            Ok(bytes) => {
                ctx.http
                    .create_message(channel)
                    .content(content)?
                    .attachment("bg_img.png", bytes)
                    .await?;
            }
            Err(why) => {
                unwind_error!(
                    warn,
                    why,
                    "Could not get full reveal of mapset id {}: {}",
                    self.mapset_id
                );
                ctx.http.create_message(channel).content(content)?.await?;
            }
        }

        Ok(())
    }

    async fn check_msg_content(&self, content: &str) -> ContentResult {
        // Guessed the title exactly?
        if content == self.title {
            return ContentResult::Title(true);
        }
        // Guessed sufficiently many words of the title?
        if self.title.contains(' ') {
            let mut same_word_len = 0;
            for title_word in self.title.split(' ') {
                for content_word in content.split(' ') {
                    if title_word == content_word {
                        same_word_len += title_word.len();
                        if same_word_len > 8 {
                            return ContentResult::Title(false);
                        }
                    }
                }
            }
        }
        // Similar enough to the title?
        let similarity = util::similarity(content, &self.title);
        if similarity > 0.5 {
            return ContentResult::Title(false);
        }
        if !self.hints.read().await.artist_guessed {
            // Guessed the artist exactly?
            if content == self.artist {
                return ContentResult::Artist(true);
            // Similar enough to the artist?
            } else if similarity < 0.3 && util::similarity(content, &self.artist) > 0.5 {
                return ContentResult::Artist(false);
            }
        }
        ContentResult::None
    }
}

#[derive(Clone, Copy)]
pub enum LoopResult {
    Winner(u64),
    Restart,
    Stop,
}

pub async fn game_loop(
    msg_stream: &mut WaitForMessageStream,
    ctx: &Context,
    game_lock: &RwLock<Option<Game>>,
    channel: ChannelId,
) -> LoopResult {
    // Collect and evaluate messages
    while let Some(msg) = msg_stream.next().await {
        let game = game_lock.read().await;
        if let Some(game) = game.as_ref() {
            let content = msg.content.cow_to_lowercase();
            match game.check_msg_content(content.as_ref()).await {
                // Title correct?
                ContentResult::Title(exact) => {
                    let content = format!(
                        "{} \\:)\nMapset: {}/beatmapsets/{}",
                        if exact {
                            format!("Gratz {}, you guessed it", msg.author.name)
                        } else {
                            format!("You were close enough {}, gratz", msg.author.name)
                        },
                        OSU_BASE,
                        game.mapset_id
                    );
                    // Send message
                    if let Err(why) = game.resolve(ctx, channel, content).await {
                        unwind_error!(warn, why, "Error while sending msg for winner: {}");
                    }
                    return LoopResult::Winner(msg.author.id.0);
                }
                // Artist correct?
                ContentResult::Artist(exact) => {
                    {
                        let mut hints = game.hints.write().await;
                        hints.artist_guessed = true;
                    }
                    let content = if exact {
                        format!(
                            "That's the correct artist `{}`, can you get the title too?",
                            msg.author.name
                        )
                    } else {
                        format!(
                            "`{}` got the artist almost correct, \
                            it's actually `{}` but can you get the title?",
                            msg.author.name, game.artist
                        )
                    };
                    // Send message
                    let msg_fut = ctx.http.create_message(channel).content(content).unwrap();
                    if let Err(why) = msg_fut.await {
                        unwind_error!(warn, why, "Error while sending msg for correct artist: {}");
                    }
                }
                ContentResult::None => {}
            }
        } else {
            return LoopResult::Stop;
        }
    }
    LoopResult::Stop
}

// bool to tell whether its an exact match
enum ContentResult {
    Title(bool),
    Artist(bool),
    None,
}
