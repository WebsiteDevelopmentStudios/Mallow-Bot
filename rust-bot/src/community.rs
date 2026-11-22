use serde_json::{json, Value};
use crate::config::GuildConfig;
use crate::premium::PremiumClient;
use serenity::{
    all::{CommandDataOptionValue, CommandInteraction, CommandOptionType, CreateCommand, CreateCommandOption, CreateInteractionResponse, CreateInteractionResponseMessage, Message},
    prelude::Context,
};
use std::{fs, path::PathBuf, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};
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

    pub fn add_xp(&self, guild: u64, user: u64, amount: i64, cooldown: u64) -> Option<(i64, i64)> {
        self.mutate(|data| {
            let key = format!("{guild}:{user}");
            let now = now_seconds();
            let last = data.get("last_xp").and_then(|v| v.get(&key)).and_then(Value::as_u64).unwrap_or(0);
            if now.saturating_sub(last) < cooldown { return None; }
            let old = data.get("xp").and_then(|v| v.get(&key)).and_then(Value::as_i64).unwrap_or(0);
            let new = old.saturating_add(amount.max(0));
            data["xp"][key.clone()] = json!(new);
            data["last_xp"][key] = json!(now);
            Some((old, new))
        })
    }

    pub fn add_swears(&self, guild: u64, user: u64, amount: i64) -> i64 {
        self.mutate(|data| {
            let key = format!("{guild}:{user}");
            let old = data.get("swears").and_then(|v| v.get(&key)).and_then(Value::as_i64).unwrap_or(0);
            let new = old.saturating_add(amount.max(0));
            data["swears"][key] = json!(new);
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

    pub fn set_record(&self, namespace: &str, key: &str, value: Option<Value>) {
        self.mutate(|data| {
            if let Some(value) = value { data[namespace][key] = value; }
            else if let Some(items) = data.get_mut(namespace).and_then(Value::as_object_mut) { items.remove(key); }
        });
    }

    pub fn record(&self, namespace: &str, key: &str) -> Option<Value> {
        self.read(|data| data.get(namespace).and_then(|v| v.get(key)).cloned())
    }

    pub fn set_autorole(&self, guild: u64, role: Option<u64>) {
        self.mutate(|data| {
            if let Some(role) = role { data["autoroles"][guild.to_string()] = json!(role); }
            else if let Some(values) = data.get_mut("autoroles").and_then(Value::as_object_mut) { values.remove(&guild.to_string()); }
        });
    }

    pub fn autorole(&self, guild: u64) -> Option<u64> {
        self.read(|data| data.get("autoroles").and_then(|v| v.get(guild.to_string())).and_then(Value::as_u64))
    }

    pub fn set_reaction_role(&self, guild: u64, message: u64, emoji: &str, role: Option<u64>) {
        self.mutate(|data| {
            let key = message.to_string();
            data["reaction_roles"][&key]["guild_id"] = json!(guild.to_string());
            if let Some(role) = role { data["reaction_roles"][&key]["roles"][emoji] = json!(role); }
            else if let Some(roles) = data["reaction_roles"][&key].get_mut("roles").and_then(Value::as_object_mut) { roles.remove(emoji); }
        });
    }

    pub fn set_rule(&self, guild: u64, message: u64, channel: u64, emoji: &str, role: u64, enabled: bool) {
        self.set_record("rules", &message.to_string(), Some(json!({
            "guild_id": guild.to_string(), "channel_id": channel.to_string(),
            "emoji": emoji, "role_id": role, "enabled": enabled
        })));
    }

    pub fn role_for_reaction(&self, guild: u64, message: u64, emoji: &str, added: bool) -> Option<u64> {
        self.read(|data| {
            if let Some(rule) = data.get("rules").and_then(|v| v.get(message.to_string())) {
                if rule.get("guild_id")?.as_str()? != guild.to_string() || rule.get("emoji")?.as_str()? != emoji { return None; }
                if added && rule.get("enabled").and_then(Value::as_bool) == Some(false) { return None; }
                return rule.get("role_id")?.as_u64();
            }
            let panel = data.get("reaction_roles")?.get(message.to_string())?;
            if panel.get("guild_id")?.as_str()? != guild.to_string() { return None; }
            panel.get("roles")?.get(emoji)?.as_u64()
        })
    }

    pub fn reaction_role(&self, guild: u64, message: u64, emoji: &str) -> Option<u64> {
        self.read(|data| {
            let panel = data.get("reaction_roles")?.get(message.to_string())?;
            if panel.get("guild_id")?.as_str()? != guild.to_string() { return None; }
            panel.get("roles")?.get(emoji)?.as_u64()
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
            "vanity" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "tagline", "New server tagline (leave blank to view)").required(false)),
            "sticky" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "message", "Sticky text (leave blank to clear)").required(false)),
            "embed" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "title", "Embed title").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "description", "Embed text").required(true)),
            "custom-command" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "action", "Create, list, or delete").required(true).add_string_choice("Create", "create").add_string_choice("List", "list").add_string_choice("Delete", "delete"))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "name", "Command name").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "response", "Command response").required(false)),
            "autorole" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "action", "Set, disable, or status").required(true).add_string_choice("Set", "set").add_string_choice("Disable", "disable").add_string_choice("Status", "status"))
                .add_option(CreateCommandOption::new(CommandOptionType::Role, "role", "Role to give new members").required(false)),
            "rules" => command
                .add_option(CreateCommandOption::new(CommandOptionType::Boolean, "enabled", "Enable or disable role verification").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "message_id", "Rules message ID").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "emoji", "Required reaction emoji").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::Role, "role", "Role granted by reacting").required(true)),
            "reaction-role" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "action", "Create, add, remove, or list").required(true).add_string_choice("Create", "create").add_string_choice("Add", "add").add_string_choice("Remove", "remove").add_string_choice("List", "list"))
                .add_option(CreateCommandOption::new(CommandOptionType::Channel, "channel", "Panel channel").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "message_id", "Panel message ID").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "emoji", "Reaction emoji").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::Role, "role", "Role to assign").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "title", "Panel title").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "description", "Panel description").required(false)),
            _ => command,
        }
    }).collect()
}

