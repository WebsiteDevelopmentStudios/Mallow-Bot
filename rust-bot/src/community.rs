use serde_json::{json, Value};
use crate::config::GuildConfig;
use crate::premium::PremiumClient;
use crate::music::MusicManager;
use serenity::{
    all::{ChannelType, CommandDataOptionValue, CommandInteraction, CommandOptionType, CreateActionRow, CreateAttachment, CreateButton, CreateChannel, CreateCommand, CreateCommandOption, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse, CreateMessage, GetMessages, Message, PermissionOverwrite, PermissionOverwriteType, Permissions, ReactionType, RoleId, UserId, ButtonStyle},
    http::Http,
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

    pub fn first_log_channel(&self) -> Option<u64> {
        self.read(|data| data.get("log_channels").and_then(Value::as_object).and_then(|items| items.values().find_map(Value::as_u64)))
    }

    pub fn record(&self, namespace: &str, key: &str) -> Option<Value> {
        self.read(|data| data.get(namespace).and_then(|v| v.get(key)).cloned())
    }

    pub fn open_ticket(&self, guild: u64, user: u64) -> Option<u64> {
        self.read(|data| {
            data.get("tickets")?.as_object()?.iter().find_map(|(channel, ticket)| {
                let matches = ticket.get("guild_id").and_then(Value::as_u64) == Some(guild)
                    && ticket.get("user_id").and_then(Value::as_u64) == Some(user);
                if matches { channel.parse::<u64>().ok() } else { None }
            })
        })
    }

    pub fn timeout_minutes(&self, guild: u64) -> u64 {
        self.read(|data| data.get("timeout_minutes").and_then(|v| v.get(guild.to_string())).and_then(Value::as_u64).unwrap_or(10).clamp(1, 10_080))
    }

    pub fn set_timeout_minutes(&self, guild: u64, minutes: u64) {
        self.set_record("timeout_minutes", &guild.to_string(), Some(json!(minutes.clamp(1, 10_080))));
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

    pub fn swears(&self, guild: u64, user: u64) -> i64 {
        self.read(|data| data.get("swears").and_then(|v| v.get(format!("{guild}:{user}"))).and_then(Value::as_i64).unwrap_or(0))
    }

    pub fn swear_leaderboard(&self, guild: u64) -> Vec<(u64, i64)> {
        self.read(|data| {
            let prefix = format!("{guild}:");
            let mut values: Vec<_> = data.get("swears").and_then(Value::as_object).into_iter().flatten()
                .filter_map(|(key, value)| key.strip_prefix(&prefix).and_then(|id| id.parse::<u64>().ok())
                    .and_then(|id| value.as_i64().map(|count| (id, count)))).collect();
            values.sort_by(|a, b| b.1.cmp(&a.1));
            values.truncate(10);
            values
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
        ("pay", "Give virtual Mallow Coins to a member"),
        ("rob", "Try to steal virtual Mallow Coins from a member"),
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
        ("say", "Make the bot post a message in this channel"),
        ("update", "Post the bot update log in this channel"),
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
            "set-log" => command.add_option(CreateCommandOption::new(CommandOptionType::Channel, "channel", "Channel that receives incoming bot DMs").required(true)),
            "youtube" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "url", "YouTube video URL").required(true)),
            "purge" => command.add_option(CreateCommandOption::new(CommandOptionType::Integer, "amount", "Number of messages to delete (1-100)").min_int_value(1).max_int_value(100).required(true)),
            "say" => command.add_option(CreateCommandOption::new(CommandOptionType::String, "message", "Text to post").required(true)),
            "dm" => command
                .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Member to message").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "message", "Message to send").required(true)),
            "volume" => command.add_option(CreateCommandOption::new(CommandOptionType::Integer, "level", "Volume from 0 to 100").min_int_value(0).max_int_value(100).required(true)),
            "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" => command.add_option(CreateCommandOption::new(CommandOptionType::Integer, "bet", "Amount of virtual coins to wager").min_int_value(1).required(false)),
            "pay" => command
                .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Member to pay").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::Integer, "amount", "Amount of Mallow Coins").min_int_value(1).required(true)),
            "rob" => command.add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Member to rob").required(true)),
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
            "announcement" => command
                .add_option(CreateCommandOption::new(CommandOptionType::Integer, "date", "Days from today").required(true).add_int_choice("Today", 0).add_int_choice("Tomorrow", 1).add_int_choice("In 2 days", 2).add_int_choice("In 3 days", 3).add_int_choice("In 1 week", 7).add_int_choice("In 2 weeks", 14).add_int_choice("In 1 month", 30))
                .add_option(CreateCommandOption::new(CommandOptionType::Integer, "time", "Minutes from now").required(true).add_int_choice("In 5 minutes", 5).add_int_choice("In 15 minutes", 15).add_int_choice("In 30 minutes", 30).add_int_choice("In 1 hour", 60).add_int_choice("In 2 hours", 120).add_int_choice("In 6 hours", 360).add_int_choice("In 12 hours", 720).add_int_choice("In 24 hours", 1440))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "news", "Announcement text").required(true)),
            "event" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "name", "Event name").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::Integer, "date", "Days from today").required(true).add_int_choice("Today", 0).add_int_choice("Tomorrow", 1).add_int_choice("In 2 days", 2).add_int_choice("In 3 days", 3).add_int_choice("In 1 week", 7).add_int_choice("In 2 weeks", 14).add_int_choice("In 1 month", 30))
                .add_option(CreateCommandOption::new(CommandOptionType::Integer, "time", "Minutes from now").required(true).add_int_choice("In 5 minutes", 5).add_int_choice("In 15 minutes", 15).add_int_choice("In 30 minutes", 30).add_int_choice("In 1 hour", 60).add_int_choice("In 2 hours", 120).add_int_choice("In 6 hours", 360).add_int_choice("In 12 hours", 720).add_int_choice("In 24 hours", 1440))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "prize", "Optional event prize").required(false)),
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

