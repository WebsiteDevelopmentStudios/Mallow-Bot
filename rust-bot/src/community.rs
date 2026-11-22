use serde_json::{json, Value};
use serenity::{
    all::{CommandInteraction, CommandOptionType, CreateCommand, CreateCommandOption, CreateInteractionResponse, CreateInteractionResponseMessage},
    prelude::Context,
};
use std::{collections::HashMap, fs, path::PathBuf, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};
use tracing::warn;

#[derive(Clone)]
pub struct CommunityStore {
    path: Arc<PathBuf>,
    data: Arc<Mutex<Value>>,
}

impl CommunityStore {
    pub fn open() -> Self {
        let path = PathBuf::from(std::env::var("MALLOW_DATA_DIR").unwrap_or_else(|_| ".".into())).join("community_data.json");
        let data = fs::read_to_string(&path).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(Value::is_object).unwrap_or_else(|| json!({}));
        Self { path: Arc::new(path), data: Arc::new(Mutex::new(data)) }
    }

    fn mutate<T>(&self, f: impl FnOnce(&mut Value) -> T) -> T {
        let mut data = self.data.lock().expect("community data lock poisoned");
        let result = f(&mut data);
        if let Some(parent) = self.path.parent() { let _ = fs::create_dir_all(parent); }
        let temp = self.path.with_extension("tmp");
        if let Ok(raw) = serde_json::to_vec_pretty(&*data) {
            if fs::write(&temp, raw).is_ok() { let _ = fs::rename(&temp, &*self.path); }
        }
        result
    }

    fn read<T>(&self, f: impl FnOnce(&Value) -> T) -> T {
        let data = self.data.lock().expect("community data lock poisoned");
        f(&data)
    }

    pub fn add_xp(&self, guild: u64, user: u64, amount: i64) -> i64 {
        self.mutate(|data| {
            let key = format!("{guild}:{user}");
            let entry = data.pointer_mut(&format!("/xp/{key}"));
            let old = entry.and_then(|value| value.as_i64()).unwrap_or(0);
            let new = old.saturating_add(amount.max(0));
            data["xp"][key] = json!(new);
            new
        })
    }

    pub fn xp(&self, guild: u64, user: u64) -> i64 {
        self.read(|data| data.get("xp").and_then(|v| v.get(format!("{guild}:{user}"))).and_then(Value::as_i64).unwrap_or(0))
    }

    pub fn balance(&self, guild: u64, user: u64, starting: i64) -> i64 {
        self.read(|data| data.get("currency").and_then(|v| v.get(format!("{guild}:{user}"))).and_then(Value::as_i64).unwrap_or(starting))
    }

    pub fn set_balance(&self, guild: u64, user: u64, value: i64) {
        self.mutate(|data| { data["currency"][format!("{guild}:{user}")] = json!(value.max(0)); });
    }

    pub fn daily_claimed(&self, guild: u64, user: u64, now: u64) -> bool {
        self.mutate(|data| {
            let key = format!("{guild}:{user}");
            let previous = data.get("daily_claims").and_then(|v| v.get(&key)).and_then(Value::as_u64).unwrap_or(0);
            if now.saturating_sub(previous) < 86_400 { return true; }
            data["daily_claims"][key] = json!(now);
            false
        })
    }

    pub fn leaderboard(&self, guild: u64) -> Vec<(u64, i64)> {
        self.read(|data| {
            let prefix = format!("{guild}:");
            let mut values: Vec<_> = data.get("xp").and_then(Value::as_object).into_iter().flatten()
                .filter_map(|(key, value)| key.strip_prefix(&prefix).and_then(|id| id.parse::<u64>().ok())
                    .and_then(|id| value.as_i64().map(|xp| (id, xp)))).collect();
            values.sort_by(|a, b| b.1.cmp(&a.1));
            values.truncate(10);
            values
        })
    }
}

pub fn commands() -> Vec<CreateCommand> {
    [
        ("xp", "Show your XP and level"),
        ("leaderboard", "Show the server XP leaderboard"),
        ("balance", "Show your virtual Mallow Coins"),
        ("daily", "Claim your daily Mallow Coins"),
        ("work", "Work to earn virtual Mallow Coins"),
        ("gamble", "Bet virtual Mallow Coins on a coin flip"),
        ("coinflip", "Bet virtual Mallow Coins on a coin flip"),
        ("slots", "Play the virtual slot machine"),
        ("roulette", "Bet virtual Mallow Coins on even or odd roulette"),
        ("dice", "Roll dice and bet virtual coins"),
        ("blackjack", "Play a quick virtual blackjack round"),
        ("vibecheck", "Run a completely unscientific vibe scan"),
        ("ship", "Get a just-for-fun compatibility score"),
        ("fortune", "Draw a random Mallow Bot fortune"),
        ("wyr", "Get a Would You Rather question"),
        ("premium-status", "Check your Mallow Premium status"),
        ("youtube", "Look up a YouTube video link"),
        ("rules", "Configure reaction-based rules verification"),
        ("autorole", "Configure automatic member roles"),
        ("reaction-role", "Configure reaction role panels"),
        ("announcement", "Post a Bliss SMP announcement"),
        ("event", "Post a Bliss SMP event"),
        ("embed", "Create and send a custom embed (Premium)"),
        ("snipe", "Show the most recently deleted message (Premium)"),
        ("sticky", "Set or clear a sticky message (Premium)"),
        ("backup", "Export this server's roles and channel layout (Premium)"),
        ("custom-command", "Create, list, or remove a custom text command (Premium)"),
        ("vanity", "Set or view your server tagline (Premium)"),
        ("dm", "Send a direct message to a server member"),
        ("ticket", "Open a private support ticket"),
        ("ticket-panel", "Post a support ticket panel"),
        ("ticket-close", "Close the ticket channel"),
        ("purge", "Delete recent messages from this channel"),
        ("set-log", "Set the bot DM log channel (developer only)"),
    ].into_iter().map(|(name, description)| {
        let command = CreateCommand::new(name).description(description);
        match name {
            "youtube" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "url", "YouTube video URL").required(true)),
            "purge" => command.add_option(CreateCommandOption::new(CommandOptionType::Integer, "amount", "Number of messages to delete (1-100)").min_int_value(1).max_int_value(100).required(true)),
            "say" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "message", "Text to post").required(true)),
            "dm" => command
                .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Member to message").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "message", "Message to send").required(true)),
            "volume" => command.add_option(CreateCommandOption::new(CommandOptionType::Integer, "level", "Volume from 0 to 100").min_int_value(0).max_int_value(100).required(true)),
            _ => command,
        }
    }).collect()
}

