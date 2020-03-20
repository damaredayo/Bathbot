use crate::util::{discord, Matrix};

use rand::RngCore;
use serenity::{
    framework::standard::{macros::command, Args, CommandResult},
    model::prelude::Message,
    prelude::Context,
};

#[command]
#[description = "Play a game of minesweeper"]
#[aliases("ms")]
#[usage = "[Easy/Medium/Hard]"]
fn minesweeper(ctx: &mut Context, msg: &Message, mut args: Args) -> CommandResult {
    let game = if let Ok(difficulty) = args.trimmed().single_quoted::<String>() {
        match difficulty.to_lowercase().as_str() {
            "easy" => Difficulty::Easy.create(),
            "medium" => Difficulty::Medium.create(),
            "hard" => Difficulty::Hard.create(),
            _ => {
                let response = msg.channel_id.say(
                    &ctx.http,
                    "The argument must be either `Easy`, `Medium`, or `Hard`",
                )?;
                discord::save_response_owner(response.id, msg.author.id, ctx.data.clone());
                return Ok(());
            }
        }
    } else {
        Difficulty::Easy.create()
    };
    let w = game.width();
    let h = game.height();
    let mut field = String::with_capacity(w * h * 9);
    for x in 0..w {
        for y in 0..h {
            field.push_str(&format!("||:{}:||", game.field[(x, y)].text()));
        }
        field.push('\n');
    }
    field.pop();
    let response = msg.channel_id.say(
        &ctx.http,
        format!(
            "Here's a {}x{} game with {} mines:\n{}",
            game.width(),
            game.height(),
            game.mines,
            field
        ),
    )?;
    // Save the response owner
    discord::save_response_owner(response.id, msg.author.id, ctx.data.clone());
    Ok(())
}

enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    fn create(&self) -> Minesweeper {
        match self {
            Difficulty::Easy => Minesweeper::new(6, 6, 6),
            Difficulty::Medium => Minesweeper::new(8, 8, 12),
            Difficulty::Hard => Minesweeper::new(10, 10, 20),
        }
    }
}

struct Minesweeper {
    pub field: Matrix<Cell>,
    pub mines: u8,
}

impl Minesweeper {
    fn new(w: usize, h: usize, mines: u8) -> Self {
        let mut field = Matrix::new(w, h);
        let mut rng = rand::thread_rng();
        let size = w * h;
        let mut new_mines = mines;
        // Place mines
        while new_mines > 0 {
            let r = rng.next_u32() as usize % size;
            let x = r % w;
            let y = r / h;
            if field[(x, y)] == Cell::None {
                field[(x, y)] = Cell::Mine;
                new_mines -= 1;
            }
        }
        // Place numbers
        for x in 0..w {
            for y in 0..h {
                if field[(x, y)] == Cell::None {
                    let mines = field.count_neighbors(x, y, Cell::Mine);
                    field[(x, y)] = Cell::Num(mines);
                }
            }
        }
        Self { field, mines }
    }

    fn width(&self) -> usize {
        self.field.width()
    }

    fn height(&self) -> usize {
        self.field.height()
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Cell {
    Num(u8),
    Mine,
    None,
}

impl Cell {
    fn text(&self) -> &str {
        use Cell::{Mine, Num};
        match self {
            Num(0) => "zero",
            Num(1) => "one",
            Num(2) => "two",
            Num(3) => "three",
            Num(4) => "four",
            Num(5) => "five",
            Num(6) => "six",
            Num(7) => "seven",
            Num(8) => "eight",
            Mine => "bomb",
            _ => unreachable!(),
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::None
    }
}