pub async fn handle_prefix(ctx: &Context, message: &Message, cfg: &GuildConfig, store: &CommunityStore, music: &MusicManager) -> bool {
    let Some(guild_id) = message.guild_id.map(|id| id.get()) else { return false };
    let Some(rest) = message.content.strip_prefix(&cfg.prefix) else { return false };
    let mut parts = rest.trim().splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("").to_lowercase();
    let argument = parts.next().unwrap_or("").trim();
    let user_id = message.author.id.get();
    let feature = match command.as_str() {
        "balance" | "daily" | "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" | "work" | "pay" | "rob" => Some("economy"),
        "xp" | "level" | "leaderboard" => Some("xp"),
        "swears" | "swear-leaderboard" => Some("swear_counter"),
        "timeout-minutes" | "dm" | "set-log" => Some("moderation"),
        "vibecheck" | "ship" | "fortune" | "wyr" => Some("fun"),
        "youtube" => Some("social"),
        "play" | "join" | "leave" | "pause" | "resume" | "skip" | "stop" | "queue" | "volume" | "lyrics" => Some("music"),
        _ => None,
    };
    if cfg.disabled_commands.iter().any(|name| name == &command)
        || feature.and_then(|key| cfg.features.get(key).and_then(Value::as_bool)) == Some(false) {
        let _ = message.reply(&ctx.http, format!("The {command} command is disabled in this server.")).await;
        return true;
    }
    let custom_key = format!("{guild_id}:{command}");
    if let Some(response) = store.record("custom_commands", &custom_key).and_then(|v| v.as_str().map(str::to_owned)) {
        let _ = message.reply(&ctx.http, response.chars().take(1900).collect::<String>()).await;
        return true;
    }
    let starting = cfg.economy_config.get("starting_money").and_then(Value::as_i64).unwrap_or(100);
    let response = match command.as_str() {
        "play" => {
            if let Some(channel) = music.voice_channel_for_user(guild_id, user_id).await {
                music.request_voice_channel(guild_id, Some(channel)).await;
                match music.enqueue(guild_id, argument).await { Ok(text) => text, Err(error) => format!("Music error: {error}") }
            } else { "Join a voice channel first, then use -mb play <song>.".into() }
        }
        "join" => {
            if let Some(channel) = music.voice_channel_for_user(guild_id, user_id).await { music.request_voice_channel(guild_id, Some(channel)).await; "Joining your voice channel.".into() }
            else { "Join a voice channel first.".into() }
        }
        "leave" => { music.request_voice_channel(guild_id, None).await; let _ = music.stop(guild_id).await; "Leaving the voice channel and clearing the queue.".into() }
        "pause" | "resume" => match music.pause(guild_id, command == "pause").await { Ok(text) => text, Err(error) => format!("Music error: {error}") },
        "skip" => match music.skip(guild_id).await { Ok(text) => text, Err(error) => format!("Music error: {error}") },
        "stop" => match music.stop(guild_id).await { Ok(text) => text, Err(error) => format!("Music error: {error}") },
        "queue" => music.queue_text(guild_id).await,
        "volume" => {
            let level = argument.parse::<u16>().unwrap_or(100).min(100);
            match music.volume(guild_id, level).await { Ok(text) => text, Err(error) => format!("Music error: {error}") }
        }
        "lyrics" => {
            let enabled = match argument.to_lowercase().as_str() { "on" | "true" | "enable" => Some(true), "off" | "false" | "disable" => Some(false), _ => None };
            music.set_lyrics(guild_id, enabled).await
        }
        "timeout-minutes" => {
            let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
            if !permitted { "You need Manage Server permission to change this setting.".into() }
            else if let Ok(minutes) = argument.parse::<u64>() {
                if !(1..=10_080).contains(&minutes) { "Usage: -mb timeout-minutes 1-10080".into() }
                else { store.set_timeout_minutes(guild_id, minutes); format!("Discord invite timeout set to {minutes} minutes.") }
            } else { "Usage: -mb timeout-minutes 1-10080".into() }
        }
        "swears" => {
            let total = store.swears(guild_id, user_id);
            let adjustment = cfg.swear_count_adjustments.get(user_id.to_string()).and_then(Value::as_i64).unwrap_or(0);
            format!("You have {} swear(s) in this server.", (total + adjustment).max(0))
        }
        "swear-leaderboard" => {
            let mut rows = store.swear_leaderboard(guild_id);
            let adjustments = cfg.swear_count_adjustments.as_object();
            if let Some(adjustments) = adjustments {
                for (id, adjustment) in adjustments {
                    if let (Ok(user), Some(delta)) = (id.parse::<u64>(), adjustment.as_i64()) {
                        if let Some(row) = rows.iter_mut().find(|row| row.0 == user) { row.1 = (row.1 + delta).max(0); }
                        else if delta > 0 { rows.push((user, delta)); }
                    }
                }
            }
            rows.retain(|row| row.1 > 0);
            rows.sort_by(|a, b| b.1.cmp(&a.1));
            rows.truncate(10);
            if rows.is_empty() { "No swear counts have been recorded yet.".into() }
            else { rows.iter().enumerate().map(|(i, (id, count))| format!("{}. <@{id}> — {} swear(s)", i + 1, count)).collect::<Vec<_>>().join("\\n") }
        }
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
        "pay" => {
            let target = message.mentions.first().map(|user| user.id.get());
            let amount = argument.split_whitespace().last().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
            if let Some(target) = target {
                let balance = store.balance(guild_id, user_id, starting);
                if target == user_id || amount < 1 { "Usage: -mb pay @member amount".into() }
                else if amount > balance { format!("You only have {balance} Mallow Coins.") }
                else {
                    let target_balance = store.balance(guild_id, target, starting);
                    store.set_balance(guild_id, user_id, balance - amount);
                    store.set_balance(guild_id, target, target_balance + amount);
                    format!("Sent {amount} Mallow Coins to <@{target}>.")
                }
            } else { "Mention a member to pay. Usage: -mb pay @member amount".into() }
        }
        "rob" => {
            if let Some(target) = message.mentions.first().map(|user| user.id.get()) {
                let target_balance = store.balance(guild_id, target, starting);
                if target == user_id { "You cannot rob yourself.".into() }
                else if target_balance < 1 { "That member has no Mallow Coins to steal.".into() }
                else if pseudo_roll(100) < 35 {
                    let stolen = (target_balance / 4).max(1).min(target_balance);
                    let own = store.balance(guild_id, user_id, starting);
                    store.set_balance(guild_id, target, target_balance - stolen);
                    store.set_balance(guild_id, user_id, own + stolen);
                    format!("You stole {stolen} Mallow Coins from <@{target}>.")
                } else { "The robbery failed. You got away empty-handed.".into() }
            } else { "Mention a member to rob. Usage: -mb rob @member".into() }
        }
        "vibecheck" => format!("Vibe scan complete: {}% vibe.", pseudo_roll(101)),
        "ship" => format!("Compatibility scan: {}%. This result is entirely unscientific.", pseudo_roll(101)),
        "help" | "commands" => "Commands: xp, leaderboard, balance, daily, work, gamble, coinflip, fortune, vibecheck, ship, update.".into(),
        "update" => std::fs::read_to_string("update.txt").unwrap_or_else(|_| "No update log is available right now.".into()).chars().take(1900).collect::<String>(),
        _ => return false,
    };
    if let Err(error) = message.reply(&ctx.http, response).await { warn!(%error, command, "Could not respond to prefix command"); }
    true
}