pub async fn handle_prefix(ctx: &Context, message: &Message, cfg: &GuildConfig, store: &CommunityStore) -> bool {
    let Some(guild_id) = message.guild_id.map(|id| id.get()) else { return false };
    let Some(rest) = message.content.strip_prefix(&cfg.prefix) else { return false };
    let mut parts = rest.trim().splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("").to_lowercase();
    let argument = parts.next().unwrap_or("").trim();
    let user_id = message.author.id.get();
    let starting = cfg.economy_config.get("starting_money").and_then(Value::as_i64).unwrap_or(100);
    let response = match command.as_str() {
        "xp" | "level" => {
            let xp = store.xp(guild_id, user_id);
            let lvl = level(xp);
            format!("Level {lvl} · {xp} XP · {} XP to next level.", ((lvl + 1) * (lvl + 1) * 100 - xp).max(0))
        }
        "leaderboard" => {
            let rows = store.leaderboard(guild_id);
            if rows.is_empty() { "No XP has been earned yet.".into() }
            else { rows.iter().enumerate().map(|(i, (id, xp))| format!("{}. <@{id}> — **{xp} XP** · Level {}", i + 1, level(*xp))).collect::<Vec<_>>().join("\\n") }
        }
        "balance" => format!("You have {} Mallow Coins.", store.balance(guild_id, user_id, starting)),
        "daily" => {
            if store.daily_claimed(guild_id, user_id, now_seconds()) { "You have already claimed your daily reward. Come back in 24 hours.".into() }
            else { let reward = cfg.economy_config.get("daily_reward").and_then(Value::as_i64).unwrap_or(100); let next = store.balance(guild_id, user_id, starting) + reward; store.set_balance(guild_id, user_id, next); format!("You claimed {reward} Mallow Coins. Balance: {next}.") }
        }
        "work" => {
            let min = cfg.economy_config.get("work_min").and_then(Value::as_i64).unwrap_or(20);
            let max = cfg.economy_config.get("work_max").and_then(Value::as_i64).unwrap_or(80).max(min);
            let earned = min + pseudo_roll((max - min + 1) as u64) as i64;
            let next = store.balance(guild_id, user_id, starting) + earned;
            store.set_balance(guild_id, user_id, next);
            format!("You worked a shift and earned {earned} Mallow Coins. Balance: {next}.")
        }
        "gamble" | "coinflip" => {
            let bet = argument.parse::<i64>().unwrap_or(10);
            let balance = store.balance(guild_id, user_id, starting);
            if bet < 1 || bet > balance { format!("Bet must be between 1 and your balance ({balance}).") }
            else { let won = pseudo_roll(2) == 0; let next = if won { balance + bet } else { balance - bet }; store.set_balance(guild_id, user_id, next); format!("{} Balance: {next} Mallow Coins.", if won { format!("You won {bet} Mallow Coins.") } else { format!("You lost {bet} Mallow Coins.") }) }
        }
        "fortune" => {
            let fortunes = ["A pleasant surprise is closer than you think.", "Your next idea will be your best one yet.", "Patience will pay off.", "A small win is still a win."];
            fortunes[pseudo_roll(fortunes.len() as u64) as usize].to_string()
        }
        "vibecheck" => format!("Vibe scan complete: {}% vibe.", pseudo_roll(101)),
        "ship" => format!("Compatibility scan: {}%. This result is entirely unscientific.", pseudo_roll(101)),
        "help" | "commands" => "Commands: xp, leaderboard, balance, daily, work, gamble, coinflip, fortune, vibecheck, ship.".into(),
        _ => return false,
    };
    if let Err(error) = message.reply(&ctx.http, response).await { warn!(%error, command, "Could not respond to prefix command"); }
    true
}