fn level(xp: i64) -> i64 { ((xp.max(0) as f64 / 100.0).sqrt()) as i64 }
fn now_seconds() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn pseudo_roll(max: u64) -> u64 {
    let t = now_seconds().wrapping_mul(6364136223846793005).wrapping_add(std::process::id() as u64);
    if max == 0 { 0 } else { (t ^ (t >> 17) ^ (t >> 31)) % max }
}

pub async fn handle(ctx: &Context, command: &CommandInteraction, store: &CommunityStore) -> bool {
    let Some(guild_id) = command.guild_id.map(|id| id.get()) else { return false };
    let user_id = command.user.id.get();
    let name = command.data.name.as_str();
    let starting = 100_i64;
    let response = match name {
        "xp" => {
            let xp = store.xp(guild_id, user_id);
            let lvl = level(xp);
            format!("Level {lvl} · {xp} XP · {} XP to next level.", ((lvl + 1) * (lvl + 1) * 100 - xp).max(0))
        }
        "leaderboard" => {
            let rows = store.leaderboard(guild_id);
            if rows.is_empty() { "No XP has been earned yet.".into() }
            else { rows.iter().enumerate().map(|(i, (id, xp))| format!("{}. <@{id}> — **{xp} XP** · Level {}", i + 1, level(*xp))).collect::<Vec<_>>().join("\n") }
        }
        "balance" => format!("You have {} Mallow Coins.", store.balance(guild_id, user_id, starting)),
        "daily" => {
            if store.daily_claimed(guild_id, user_id, now_seconds()) { "You have already claimed your daily reward. Come back in 24 hours.".into() }
            else { let value = store.balance(guild_id, user_id, starting) + 100; store.set_balance(guild_id, user_id, value); format!("You claimed 100 Mallow Coins. Balance: {value}.") }
        }
        "work" => {
            let earned = 20 + pseudo_roll(61) as i64;
            let value = store.balance(guild_id, user_id, starting) + earned;
            store.set_balance(guild_id, user_id, value);
            format!("You worked a shift and earned {earned} Mallow Coins. Balance: {value}.")
        }
        "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" => {
            let bet = 10_i64;
            let balance = store.balance(guild_id, user_id, starting);
            if balance < bet { format!("You need at least {bet} Mallow Coins to play. Your balance is {balance}.") }
            else {
                let roll = pseudo_roll(100);
                let (payout, detail) = match name {
                    "slots" => { let a = pseudo_roll(5); let b = pseudo_roll(5); let c = pseudo_roll(5); let symbols = ["cherry", "lemon", "grape", "bell", "seven"]; let p = if a == b && b == c { bet * 5 } else if a == b || b == c || a == c { bet * 2 } else { 0 }; (p, format!("{} · {} · {}", symbols[a as usize], symbols[b as usize], symbols[c as usize])) }
                    "roulette" => { let n = pseudo_roll(37); (if n % 2 == 0 { bet * 2 } else { 0 }, format!("Roulette landed on {n}.")) }
                    "dice" => { let n = pseudo_roll(6) + 1; (if n >= 4 { bet * 2 } else { 0 }, format!("You rolled {n}.")) }
                    "blackjack" => { let p = 12 + pseudo_roll(10); let d = 15 + pseudo_roll(7); (if p > d { bet * 2 } else { 0 }, format!("You: {p} · Dealer: {d}.")) }
                    _ => (if roll >= 50 { bet * 2 } else { 0 }, if roll >= 50 { "Heads! You won.".into() } else { "Tails! You lost.".into() })
                };
                let next = balance - bet + payout;
                store.set_balance(guild_id, user_id, next);
                format!("{detail} {} Balance: {next} Mallow Coins.", if payout > 0 { format!("Won {payout} Mallow Coins.") } else { format!("Lost {bet} Mallow Coins.") })
            }
        }
        "vibecheck" => format!("Vibe scan complete: {}% vibe.", pseudo_roll(101)),
        "ship" => format!("Compatibility scan: {}%. This result is entirely unscientific.", pseudo_roll(101)),
        "fortune" => {
            let fortunes = ["A pleasant surprise is closer than you think.", "Your next idea will be your best one yet.", "Patience will pay off.", "A small win is still a win.", "Today is a good day to try something new."];
            fortunes[pseudo_roll(fortunes.len() as u64) as usize].to_string()
        }
        "wyr" => "Would you rather explore the deepest ocean or travel to the edge of space?".into(),
        "premium-status" => "Premium status is managed through the Mallow dashboard: https://mallowbot.kz.ci/subscriptions.html".into(),
        _ => return false,
    };
    let builder = CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(response));
    if let Err(error) = command.create_response(&ctx.http, builder).await {
        warn!(%error, command = name, "Could not respond to community command");
    }
    true
}