async fn create_ticket_channel(http: &Http, guild_id: u64, user_id: u64, username: &str, cfg: &GuildConfig, store: &CommunityStore) -> Result<u64, String> {
    if cfg.tickets_config.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err("Tickets are disabled for this server.".into());
    }
    if let Some(existing) = store.open_ticket(guild_id, user_id) {
        return Err(format!("You already have an open ticket: <#{existing}>."));
    }
    let guild = serenity::all::GuildId::new(guild_id);
    let bot = http.get_current_user().await.map_err(|_| "I couldn't identify the bot account.".to_string())?;
    let mut overwrites = vec![
        PermissionOverwrite { allow: Permissions::empty(), deny: Permissions::VIEW_CHANNEL, kind: PermissionOverwriteType::Role(RoleId::new(guild_id)) },
        PermissionOverwrite { allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(UserId::new(user_id)) },
        PermissionOverwrite { allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY, deny: Permissions::empty(), kind: PermissionOverwriteType::Member(bot.id) },
    ];
    if let Some(roles) = cfg.tickets_config.get("support_role_ids").and_then(Value::as_array) {
        for role in roles.iter().filter_map(Value::as_str).filter_map(|v| v.parse::<u64>().ok()).take(20) {
            overwrites.push(PermissionOverwrite { allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY, deny: Permissions::empty(), kind: PermissionOverwriteType::Role(RoleId::new(role)) });
        }
    }
    let slug: String = username.to_lowercase().chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-').take(40).collect();
    let mut builder = CreateChannel::new(format!("ticket-{}", if slug.is_empty() { user_id.to_string() } else { slug }))
        .kind(ChannelType::Text)
        .permissions(overwrites)
        .topic(format!("mallow-ticket:{user_id}:{guild_id}"))
        .audit_log_reason("Mallow Bot support ticket");
    if let Some(category) = cfg.tickets_config.get("category_id").and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok()) {
        builder = builder.category(serenity::all::ChannelId::new(category));
    }
    let channel = guild.create_channel(http, builder).await.map_err(|_| "I couldn't create the private ticket channel. Check Manage Channels and permission settings.".to_string())?;
    let welcome = cfg.tickets_config.get("welcome_message").and_then(Value::as_str).unwrap_or("Thanks for opening a ticket. A staff member will be with you soon.");
    let close_button = CreateActionRow::Buttons(vec![
        CreateButton::new("mallow:ticket:close").label("Close ticket").style(ButtonStyle::Danger)
    ]);
    let _ = channel.id.send_message(http, CreateMessage::new()
        .content(format!("<@{user_id}>\n{welcome}"))
        .components(vec![close_button])).await;
    store.set_record("tickets", &channel.id.get().to_string(), Some(json!({"guild_id":guild_id,"user_id":user_id,"opened_at":now_seconds()})));
    Ok(channel.id.get())
}