fn level(xp: i64) -> i64 { ((xp.max(0) as f64 / 100.0).sqrt()) as i64 }
fn now_seconds() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn pseudo_roll(max: u64) -> u64 {
    let t = now_seconds().wrapping_mul(6364136223846793005).wrapping_add(std::process::id() as u64);
    if max == 0 { 0 } else { (t ^ (t >> 17) ^ (t >> 31)) % max }
}

pub async fn handle(ctx: &Context, command: &CommandInteraction, store: &CommunityStore, premium: &PremiumClient) -> bool {
    let Some(guild_id) = command.guild_id.map(|id| id.get()) else { return false };
    let user_id = command.user.id.get();
    let name = command.data.name.as_str();
    let starting = 100_i64;
    if matches!(name, "embed" | "snipe" | "sticky" | "backup" | "custom-command" | "vanity") && user_id != 1021068313071210498 {
        let (banned, active, _) = premium.status(user_id).await;
        if banned {
            let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("This account cannot use Mallow Bot.").ephemeral(true))).await;
            return true;
        }
        if !active {
            let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("Mallow Premium is required. Get Premium at https://mallowbot.kz.ci/subscriptions.html").ephemeral(true))).await;
            return true;
        }
    }
    let response = match name {
        "say" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if !permitted { "You need Manage Messages permission to use this command.".to_string() }
            else {
                let text = command.data.options.iter().find_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).unwrap_or("");
                if text.trim().is_empty() { "Provide a message to send.".into() }
                else {
                    if let Err(error) = command.channel_id.say(&ctx.http, text.chars().take(1900).collect::<String>()).await { warn!(%error, "Could not send /say message"); }
                    "Message sent.".into()
                }
            }
        }
        "youtube" => {
            let url = command.data.options.iter().find_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).unwrap_or("");
            if !url.starts_with("https://www.youtube.com/") && !url.starts_with("https://youtu.be/") { "Provide a valid YouTube video URL.".into() }
            else {
                match reqwest::Client::new().get("https://www.youtube.com/oembed").query(&[("url", url), ("format", "json")]).send().await {
                    Ok(result) if result.status().is_success() => match result.json::<Value>().await {
                        Ok(info) => format!("**{}**\nChannel: {}\n{}", info.get("title").and_then(Value::as_str).unwrap_or("YouTube video"), info.get("author_name").and_then(Value::as_str).unwrap_or("Unknown"), url),
                        _ => "YouTube returned invalid video information.".into(),
                    },
                    _ => "I couldn't look up that YouTube video right now.".into(),
                }
            }
        }
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
        "premium-status" => {
            if user_id == 1021068313071210498 { "Mallow Premium: developer access is enabled.".into() }
            else { let (_, active, expires) = premium.status(user_id).await; if active { format!("Mallow Premium is active. Expires: {}.", expires.unwrap_or_else(|| "unknown".into())) } else { "Mallow Premium is not active for your Discord account. Get Premium at https://mallowbot.kz.ci/subscriptions.html".into() } }
        },
        "vanity" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
            if !permitted { "You need Manage Server permission to use this command.".into() }
            else {
                let tagline = command.data.options.iter().find_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).unwrap_or("").trim();
                if !tagline.is_empty() { store.set_record("vanity", &guild_id.to_string(), Some(json!(tagline.chars().take(160).collect::<String>()))); }
                let current = store.record("vanity", &guild_id.to_string()).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "No tagline set.".into());
                format!("Server tagline: {current}")
            }
        }
        "sticky" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if !permitted { "You need Manage Messages permission to use this command.".into() }
            else {
                let text = command.data.options.iter().find_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).unwrap_or("").trim();
                let key = command.channel_id.get().to_string();
                if text.is_empty() { store.set_record("sticky_messages", &key, None); "Sticky message cleared for this channel.".into() }
                else {
                    let content = text.chars().take(1800).collect::<String>();
                    match command.channel_id.send_message(&ctx.http, CreateMessage::new().content(&content)).await {
                        Ok(posted) => { store.set_record("sticky_messages", &key, Some(json!({"guild_id":guild_id,"content":content,"message_id":posted.id.get()}))); "Sticky message set.".into() }
                        Err(_) => "I couldn't post the sticky message. Check my channel permissions.".into()
                    }
                }
            }
        }
        "custom-command" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
            if !permitted { "You need Manage Server permission to use this command.".into() }
            else {
                let values: Vec<&str> = command.data.options.iter().filter_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).collect();
                let action = values.first().copied().unwrap_or("");
                let name = values.get(1).copied().unwrap_or("").to_lowercase();
                let key = format!("{guild_id}:{name}");
                match action {
                    "list" => "Custom commands are stored for this server.".into(),
                    "delete" => { store.set_record("custom_commands", &key, None); format!("Deleted custom command {name}.") }
                    "create" if !name.is_empty() && values.get(2).is_some() => {
                        store.set_record("custom_commands", &key, Some(json!(values[2].chars().take(1500).collect::<String>())));
                        format!("Created custom command {name}.")
                    }
                    _ => "Choose Create, List, or Delete. Create and Delete also require a command name.".into()
                }
            }
        }
        "embed" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if !permitted { "You need Manage Messages permission to use this command.".into() }
            else {
                let values: Vec<&str> = command.data.options.iter().filter_map(|option| match &option.value { CommandDataOptionValue::String(value) => Some(value.as_str()), _ => None }).collect();
                let title = values.first().copied().unwrap_or("Mallow Bot");
                let description = values.get(1).copied().unwrap_or("");
                let embed = CreateEmbed::new().title(title.chars().take(256).collect::<String>()).description(description.chars().take(4000).collect::<String>());
                match command.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
                    Ok(_) => "Embed sent.".into(),
                    Err(_) => "I couldn't send the embed. Check my channel permissions.".into()
                }
            }
        }
        "snipe" => {
            match store.record("snipes", &command.channel_id.get().to_string()) {
                Some(value) => format!("Recently deleted message: {}", value.get("content").and_then(Value::as_str).unwrap_or("[No text content]")),
                None => "There is no recently deleted message saved for this channel.".into()
            }
        }
        "backup" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.administrator()).unwrap_or(false);
            if !permitted { "You need Administrator permission to use this command.".into() }
            else {
                match command.guild_id.unwrap().channels(&ctx.http).await {
                    Ok(channels) => {
                        let payload = json!({
                            "guild_id": guild_id.to_string(),
                            "created_at": now_seconds(),
                            "channels": channels.values().map(|channel| json!({
                                "id": channel.id.get(),
                                "name": channel.name,
                                "type": format!("{:?}", channel.kind),
                                "position": channel.position,
                                "parent_id": channel.parent_id.map(|id| id.get()),
                                "topic": channel.topic
                            })).collect::<Vec<_>>()
                        });
                        match serde_json::to_vec_pretty(&payload) {
                            Ok(bytes) => {
                                let attachment = CreateAttachment::bytes(bytes, format!("mallow-backup-{guild_id}.json"));
                                if command.channel_id.send_message(&ctx.http, CreateMessage::new().content("Server channel-layout export attached. This is not a full permission or message restore backup.").add_file(attachment)).await.is_ok() {
                                    "Server layout export attached.".into()
                                } else { "I couldn't attach the server backup. Check my channel permissions.".into() }
                            }
                            Err(_) => "I couldn't serialize the server backup.".into()
                        }
                    }
                    Err(_) => "I couldn't read this server's channel layout. Check my permissions.".into()
                }
            }
        },
        _ => return false,
    };
    let builder = CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(response));
    if let Err(error) = command.create_response(&ctx.http, builder).await {
        warn!(%error, command = name, "Could not respond to community command");
    }
    true
}