fn level(xp: i64) -> i64 { ((xp.max(0) as f64 / 100.0).sqrt()) as i64 }
fn now_seconds() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn pseudo_roll(max: u64) -> u64 {
    let t = now_seconds().wrapping_mul(6364136223846793005).wrapping_add(std::process::id() as u64);
    if max == 0 { 0 } else { (t ^ (t >> 17) ^ (t >> 31)) % max }
}

async fn respond(http: &Http, command: &CommandInteraction, content: String, ephemeral: bool, deferred: bool) {
    if deferred {
        let _ = command.edit_response(http, EditInteractionResponse::new().content(content)).await;
    } else {
        let builder = CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(content).ephemeral(ephemeral));
        let _ = command.create_response(http, builder).await;
    }
}

pub async fn handle(http: &Http, command: &CommandInteraction, store: &CommunityStore, premium: &PremiumClient, cfg: &GuildConfig, deferred: bool) -> bool {
    let Some(guild_id) = command.guild_id.map(|id| id.get()) else { return false };
    let user_id = command.user.id.get();
    let name = command.data.name.as_str();
    if cfg.disabled_commands.iter().any(|disabled| disabled == name) {
        let _ = respond(http, command, format!("The {name} command is disabled in this server."), true, deferred).await;
        return true;
    }
    let feature = match name {
        "balance" | "daily" | "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" | "work" | "pay" | "rob" => Some("economy"),
        "xp" | "leaderboard" => Some("xp"),
        "vibecheck" | "ship" | "fortune" | "wyr" => Some("fun"),
        "youtube" => Some("social"),
        "dm" | "set-log" | "purge" => Some("moderation"),
        _ => None,
    };
    if feature.and_then(|key| cfg.features.get(key).and_then(Value::as_bool)).unwrap_or(true) == false {
        let _ = respond(http, command, format!("The {name} feature is disabled in this server."), true, deferred).await;
        return true;
    }
    let starting = cfg.economy_config.get("starting_money").and_then(Value::as_i64).unwrap_or(100);
    if matches!(name, "embed" | "snipe" | "sticky" | "backup" | "custom-command" | "vanity") && user_id != 1021068313071210498 {
        let (banned, active, _) = premium.status(user_id).await;
        if banned {
            let _ = respond(http, command, "This account cannot use Mallow Bot.".into(), true, deferred).await;
            return true;
        }
        if !active {
            let _ = respond(http, command, "Mallow Premium is required. Get Premium at https://mallowbot.kz.ci/subscriptions.html".into(), true, deferred).await;
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
                    if let Err(error) = command.channel_id.say(http, text.chars().take(1900).collect::<String>()).await { warn!(%error, "Could not send /say message"); }
                    "Message sent.".into()
                }
            }
        }
        "update" => {
            match fs::read_to_string("update.txt") {
                Ok(update) if !update.trim().is_empty() => {
                    let mut sent = true;
                    for chunk in update.as_bytes().chunks(1800) {
                        let text = String::from_utf8_lossy(chunk);
                        if command.channel_id.say(http, text.to_string()).await.is_err() { sent = false; break; }
                    }
                    if sent { "Posted the update log.".into() } else { "I couldn't post the full update log.".into() }
                }
                _ => "No update log is available right now.".into()
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
            else { let reward = cfg.economy_config.get("daily_reward").and_then(Value::as_i64).unwrap_or(100); let value = store.balance(guild_id, user_id, starting) + reward; store.set_balance(guild_id, user_id, value); format!("You claimed {reward} Mallow Coins. Balance: {value}.") }
        }
        "work" => {
            let min = cfg.economy_config.get("work_min").and_then(Value::as_i64).unwrap_or(20);
            let max = cfg.economy_config.get("work_max").and_then(Value::as_i64).unwrap_or(80).max(min);
            let earned = min + pseudo_roll((max - min + 1) as u64) as i64;
            let value = store.balance(guild_id, user_id, starting) + earned;
            store.set_balance(guild_id, user_id, value);
            format!("You worked a shift and earned {earned} Mallow Coins. Balance: {value}.")
        }
        "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" => {
            let bet = command.data.options.iter().find_map(|o| if o.name == "bet" { if let CommandDataOptionValue::Integer(value) = o.value { Some(value) } else { None } } else { None }).unwrap_or(10);
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
        "pay" => {
            let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = o.value { Some(id.get()) } else { None } } else { None });
            let amount = command.data.options.iter().find_map(|o| if o.name == "amount" { if let CommandDataOptionValue::Integer(value) = o.value { Some(value) } else { None } } else { None }).unwrap_or(0);
            if let Some(target) = target {
                let balance = store.balance(guild_id, user_id, starting);
                if target == user_id || amount < 1 { "Choose another member and an amount greater than zero.".into() }
                else if amount > balance { format!("You only have {balance} Mallow Coins.") }
                else {
                    let target_balance = store.balance(guild_id, target, starting);
                    store.set_balance(guild_id, user_id, balance - amount);
                    store.set_balance(guild_id, target, target_balance + amount);
                    format!("Sent {amount} Mallow Coins to <@{target}>.")
                }
            } else { "Choose a member to pay.".into() }
        }
        "rob" => {
            let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = o.value { Some(id.get()) } else { None } } else { None });
            if let Some(target) = target {
                let target_balance = store.balance(guild_id, target, starting);
                if target == user_id { "You cannot rob yourself.".into() }
                else if target_balance < 1 { "That member has no Mallow Coins to steal.".into() }
                else if pseudo_roll(100) < 35 {
                    let stolen = (target_balance / 4).max(1).min(target_balance);
                    let own = store.balance(guild_id, user_id, starting);
                    store.set_balance(guild_id, target, target_balance - stolen);
                    store.set_balance(guild_id, user_id, own + stolen);
                    format!("You stole {stolen} Mallow Coins from <@{target}>.")
                } else { "The robbery failed. You got away empty-handed.".into() }
            } else { "Choose a member to rob.".into() }
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
                    match command.channel_id.send_message(http, CreateMessage::new().content(&content)).await {
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
                match command.channel_id.send_message(http, CreateMessage::new().embed(embed)).await {
                    Ok(_) => "Embed sent.".into(),
                    Err(_) => "I couldn't send the embed. Check my channel permissions.".into()
                }
            }
        }
        "set-log" => {
            if user_id != 1021068313071210498 { "Only the bot developer can configure DM logs.".into() }
            else {
                let channel = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Channel(id) = o.value { Some(id.get()) } else { None });
                if let Some(channel) = channel {
                    store.set_record("log_channels", &guild_id.to_string(), Some(json!(channel)));
                    format!("DM conversations will be logged to <#{channel}>.")
                } else { "Choose a text channel for DM logs.".into() }
            }
        }
        "dm" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if !permitted { "You need Manage Messages permission to use this command.".into() }
            else {
                let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = o.value { Some(id) } else { None } } else { None });
                let text = command.data.options.iter().find_map(|o| if o.name == "message" { if let CommandDataOptionValue::String(value) = &o.value { Some(value.as_str()) } else { None } } else { None }).unwrap_or("");
                let log_channel = store.record("log_channels", &guild_id.to_string()).and_then(|v| v.as_u64());
                if let (Some(target), Some(log_channel)) = (target, log_channel) {
                    if target.get() == user_id || text.trim().is_empty() || text.len() > 1900 { "Choose another member and provide a message of 1,900 characters or fewer.".into() }
                    else {
                        match target.create_dm_channel(http).await {
                            Ok(channel) if channel.say(http, text).await.is_ok() => {
                                store.set_record("dm_routes", &target.get().to_string(), Some(json!({"guild_id":guild_id,"channel_id":log_channel,"set_by":user_id,"updated_at":now_seconds()})));
                                format!("Sent a DM to <@{}>. Their replies will be forwarded only to <#{log_channel}>.", target.get())
                            }
                            _ => "I couldn't DM that member. They may have DMs disabled.".into()
                        }
                    }
                } else { "This server needs a DM log channel. Ask the bot developer to configure it with /set-log.".into() }
            }
        }
        "purge" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if !permitted { "You need Manage Messages permission to use this command.".into() }
            else {
                let amount = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Integer(value) = o.value { Some(value) } else { None }).unwrap_or(0).clamp(1,100) as u8;
                match command.channel_id.messages(http, GetMessages::new().limit(amount)).await {
                    Ok(messages) if !messages.is_empty() => {
                        let ids: Vec<_> = messages.iter().map(|message| message.id).collect();
                        match command.channel_id.delete_messages(http, ids).await {
                            Ok(()) => format!("Deleted {} message(s).", messages.len()),
                            Err(_) => "I couldn't delete those messages. Check my Manage Messages permission and message age.".into()
                        }
                    }
                    Ok(_) => "There are no messages to delete.".into(),
                    Err(_) => "I couldn't read this channel's message history.".into()
                }
            }
        }
        "snipe" => {
            match store.record("snipes", &command.channel_id.get().to_string()) {
                Some(value) => format!("Recently deleted message: {}", value.get("content").and_then(Value::as_str).unwrap_or("[No text content]")),
                None => "There is no recently deleted message saved for this channel.".into()
            }
        }
        "autorole" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_roles()).unwrap_or(false);
            if !permitted { "You need Manage Roles permission to use this command.".into() }
            else {
                let action = command.data.options.iter().find_map(|o| if o.name == "action" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("status");
                let role = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Role(id) = o.value { Some(id.get()) } else { None });
                match action {
                    "set" => if let Some(role) = role { store.set_autorole(guild_id, Some(role)); format!("Auto-role enabled. New members will receive <@&{role}>.") } else { "Choose a role to assign.".into() },
                    "disable" => { store.set_autorole(guild_id, None); "Auto-role has been disabled for this server.".into() },
                    _ => store.autorole(guild_id).map(|id| format!("New members will receive <@&{id}>.")).unwrap_or_else(|| "Auto-role is not configured. Use /autorole action:Set to choose a role.".into())
                }
            }
        }
        "rules" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_roles()).unwrap_or(false);
            if !permitted { "You need Manage Roles permission to use this command.".into() }
            else {
                let enabled = command.data.options.iter().find_map(|o| if o.name == "enabled" { if let CommandDataOptionValue::Boolean(v) = o.value { Some(v) } else { None } } else { None }).unwrap_or(false);
                let message_id = command.data.options.iter().find_map(|o| if o.name == "message_id" { if let CommandDataOptionValue::String(v) = &o.value { v.parse::<u64>().ok() } else { None } } else { None });
                let emoji = command.data.options.iter().find_map(|o| if o.name == "emoji" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.clone()) } else { None } } else { None }).unwrap_or_default();
                let role = command.data.options.iter().find_map(|o| if o.name == "role" { if let CommandDataOptionValue::Role(id) = o.value { Some(id.get()) } else { None } } else { None });
                if let (Some(message_id), Some(role)) = (message_id, role) {
                    if emoji.is_empty() || emoji.len() > 100 { "Enter a valid emoji.".into() }
                    else {
                        if enabled {
                            let reaction = ReactionType::Unicode(emoji.clone());
                            if let Err(_) = command.channel_id.create_reaction(http, serenity::all::MessageId::new(message_id), reaction).await { "I couldn't add the verification reaction in this channel.".into() }
                            else { store.set_rule(guild_id, message_id, command.channel_id.get(), &emoji, role, enabled); format!("Rules verification enabled for message {message_id}. React with {emoji} to receive <@&{role}>.") }
                        } else {
                            store.set_rule(guild_id, message_id, command.channel_id.get(), &emoji, role, false);
                            format!("Rules verification disabled for message {message_id}.")
                        }
                    }
                } else { "Provide a valid message ID and role.".into() }
            }
        }
        "reaction-role" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_roles()).unwrap_or(false);
            if !permitted { "You need Manage Roles permission to use this command.".into() }
            else {
                let action = command.data.options.iter().find_map(|o| if o.name == "action" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("");
                let message_id = command.data.options.iter().find_map(|o| if o.name == "message_id" { if let CommandDataOptionValue::String(v) = &o.value { v.parse::<u64>().ok() } else { None } } else { None });
                let emoji = command.data.options.iter().find_map(|o| if o.name == "emoji" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.clone()) } else { None } } else { None }).unwrap_or_default();
                let role = command.data.options.iter().find_map(|o| if o.name == "role" { if let CommandDataOptionValue::Role(id) = o.value { Some(id.get()) } else { None } } else { None });
                match action {
                    "create" => {
                        let channel = command.data.options.iter().find_map(|o| if o.name == "channel" { if let CommandDataOptionValue::Channel(id) = o.value { Some(id) } else { None } } else { None }).unwrap_or(command.channel_id);
                        let title = command.data.options.iter().find_map(|o| if o.name == "title" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("Choose your roles");
                        let description = command.data.options.iter().find_map(|o| if o.name == "description" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("React below to get a role. Remove your reaction to remove it.");
                        match channel.send_message(http, CreateMessage::new().embed(CreateEmbed::new().title(title).description(description))).await {
                            Ok(posted) => { store.set_record("reaction_roles", &posted.id.get().to_string(), Some(json!({"guild_id":guild_id.to_string(),"channel_id":channel.get().to_string(),"roles":{}}))); format!("Reaction-role panel created: https://discord.com/channels/{guild_id}/{}/{}/", channel.get(), posted.id.get()) }
                            Err(_) => "I couldn't post in that channel. Check my channel permissions.".into()
                        }
                    }
                    "add" | "remove" => {
                        if let (Some(message_id), Some(role)) = (message_id, role) {
                            let panel = store.record("reaction_roles", &message_id.to_string());
                            let channel = panel.as_ref().and_then(|v| v.get("channel_id")).and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok());
                            if emoji.is_empty() || channel.is_none() { "That panel or emoji was not found.".into() }
                            else if action == "add" {
                                let reaction = ReactionType::Unicode(emoji.clone());
                                if serenity::all::ChannelId::new(channel.unwrap()).create_reaction(http, serenity::all::MessageId::new(message_id), reaction).await.is_err() { "I couldn't add that reaction to the panel.".into() }
                                else { store.set_reaction_role(guild_id, message_id, &emoji, Some(role)); format!("Added {emoji} → <@&{role}>.") }
                            } else { store.set_reaction_role(guild_id, message_id, &emoji, None); format!("Removed the {emoji} role mapping.") }
                        } else { "Provide a panel message ID, emoji, and role.".into() }
                    }
                    "list" => {
                        if let Some(message_id) = message_id {
                            let panel = store.record("reaction_roles", &message_id.to_string());
                            let roles = panel.and_then(|v| v.get("roles").cloned()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
                            if roles.is_empty() { "This panel has no role mappings yet.".into() }
                            else { roles.iter().map(|(emoji, role)| format!("{emoji} → <@&{}>", role.as_u64().unwrap_or(0))).collect::<Vec<_>>().join("\\n") }
                        } else { "Provide the panel message ID.".into() }
                    }
                    _ => "Choose Create, Add, Remove, or List.".into()
                }
            }
        }
        "announcement" | "event" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
            if guild_id != 1544628300217978932 { "This command is only available in Bliss SMP.".into() }
            else if !permitted { "You need Manage Messages permission to publish Bliss SMP announcements.".into() }
            else {
                let days = command.data.options.iter().find_map(|o| if o.name == "date" { if let CommandDataOptionValue::Integer(v) = o.value { Some(v) } else { None } } else { None }).unwrap_or(0).max(0) as u64;
                let minutes = command.data.options.iter().find_map(|o| if o.name == "time" { if let CommandDataOptionValue::Integer(v) = o.value { Some(v) } else { None } } else { None }).unwrap_or(5).max(0) as u64;
                let timestamp = now_seconds() + days * 86_400 + minutes * 60;
                let channel = serenity::all::ChannelId::new(1574140545570181141);
                if name == "announcement" {
                    let news = command.data.options.iter().find_map(|o| if o.name == "news" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("");
                    if news.trim().is_empty() || news.len() > 1500 { "Announcement text must be between 1 and 1,500 characters.".into() }
                    else {
                        let embed = CreateEmbed::new().title("BLISS SMP ANNOUNCEMENT")
                            .description(format!("━━━━━━━━━━━━━━━━━━━━━━\n\n{}\n\n━━━━━━━━━━━━━━━━━━━━━━", news.trim()))
                            .field("Date", format!("<t:{timestamp}:D>"), true)
                            .field("Time", format!("<t:{timestamp}:t>\n<t:{timestamp}:R>"), true)
                            .field("Realm Code", "Find the code in <#1544638277179678730>", false)
                            .field("Want to join?", "React to this announcement with 💎 if you're joining!", false)
                            .footer(serenity::builder::CreateEmbedFooter::new("BLISS SMP • Community Announcement"));
                        match channel.send_message(http, CreateMessage::new().embed(embed)).await {
                            Ok(posted) => { let _ = channel.create_reaction(http, posted.id, ReactionType::Unicode("💎".into())).await; "Bliss SMP announcement posted.".into() }
                            Err(_) => "I couldn't publish the announcement. Check my channel permissions.".into()
                        }
                    }
                } else {
                    let event_name = command.data.options.iter().find_map(|o| if o.name == "name" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("");
                    let prize = command.data.options.iter().find_map(|o| if o.name == "prize" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).filter(|v| !v.trim().is_empty()).unwrap_or("No prize announced");
                    if event_name.trim().is_empty() || event_name.len() > 150 { "Event name must be between 1 and 150 characters.".into() }
                    else {
                        let embed = CreateEmbed::new().title("BLISS SMP EVENT")
                            .description(format!("━━━━━━━━━━━━━━━━━━━━━━\n\n**{}**\n\n━━━━━━━━━━━━━━━━━━━━━━", event_name.trim()))
                            .field("Date", format!("<t:{timestamp}:D>"), true)
                            .field("Time", format!("<t:{timestamp}:t>\n<t:{timestamp}:R>"), true)
                            .field("Prize", prize, false)
                            .field("How to join", "React to this event with 🎪 to enter!", false)
                            .footer(serenity::builder::CreateEmbedFooter::new("BLISS SMP • Community Event"));
                        match channel.send_message(http, CreateMessage::new().embed(embed)).await {
                            Ok(posted) => { let _ = channel.create_reaction(http, posted.id, ReactionType::Unicode("🎪".into())).await; "Bliss SMP event posted.".into() }
                            Err(_) => "I couldn't publish the event. Check my channel permissions.".into()
                        }
                    }
                }
            }
        }
        "ticket" => {
            match create_ticket_channel(http, guild_id, user_id, &command.user.name, cfg, store).await {
                Ok(channel) => format!("Your private support ticket has been created: <#{channel}>."),
                Err(error) => error
            }
        }
        "ticket-panel" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
            if !permitted { "You need Manage Server permission to post a ticket panel.".into() }
            else {
                let open_button = CreateActionRow::Buttons(vec![
                    CreateButton::new("mallow:ticket:open").label("Open a ticket").style(ButtonStyle::Primary)
                ]);
                match command.channel_id.send_message(http, CreateMessage::new()
                    .embed(CreateEmbed::new().title("Mallow Bot Support").description("Need help? Click below to open a private support channel with the server's support team."))
                    .components(vec![open_button])).await {
                    Ok(_) => "Support ticket panel posted.".into(),
                    Err(_) => "I couldn't post the ticket panel in this channel.".into()
                }
            }
        }
        "ticket-close" => {
            if store.record("tickets", &command.channel_id.get().to_string()).is_none() { "This channel is not a ticket created by Mallow Bot.".into() }
            else {
                match command.channel_id.delete(http).await {
                    Ok(_) => { store.set_record("tickets", &command.channel_id.get().to_string(), None); "Ticket closed.".into() }
                    Err(_) => "I couldn't close this ticket. Check my Manage Channels permission.".into()
                }
            }
        }
        "backup" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.administrator()).unwrap_or(false);
            if !permitted { "You need Administrator permission to use this command.".into() }
            else {
                let guild = command.guild_id.unwrap();
                match (guild.channels(http).await, guild.roles(http).await) {
                    (Ok(channels), Ok(roles)) => {
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
                            })).collect::<Vec<_>>(),
                            "roles": roles.values().map(|role| json!({
                                "id": role.id.get(),
                                "name": role.name,
                                "permissions": role.permissions.bits(),
                                "colour": role.colour.0,
                                "position": role.position,
                                "hoist": role.hoist,
                                "mentionable": role.mentionable
                            })).collect::<Vec<_>>()
                        });
                        match serde_json::to_vec_pretty(&payload) {
                            Ok(bytes) => {
                                let attachment = CreateAttachment::bytes(bytes, format!("mallow-backup-{guild_id}.json"));
                                if command.channel_id.send_message(http, CreateMessage::new().content("Server channel-layout export attached. This is not a full permission or message restore backup.").add_file(attachment)).await.is_ok() {
                                    "Server layout export attached.".into()
                                } else { "I couldn't attach the server backup. Check my channel permissions.".into() }
                            }
                            Err(_) => "I couldn't serialize the server backup.".into()
                        }
                    }
                    _ => "I couldn't read this server's channel and role layout. Check my permissions.".into()
                }
            }
        },
        _ => return false,
    };
    respond(http, command, response, false, deferred).await;
    true
}
