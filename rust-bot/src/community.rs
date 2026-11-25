use serde_json::{json, Value};
use crate::config::GuildConfig;
use crate::halloween;
use crate::premium::PremiumClient;
use crate::music::MusicManager;
use serenity::{
    all::{ChannelType, EmojiId, Command, CommandDataOptionValue, CommandInteraction, CommandOptionType, CreateActionRow, CreateAttachment, CreateButton, CreateChannel, CreateCommand, CreateCommandOption, CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse, CreateMessage, GetMessages, GuildId, Message, PermissionOverwrite, PermissionOverwriteType, Permissions, ReactionType, RoleId, UserId, ButtonStyle},
    http::Http,
    prelude::Context,
};
use std::{fs, path::PathBuf, sync::{Arc, Mutex}, time::{Instant, SystemTime, UNIX_EPOCH}};
use tracing::warn;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

#[derive(Clone)]
pub struct CommunityStore {
    path: Arc<PathBuf>,
    data: Arc<Mutex<Value>>,
}

impl CommunityStore {
    pub fn open() -> Self {
        let path = PathBuf::from(std::env::var("MALLOW_DATA_DIR").unwrap_or_else(|_| ".".into())).join("community_data.json");
        let data = fs::read_to_string(&path).ok()
            .or_else(|| fs::read_to_string("community_data.json").ok())
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
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
            let last = data.get("last_xp").and_then(|v| v.get(&key)).and_then(Value::as_f64).map(|v| v.max(0.0) as u64).unwrap_or(0);
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

    // Gateway reconnects can occasionally deliver the same message event again.
    // Persist a bounded set of message IDs so one Discord message can only count once.
    pub fn add_swears_once(&self, guild: u64, user: u64, message_id: u64, amount: i64) -> Option<i64> {
        self.mutate(|data| {
            let message_key = json!(message_id);
            let seen = data["swear_message_ids"].as_array_mut();
            if let Some(seen) = seen {
                if seen.contains(&message_key) { return None; }
                seen.push(message_key);
                if seen.len() > 5000 { seen.drain(0..seen.len() - 5000); }
            } else {
                data["swear_message_ids"] = json!([message_id]);
            }
            let key = format!("{guild}:{user}");
            let old = data.get("swears").and_then(|v| v.get(&key)).and_then(Value::as_i64).unwrap_or(0);
            let new = old.saturating_add(amount.max(0));
            data["swears"][key] = json!(new);
            Some(new)
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
            let previous = data.get("daily_claims").and_then(|v| v.get(&key)).and_then(Value::as_f64).map(|v| v.max(0.0) as u64).unwrap_or(0);
            if now.saturating_sub(previous) < 86_400 { return true; }
            data["daily_claims"][key] = json!(now);
            false
        })
    }

    pub fn existing_ticket(&self, guild: u64, user: u64) -> Option<u64> {
        self.read(|data| {
            data.get("tickets").and_then(Value::as_object)?.iter().find_map(|(channel, ticket)| {
                (ticket.get("guild_id").and_then(Value::as_u64) == Some(guild)
                    && ticket.get("user_id").and_then(Value::as_u64) == Some(user))
                    .then(|| channel.parse::<u64>().ok()).flatten()
            })
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
    pub fn custom_command(&self, guild: u64, name: &str) -> Option<String> {
        self.read(|data| data.get("custom_commands").and_then(|v| v.get(guild.to_string()))
            .and_then(|v| v.get(name)).and_then(Value::as_str).map(str::to_owned))
    }

    pub fn custom_commands(&self, guild: u64) -> Vec<String> {
        self.read(|data| data.get("custom_commands").and_then(|v| v.get(guild.to_string()))
            .and_then(Value::as_object).map(|values| values.keys().cloned().collect()).unwrap_or_default())
    }

    pub fn set_custom_command(&self, guild: u64, name: &str, response: Option<&str>) {
        self.mutate(|data| {
            let key = guild.to_string();
            if let Some(response) = response {
                data["custom_commands"][&key][name] = json!(response);
            } else if let Some(commands) = data.get_mut("custom_commands").and_then(|v| v.get_mut(&key)).and_then(Value::as_object_mut) {
                commands.remove(name);
            }
        });
    }

    pub fn try_cooldown(&self, name: &str, guild: u64, user: u64, seconds: u64) -> Option<u64> {
        self.mutate(|data| {
            let key = format!("{guild}:{user}");
            let now = now_seconds();
            let last = data.get("cooldowns").and_then(|v| v.get(name)).and_then(|v| v.get(&key)).and_then(Value::as_u64).unwrap_or(0);
            if now < last.saturating_add(seconds) {
                Some(last + seconds - now)
            } else {
                data["cooldowns"][name][key] = json!(now);
                None
            }
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
        ("rule", "Set up verified role and channel access rules"),
        ("autorole", "Configure automatic member roles"),
        ("reaction-role", "Configure reaction role panels"),
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
                .add_option(CreateCommandOption::new(CommandOptionType::String, "description", "Embed text").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "color", "Hex color such as #8B5CF6").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::Channel, "channel", "Where to send the embed").required(false)),
            "vibecheck" => command.add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Who should get their vibe checked? Leave empty for yourself.").required(false)),
            "ship" => command.add_option(CreateCommandOption::new(CommandOptionType::User, "member", "The other half of this pairing").required(true)),
            "custom-command" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "action", "Create, list, or delete").required(true).add_string_choice("Create", "create").add_string_choice("List", "list").add_string_choice("Delete", "delete"))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "name", "Command name").required(false))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "response", "Command response").required(false)),
            "autorole" => command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "action", "Set, disable, or status").required(true).add_string_choice("Set", "set").add_string_choice("Disable", "disable").add_string_choice("Status", "status"))
                .add_option(CreateCommandOption::new(CommandOptionType::Role, "role", "Role to give new members").required(false)),
            "rule" => command
                .add_option(CreateCommandOption::new(CommandOptionType::Boolean, "make_verified_role", "Create the verified role if it does not exist").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "message_id", "Rules message ID in this channel").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::Boolean, "setup_channels", "Hide all channels and voice channels from @everyone").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "reaction_emoji", "Emoji members react with to verify").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "role_color", "Role color name or hex code, e.g. blue or #5865F2").required(true))
                .add_option(CreateCommandOption::new(CommandOptionType::String, "role_name", "Verified role name (defaults to Verified)").required(false)),
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

pub fn bliss_commands() -> Vec<CreateCommand> {
    let date = || CreateCommandOption::new(CommandOptionType::Integer, "date", "Days from today")
        .required(true)
        .add_int_choice("Today", 0).add_int_choice("Tomorrow", 1)
        .add_int_choice("In 2 days", 2).add_int_choice("In 3 days", 3)
        .add_int_choice("In 1 week", 7).add_int_choice("In 2 weeks", 14)
        .add_int_choice("In 1 month", 30);
    let time = || CreateCommandOption::new(CommandOptionType::Integer, "time", "Minutes from now")
        .required(true)
        .add_int_choice("In 5 minutes", 5).add_int_choice("In 15 minutes", 15)
        .add_int_choice("In 30 minutes", 30).add_int_choice("In 1 hour", 60)
        .add_int_choice("In 2 hours", 120).add_int_choice("In 6 hours", 360)
        .add_int_choice("In 12 hours", 720).add_int_choice("In 24 hours", 1440);
    vec![
        CreateCommand::new("announcement").description("Post a Bliss SMP announcement")
            .add_option(date()).add_option(time())
            .add_option(CreateCommandOption::new(CommandOptionType::String, "news", "Announcement text").required(true)),
        CreateCommand::new("event").description("Post a Bliss SMP event")
            .add_option(CreateCommandOption::new(CommandOptionType::String, "name", "Event name").required(true))
            .add_option(date()).add_option(time())
            .add_option(CreateCommandOption::new(CommandOptionType::String, "prize", "Optional event prize").required(false)),
    ]
}

pub async fn handle_prefix(ctx: &Context, message: &Message, cfg: &GuildConfig, store: &CommunityStore, music: &MusicManager, premium: &PremiumClient, started: Instant) -> bool {
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
        "timeout-minutes" | "dm" | "set-log" | "purge" | "snipe" | "sticky" => Some("moderation"),
        "vibecheck" | "ship" | "fortune" | "wyr" => Some("fun"),
        "youtube" => Some("social"),
        "play" | "join" | "leave" | "pause" | "resume" | "skip" | "stop" | "queue" | "volume" | "lyrics" | "tts" => Some("music"),
        "commands" | "help" | "update" | "uptime" | "sync" | "tos" | "terms" | "terms-of-service" | "say" | "countdown" | "ticket" | "ticket-panel" | "ticket-close" | "backup" | "embed" | "vanity" | "custom-command" | "premium-status" | "announcement" | "event" | "rules" | "autorole" | "reaction-role" => Some("utility"),
        _ => None,
    };
    if cfg.disabled_commands.iter().any(|name| name == &command)
        || feature.and_then(|key| cfg.features.get(key).and_then(Value::as_bool)) == Some(false) {
        let _ = message.reply(&ctx.http, format!("The {command} command is disabled in this server.")).await;
        return true;
    }
    if let Some(response) = store.custom_command(guild_id, &command) {
        let _ = message.reply(&ctx.http, response.chars().take(1900).collect::<String>()).await;
        return true;
    }
    let premium_commands = ["embed", "snipe", "sticky", "backup", "custom-command", "vanity"];
    if ["tos", "terms", "terms-of-service"].contains(&command.as_str()) {
        let embed = CreateEmbed::new()
            .title("Mallow Bot Terms of Service")
            .description(include_str!("../../TERMS_OF_SERVICE.md").chars().take(4000).collect::<String>())
            .colour(serenity::all::Colour::new(0x5865F2));
        let _ = message.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed)).await;
        return true;
    }
    if command == "countdown" {
        let embed = halloween::build_embed();
        match message.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
            Ok(posted) => {
                if let Err(error) = halloween::set_target(guild_id, message.channel_id.get(), posted.id.get()) {
                    warn!(%error, guild_id, "Could not persist Halloween countdown");
                    let _ = message.reply(&ctx.http, "I created the countdown, but couldn't save its automatic refresh settings.").await;
                } else {
                    let _ = message.reply(&ctx.http, "Halloween countdown is set up and will refresh automatically, including after a bot restart.").await;
                }
            }
            Err(_) => { let _ = message.reply(&ctx.http, "I couldn't create the countdown. Check my channel permissions and try again.").await; }
        }
        return true;
    }
    if command == "premium-status" {
        let reply = if user_id == 1021068313071210498 {
            "Mallow Premium: developer access is enabled.".to_string()
        } else {
            let (banned, active, expires) = premium.status(user_id).await;
            if banned { "This account cannot use Mallow Bot.".to_string() }
            else if active { format!("Mallow Premium is active. Expires: {}.", expires.unwrap_or_else(|| "unknown".into())) }
            else { "Mallow Premium is not active for your Discord account. Get Premium at https://mallowbot.kz.ci/subscriptions.html".to_string() }
        };
        let _ = message.reply(&ctx.http, reply).await;
        return true;
    }
    if premium_commands.contains(&command.as_str()) && user_id != 1021068313071210498 {
        let (banned, active, _) = premium.status(user_id).await;
        if banned {
            let _ = message.reply(&ctx.http, "This account cannot use Mallow Bot.").await;
            return true;
        }
        if !active {
            let _ = message.reply(&ctx.http, "Mallow Premium is required. Get Premium at https://mallowbot.kz.ci/subscriptions.html").await;
            return true;
        }
    }
    if command == "embed" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Manage Messages permission to use this command.").await; return true; }
        let parts: Vec<&str> = argument.splitn(4, '|').map(str::trim).collect();
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            let _ = message.reply(&ctx.http, "Usage: -mb embed <title> | <description> [| #hexcolor] [| #channel or channel ID]").await;
            return true;
        }
        let color_text = parts.get(2).copied().filter(|v| !v.is_empty()).unwrap_or("#8B5CF6").trim_start_matches('#');
        let Ok(color) = u32::from_str_radix(color_text, 16) else {
            let _ = message.reply(&ctx.http, "Use a valid hex color such as #8B5CF6.").await;
            return true;
        };
        if color > 0xFFFFFF { let _ = message.reply(&ctx.http, "Use a valid hex color such as #8B5CF6.").await; return true; }
        let target = if let Some(value) = parts.get(3).copied().filter(|v| !v.is_empty()) {
            let id = value.trim_start_matches("<#").trim_end_matches('>').parse::<u64>().ok();
            id.map(serenity::all::ChannelId::new).unwrap_or(message.channel_id)
        } else { message.channel_id };
        let embed = CreateEmbed::new().title(parts[0].chars().take(256).collect::<String>())
            .description(parts[1].chars().take(4000).collect::<String>())
            .colour(serenity::all::Colour::new(color));
        let result = target.send_message(&ctx.http, CreateMessage::new().embed(embed)).await;
        let reply = if result.is_ok() { format!("Embed sent to <#{target}>.") } else { "I couldn't send the embed. Check my channel permissions.".into() };
        let _ = message.reply(&ctx.http, reply).await;
        return true;
    }
    if command == "snipe" {
        let entry = store.record("snipes", &message.channel_id.get().to_string());
        let embed = match entry {
            Some(value) => CreateEmbed::new().title("Recently deleted message")
                .description(value.get("content").and_then(Value::as_str).unwrap_or("[No text content]").chars().take(4000).collect::<String>())
                .field("Author", value.get("author").and_then(Value::as_str).unwrap_or("Unknown"), false)
                .colour(serenity::all::Colour::ORANGE),
            None => {
                let _ = message.reply(&ctx.http, "There is no recently deleted message saved for this channel.").await;
                return true;
            }
        };
        let _ = message.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed)).await;
        return true;
    }
    if command == "sticky" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Manage Messages permission to use this command.").await; return true; }
        let key = message.channel_id.get().to_string();
        if argument.is_empty() {
            store.set_record("sticky_messages", &key, None);
            let _ = message.reply(&ctx.http, "Sticky message cleared for this channel.").await;
        } else {
            let content: String = argument.chars().take(1800).collect();
            match message.channel_id.say(&ctx.http, &content).await {
                Ok(posted) => {
                    store.set_record("sticky_messages", &key, Some(json!({"guild_id":guild_id,"content":content,"message_id":posted.id.get()})));
                    let _ = message.reply(&ctx.http, "Sticky message set. Use -mb sticky without text to clear it.").await;
                }
                Err(_) => { let _ = message.reply(&ctx.http, "I couldn't post the sticky message. Check my channel permissions.").await; }
            }
        }
        return true;
    }
    if command == "custom-command" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Manage Server permission to use this command.").await; return true; }
        let mut parts = argument.splitn(3, char::is_whitespace);
        let action = parts.next().unwrap_or("").to_lowercase();
        let name: String = parts.next().unwrap_or("").to_lowercase().chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_').take(32).collect();
        let response = parts.next().unwrap_or("").trim();
        let reply = match action.as_str() {
            "list" => { let names = store.custom_commands(guild_id); format!("Custom commands: {}", if names.is_empty() { "none".into() } else { names.join(", ") }) }
            "delete" if !name.is_empty() && store.custom_command(guild_id, &name).is_some() => { store.set_custom_command(guild_id, &name, None); format!("Deleted custom command {name}.") }
            "create" if !name.is_empty() && !response.is_empty() => { store.set_custom_command(guild_id, &name, Some(&response.chars().take(1500).collect::<String>())); format!("Created custom command {name}. Members can use {} {name}.", cfg.prefix) }
            _ => "Usage: -mb custom-command create <name> <response> | list | delete <name>".into()
        };
        let _ = message.reply(&ctx.http, reply).await;
        return true;
    }
    if command == "vanity" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_guild()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Manage Server permission to use this command.").await; return true; }
        if !argument.is_empty() { store.set_record("vanity", &guild_id.to_string(), Some(json!(argument.chars().take(160).collect::<String>()))); }
        let current = store.record("vanity", &guild_id.to_string()).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "No tagline set.".into());
        let _ = message.reply(&ctx.http, format!("{} tagline: {current}", message.guild_id.unwrap())).await;
        return true;
    }
    if command == "backup" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.administrator()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Administrator permission to use this command.").await; return true; }
        let guild = serenity::all::GuildId::new(guild_id);
        match (guild.channels(&ctx.http).await, guild.roles(&ctx.http).await) {
            (Ok(channels), Ok(roles)) => {
                let payload = json!({
                    "guild_id": guild_id.to_string(),
                    "created_at": now_seconds(),
                    "channels": channels.values().map(|channel| json!({
                        "id": channel.id.get(), "name": channel.name, "type": format!("{:?}", channel.kind),
                        "position": channel.position, "parent_id": channel.parent_id.map(|id| id.get()), "topic": channel.topic
                    })).collect::<Vec<_>>(),
                    "roles": roles.values().filter(|role| role.id.get() != guild_id).map(|role| json!({
                        "id": role.id.get(), "name": role.name, "permissions": role.permissions.bits(),
                        "colour": role.colour.0, "position": role.position, "hoist": role.hoist, "mentionable": role.mentionable
                    })).collect::<Vec<_>>()
                });
                match serde_json::to_vec_pretty(&payload) {
                    Ok(bytes) => {
                        let file = CreateAttachment::bytes(bytes, format!("mallow-backup-{guild_id}.json"));
                        let result = message.channel_id.send_message(&ctx.http, CreateMessage::new()
                            .content("Server layout export attached. This is not a full message or permission restore backup.")
                            .add_file(file)).await;
                        if result.is_err() { let _ = message.reply(&ctx.http, "I couldn't attach the server backup. Check my channel permissions.").await; }
                    }
                    Err(_) => { let _ = message.reply(&ctx.http, "I couldn't serialize the server backup.").await; }
                }
            }
            _ => { let _ = message.reply(&ctx.http, "I couldn't read this server's channel and role layout. Check my permissions.").await; }
        }
        return true;
    }
    if command == "set-log" {
        if user_id != 1021068313071210498 { let _ = message.reply(&ctx.http, "Only the bot developer can configure DM logs.").await; return true; }
        let id = if argument.is_empty() { Some(message.channel_id.get()) } else {
            argument.trim_start_matches("<#").trim_end_matches('>').parse::<u64>().ok()
        };
        if let Some(channel) = id {
            store.set_record("log_channels", &guild_id.to_string(), Some(json!(channel)));
            let _ = message.reply(&ctx.http, format!("DM conversations will be logged to <#{channel}>.")).await;
        } else { let _ = message.reply(&ctx.http, "Usage: -mb set-log [#channel]").await; }
        return true;
    }
    if command == "dm" {
        let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_messages()).unwrap_or(false);
        if !permitted { let _ = message.reply(&ctx.http, "You need Manage Messages permission to use this command.").await; return true; }
        let Some(target) = message.mentions.first().filter(|user| !user.bot && user.id.get() != user_id) else {
            let _ = message.reply(&ctx.http, "Usage: -mb dm @member <message>").await; return true;
        };
        let text = argument.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim();
        let Some(log_channel) = store.record("log_channels", &guild_id.to_string()).and_then(|v| v.as_u64()) else {
            let _ = message.reply(&ctx.http, "This server needs a DM log channel. Ask the bot developer to configure it with -mb set-log.").await; return true;
        };
        if text.is_empty() || text.len() > 1900 { let _ = message.reply(&ctx.http, "Provide a message of 1,900 characters or fewer.").await; return true; }
        let dm_result = match target.id.create_dm_channel(&ctx.http).await {
            Ok(channel) => channel.say(&ctx.http, text).await.map(|_| ()),
            Err(error) => Err(error),
        };
        if dm_result.is_err() { let _ = message.reply(&ctx.http, "I couldn't DM that member. They may have DMs disabled.").await; return true; }
        store.set_record("dm_routes", &target.id.get().to_string(), Some(json!({"guild_id":guild_id,"channel_id":log_channel,"set_by":user_id,"updated_at":now_seconds()})));
        let _ = message.reply(&ctx.http, format!("Sent a DM to <@{}>. Their replies will be forwarded only to <#{log_channel}>.", target.id.get())).await;
        return true;
    }
    if command == "youtube" {
        let candidate = argument.split_whitespace().find(|part| part.contains("youtube.com") || part.contains("youtu.be")).unwrap_or("");
        let video_id = if let Some(id) = candidate.split("youtu.be/").nth(1) { id.split(['?', '&']).next().unwrap_or("") }
            else if let Some(id) = candidate.split("v=").nth(1) { id.split('&').next().unwrap_or("") }
            else if let Some(id) = candidate.split("/shorts/").nth(1) { id.split(['?', '&']).next().unwrap_or("") } else { "" };
        if video_id.len() != 11 {
            let _ = message.reply(&ctx.http, "Usage: -mb youtube <YouTube URL>").await;
            return true;
        }
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        let result = reqwest::Client::new().get("https://www.youtube.com/oembed").query(&[("url", url.as_str()), ("format", "json")]).send().await;
        let reply = match result {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(info) => CreateEmbed::new().title(info.get("title").and_then(Value::as_str).unwrap_or("YouTube video"))
                    .url(&url).field("Channel", info.get("author_name").and_then(Value::as_str).unwrap_or("Unknown"), false)
                    .colour(serenity::all::Colour::RED),
                _ => { let _ = message.reply(&ctx.http, "YouTube returned invalid video information.").await; return true; }
            },
            _ => { let _ = message.reply(&ctx.http, "I couldn't look up that video right now.").await; return true; }
        };
        let _ = message.channel_id.send_message(&ctx.http, CreateMessage::new().embed(reply)).await;
        return true;
    }
    let starting = cfg.economy_config.get("starting_money").and_then(Value::as_i64).unwrap_or(100);
    let response = match command.as_str() {
        "uptime" => {
            let elapsed = started.elapsed();
            format!("I have been online for {}d {:02}h {:02}m {:02}s.",
                elapsed.as_secs() / 86_400,
                (elapsed.as_secs() / 3_600) % 24,
                (elapsed.as_secs() / 60) % 60,
                elapsed.as_secs() % 60)
        }
        "sync" => {
            let permitted = message.member.as_ref().and_then(|member| member.permissions).map(|p| p.administrator()).unwrap_or(false);
            if !permitted {
                "You need Administrator permission to use this command.".into()
            } else {
                match Command::set_global_commands(&ctx.http, crate::registered_global_commands()).await {
                    Ok(synced) => {
                        let guild = GuildId::new(guild_id);
                        // Register this server's commands as guild commands too. Guild
                        // commands become available immediately instead of waiting for
                        // Discord's global-command propagation window.
                        let mut guild_commands = crate::registered_global_commands();
                        if guild_id == 1544628300217978932 {
                            guild_commands.extend(bliss_commands());
                        }
                        let guild_result = guild.set_commands(&ctx.http, guild_commands).await;
                        if guild_result.is_err() {
                            "I could not fully reset this server's duplicate commands. Check my application-command permissions and try again.".into()
                        } else {
                            let premium_names = ["embed", "snipe", "sticky", "backup", "custom-command", "vanity", "premium-status"];
                            let premium_count = synced.iter().filter(|item| premium_names.contains(&item.name.as_str())).count();
                            let standard_count = synced.len().saturating_sub(premium_count);
                            format!("Synced {standard_count} commands and {premium_count} Premium commands. Global slash-command updates can take a little while to appear.")
                        }
                    }
                    Err(_) => "I could not refresh the application commands. Check my application-command permissions and try again.".into()
                }
            }
        }

        "play" => {
            if let Some(channel) = music.voice_channel_for_user(guild_id, user_id).await {
                // Avoid rejoining the same voice channel on every /play or prefix play;
                // a redundant voice-state update can make Lavalink restart the current song.
                if music.bot_voice_channel(guild_id).await != Some(channel) {
                    music.request_voice_channel(guild_id, Some(channel)).await;
                }
                match music.enqueue(guild_id, argument, user_id).await { Ok(text) => text, Err(error) => format!("Music error: {error}") }
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
            if let Some(remaining) = store.try_cooldown("work", guild_id, user_id, 3600) { format!("You can work again in {} minute(s).", (remaining + 59) / 60) }
            else {
            let min = cfg.economy_config.get("work_min").and_then(Value::as_i64).unwrap_or(20);
            let max = cfg.economy_config.get("work_max").and_then(Value::as_i64).unwrap_or(80).max(min);
            let earned = min + pseudo_roll((max - min + 1) as u64) as i64;
            let next = store.balance(guild_id, user_id, starting) + earned;
            store.set_balance(guild_id, user_id, next);
            format!("You worked a shift and earned {earned} Mallow Coins. Balance: {next}.")
            }
        }
        "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" => {
            let bet = argument.split_whitespace().next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(10);
            let balance = store.balance(guild_id, user_id, starting);
            if bet < 1 || bet > balance { format!("Bet must be between 1 and your balance ({balance}).") }
            else {
                let (payout, detail) = match command.as_str() {
                    "slots" => {
                        let a = pseudo_roll(5); let b = pseudo_roll(5); let c = pseudo_roll(5);
                        let symbols = ["cherry", "lemon", "grape", "bell", "seven"];
                        let payout = if a == b && b == c { bet * 5 } else if a == b || b == c || a == c { bet * 2 } else { 0 };
                        (payout, format!("{} · {} · {}", symbols[a as usize], symbols[b as usize], symbols[c as usize]))
                    }
                    "roulette" => { let roll = pseudo_roll(37); (if roll % 2 == 0 { bet * 2 } else { 0 }, format!("Roulette landed on {roll} ({}).", if roll % 2 == 0 { "even" } else { "odd" })) }
                    "dice" => { let roll = pseudo_roll(6) + 1; (if roll >= 4 { bet * 2 } else { 0 }, format!("You rolled {roll}.")) }
                    "blackjack" => { let player = 12 + pseudo_roll(10); let dealer = 15 + pseudo_roll(7); (if player > dealer { bet * 2 } else { 0 }, format!("You: {player} · Dealer: {dealer}.")) }
                    _ => { let won = pseudo_roll(2) == 0; (if won { bet * 2 } else { 0 }, if won { "Heads!".into() } else { "Tails!".into() }) }
                };
                let next = balance - bet + payout;
                store.set_balance(guild_id, user_id, next);
                format!("{detail} {} Balance: {next} Mallow Coins.", if payout > 0 { format!("Won {payout} Mallow Coins.") } else { format!("Lost {bet} Mallow Coins.") })
            }
        }
        "fortune" => {
            let fortunes = ["A plot twist is closer than you think.", "Today, your side quest becomes the main story.", "The next message you send will be oddly important.", "A small win is loading.", "Your future contains snacks and questionable decisions.", "The universe says yes, but asks you to read the patch notes first.", "Someone in this server owes you a high five.", "Avoid suspiciously confident people bearing spoilers."];
            format!("Your Mallow Bot fortune: {}", fortunes[pseudo_roll(fortunes.len() as u64) as usize])
        }
        "wyr" => {
            let questions = ["Would you rather pause time or rewind it by one minute?", "Would you rather explore the deepest ocean or travel to the edge of space?", "Would you rather speak every language or play every instrument?", "Would you rather have unlimited free travel or a fully paid dream home?", "Would you rather know every answer or solve any puzzle?", "Would you rather live one year in the past or one year in the future?"];
            questions[pseudo_roll(questions.len() as u64) as usize].to_string()
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
            if let Some(target) = message.mentions.first().filter(|user| !user.bot).map(|user| user.id.get()) {
                let target_balance = store.balance(guild_id, target, starting);
                if target == user_id { "You cannot rob yourself.".into() }
                else if target_balance < 1 { "That member has no Mallow Coins to steal.".into() }
                else if pseudo_roll(100) < 35 {
                    let stolen = (target_balance / 4).max(1).min(target_balance);
                    let own = store.balance(guild_id, user_id, starting);
                    store.set_balance(guild_id, target, target_balance - stolen);
                    store.set_balance(guild_id, user_id, own + stolen);
                    format!("You stole {stolen} Mallow Coins from <@{target}>.")
                } else {
                    let own = store.balance(guild_id, user_id, starting);
                    let fine = own.min(25);
                    store.set_balance(guild_id, user_id, own - fine);
                    format!("The robbery failed. You paid a {fine}-coin fine.")
                }
            } else { "Mention a member to rob. Usage: -mb rob @member".into() }
        }
        "vibecheck" => {
            let target = message.mentions.first().map(|user| user.name.as_str()).unwrap_or(message.author.name.as_str());
            let score = pseudo_roll(101);
            let verdict = if score >= 90 { "legendary energy" } else if score >= 70 { "main-character energy" } else if score >= 40 { "mysterious side-character energy" } else { "needs a snack and a nap" };
            format!("Vibe scan for **{target}**: **{score}/100** — {verdict}.")
        },
        "ship" => {
            let targets = &message.mentions;
            let (first, second) = if targets.len() >= 2 { (&targets[0], &targets[1]) }
                else if let Some(target) = targets.first() { (&message.author, target) }
                else { return false };
            if first.bot || second.bot || first.id == second.id { "Choose two different human members. This is just for fun!".into() }
            else {
                let score = pseudo_roll(101);
                let verdict = if score >= 90 { "cinematic soulmates" } else if score >= 70 { "power couple material" } else if score >= 40 { "chaotic duo" } else { "unlikely pairing" };
                format!("**{} + {}** = **{score}%** — {verdict}. (For fun only!)", first.name, second.name)
            }
        },
        "help" | "commands" => "Music: play, join, leave, pause, resume, skip, queue, volume, lyrics, stop.\nCommunity: xp, level, leaderboard, swears, swear-leaderboard, balance, daily, work, gamble, coinflip, slots, roulette, dice, blackjack, pay, rob.\nFun: vibecheck, ship, fortune, wyr.\nUtilities: uptime, update, sync, tos, countdown, commands, youtube, dm, set-log, timeout-minutes, purge, ticket, ticket-panel, ticket-close, embed, snipe, sticky, backup, custom-command, vanity, premium-status.".into(),
        "update" => match reqwest::get("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/updates.txt").await {
            Ok(response) if response.status().is_success() => response.text().await.unwrap_or_else(|_| "No update log is available right now.".into()).chars().take(1900).collect::<String>(),
            _ => "No update log is available right now.".into()
        },
        _ => return false,
    };
    let music_command = ["play","join","leave","pause","resume","skip","stop","queue","volume","lyrics","tts"].contains(&command.as_str());
    if music_command {
        let title = match command.as_str() {
            "play" => "NOW PLAYING / QUEUED",
            "join" => "VOICE CHANNEL",
            "leave" => "MUSIC DISCONNECTED",
            "pause" => "PLAYBACK PAUSED",
            "resume" => "PLAYBACK RESUMED",
            "skip" => "TRACK SKIPPED",
            "stop" => "PLAYBACK STOPPED",
            "queue" => "UP NEXT",
            "volume" => "VOLUME UPDATED",
            "lyrics" => "LYRICS SETTINGS",
            "tts" => "VOICE TTS",
            _ => "MALLOW MUSIC",
        };
        let embed = CreateEmbed::new()
            .title(format!("MALLOW MUSIC  //  {title}"))
            .description(response.chars().take(4000).collect::<String>())
            .colour(serenity::all::Colour::new(0x8B5CF6))
            .footer(serenity::builder::CreateEmbedFooter::new(format!("Requested by {}", message.author.name)));
        if let Err(error) = message.channel_id.send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
            warn!(%error, command, "Could not send public music embed");
        }
    } else if let Err(error) = message.reply(&ctx.http, response).await {
        warn!(%error, command, "Could not respond to prefix command");
    }
    true
}

pub async fn export_ticket_transcript(http: &Http, channel_id: serenity::all::ChannelId, cfg: &GuildConfig) -> Result<(), String> {
    let Some(destination) = cfg.tickets_config.get("transcript_channel_id").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))) else { return Ok(()) };
    let messages = channel_id.messages(http, GetMessages::new().limit(100)).await.map_err(|_| "could not read ticket history".to_string())?;
    let mut messages = messages;
    messages.reverse();
    let mut transcript = String::new();
    for message in &messages {
        transcript.push_str(&format!("[{}] {}: {}\n", message.timestamp, message.author.name, message.content));
        for attachment in &message.attachments {
            transcript.push_str(&format!("  attachment: {}\n", attachment.url));
        }
    }
    if transcript.is_empty() { transcript.push_str("This ticket had no messages.\n"); }
    let bytes = transcript.into_bytes();
    let attachment = CreateAttachment::bytes(bytes, format!("ticket-{}.txt", channel_id.get()));
    serenity::all::ChannelId::new(destination).send_message(http, CreateMessage::new()
        .content(format!("Ticket transcript for <#{}>.", channel_id.get()))
        .add_file(attachment)).await.map_err(|_| "could not send ticket transcript".to_string())?;
    Ok(())
}

pub async fn create_ticket_channel(http: &Http, guild_id: u64, user_id: u64, username: &str, cfg: &GuildConfig, store: &CommunityStore) -> Result<u64, String> {
    if cfg.tickets_config.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err("Tickets are disabled for this server.".into());
    }
    if let Some(existing) = store.existing_ticket(guild_id, user_id) {
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
        for role in roles.iter().filter_map(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))).take(20) {
            overwrites.push(PermissionOverwrite { allow: Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY, deny: Permissions::empty(), kind: PermissionOverwriteType::Role(RoleId::new(role)) });
        }
    }
    let slug: String = username.to_lowercase().chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-').take(40).collect();
    let mut builder = CreateChannel::new(format!("ticket-{}", if slug.is_empty() { user_id.to_string() } else { slug }))
        .kind(ChannelType::Text)
        .permissions(overwrites)
        .topic(format!("mallow-ticket:{user_id}:{guild_id}"))
        .audit_log_reason("Mallow Bot support ticket");
    if let Some(category) = cfg.tickets_config.get("category_id").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))) {
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

fn parse_reaction_type(value: &str) -> ReactionType {
    let trimmed = value.trim();
    if trimmed.starts_with("<:") || trimmed.starts_with("<a:") {
        let animated = trimmed.starts_with("<a:");
        let inner = trimmed.trim_start_matches('<').trim_end_matches('>');
        let mut parts = inner.split(':');
        let _kind = parts.next();
        let name = parts.next().unwrap_or("emoji");
        let id = parts.next().and_then(|v| v.parse::<u64>().ok());
        if let Some(id) = id {
            return ReactionType::Custom { animated, id: EmojiId::new(id), name: Some(name.to_string()) };
        }
    }
    ReactionType::Unicode(trimmed.to_string())
}

fn level(xp: i64) -> i64 { ((xp.max(0) as f64 / 100.0).sqrt()) as i64 }

fn normalize_swear_phrase(value: &str) -> Vec<String> {
    let mut mapped = String::new();
    for raw in value.nfkd().flat_map(char::to_lowercase).filter(|ch| !is_combining_mark(*ch)) {
        // Do not treat ordinary punctuation such as ! or | as leetspeak letters.
        // Doing so caused harmless messages to normalize into configured swear words.
        let ch = match raw {
            '0' => 'o',
            '1' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            '7' => 't',
            '8' => 'b',
            '9' => 'g',
            '€' => 'e',
            other => other,
        };
        if ch.is_alphanumeric() {
            mapped.push(ch);
        } else if ch.is_whitespace() {
            mapped.push(' ');
        }
    }
    mapped.split_whitespace().map(str::to_owned).collect()
}

pub fn count_swear_hits(content: &str, configured_words: &[String]) -> i64 {
    let words = normalize_swear_phrase(content);
    let mut phrases: Vec<Vec<String>> = configured_words.iter()
        .map(|term| normalize_swear_phrase(term))
        .filter(|phrase| !phrase.is_empty())
        .collect();
    phrases.sort_by(|a, b| b.len().cmp(&a.len()));
    phrases.dedup();
    let mut hits = 0_i64;
    let mut cursor = 0usize;
    while cursor < words.len() {
        if let Some(phrase) = phrases.iter().find(|phrase| words[cursor..].starts_with(phrase.as_slice())) {
            hits += 1;
            cursor += phrase.len();
        } else { cursor += 1; }
    }
    hits
}

fn now_seconds() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn pseudo_roll(max: u64) -> u64 {
    if max == 0 { 0 } else { rand::Rng::gen_range(&mut rand::thread_rng(), 0..max) }
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
        "dm" | "set-log" | "purge" | "snipe" | "sticky" | "rule" | "autorole" | "reaction-role" => Some("moderation"),
        "play" | "join" | "leave" | "pause" | "resume" | "skip" | "stop" | "queue" | "volume" | "lyrics" => Some("music"),
        "uptime" | "update" | "commands" | "sync" | "tos" | "say" | "ticket" | "ticket-panel" | "ticket-close" | "backup" | "embed" | "custom-command" | "vanity" | "premium-status" | "announcement" | "event" => Some("utility"),
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
            match reqwest::get("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/updates.txt").await {
                Ok(response) if response.status().is_success() => {
                    match response.text().await {
                        Ok(update) if !update.trim().is_empty() => {
                            let mut sent = true;
                            let mut remaining = update.as_str();
                            while !remaining.is_empty() {
                                let mut cut = remaining.len().min(1800);
                                while !remaining.is_char_boundary(cut) { cut -= 1; }
                                if let Some(boundary) = remaining[..cut].rfind('\n') { if boundary > 400 { cut = boundary; } }
                                if command.channel_id.say(http, remaining[..cut].to_string()).await.is_err() { sent = false; break; }
                                remaining = remaining[cut..].trim_start();
                            }
                            if sent { "Posted the update log.".into() } else { "I couldn't post the full update log.".into() }
                        }
                        _ => "No update log is available right now.".into()
                    }
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
            if let Some(remaining) = store.try_cooldown("work", guild_id, user_id, 3600) { format!("You can work again in {} minute(s).", (remaining + 59) / 60) }
            else {
            let min = cfg.economy_config.get("work_min").and_then(Value::as_i64).unwrap_or(20);
            let max = cfg.economy_config.get("work_max").and_then(Value::as_i64).unwrap_or(80).max(min);
            let earned = min + pseudo_roll((max - min + 1) as u64) as i64;
            let value = store.balance(guild_id, user_id, starting) + earned;
            store.set_balance(guild_id, user_id, value);
            format!("You worked a shift and earned {earned} Mallow Coins. Balance: {value}.")
            }
        }
        "gamble" | "coinflip" | "slots" | "roulette" | "dice" | "blackjack" => {
            let bet = command.data.options.iter().find_map(|o| if o.name == "bet" { if let CommandDataOptionValue::Integer(value) = &o.value { Some(*value) } else { None } } else { None }).unwrap_or(10);
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
            let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = &o.value { Some(id.get()) } else { None } } else { None });
            let amount = command.data.options.iter().find_map(|o| if o.name == "amount" { if let CommandDataOptionValue::Integer(value) = &o.value { Some(*value) } else { None } } else { None }).unwrap_or(0);
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
            let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = &o.value { Some(id.get()) } else { None } } else { None });
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
        "wyr" => {
            let questions = [
                "Would you rather be able to pause time or rewind it by one minute?",
                "Would you rather explore the deepest ocean or travel to the edge of space?",
                "Would you rather speak every language or play every instrument?",
                "Would you rather have unlimited free travel or a fully paid dream home?",
                "Would you rather know the answer to every question or be able to solve any puzzle?",
                "Would you rather live one year in the past or one year in the future?",
                "Would you rather have a personal chef or a personal driver?",
                "Would you rather always be ten minutes early or never wait in a queue again?",
            ];
            questions[pseudo_roll(questions.len() as u64) as usize].to_string()
        },
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
                    "list" => { let items = store.custom_commands(guild_id); if items.is_empty() { "No custom commands are configured.".into() } else { items.iter().map(|name| format!("-mb {name}")).collect::<Vec<_>>().join("\\n") } },
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
                let option_text = |name: &str| command.data.options.iter().find_map(|option| {
                    if option.name == name { if let CommandDataOptionValue::String(value) = &option.value { Some(value.as_str()) } else { None } } else { None }
                });
                let title = option_text("title").unwrap_or("Mallow Bot");
                let description = option_text("description").unwrap_or("");
                let color_text = option_text("color").unwrap_or("#8B5CF6").trim_start_matches('#');
                let color = u32::from_str_radix(color_text, 16).ok().filter(|value| *value <= 0xFFFFFF);
                if color.is_none() { "Use a valid hex color such as #8B5CF6.".into() }
                else {
                    let target = command.data.options.iter().find_map(|option| {
                        if option.name == "channel" { if let CommandDataOptionValue::Channel(id) = &option.value { Some(*id) } else { None } } else { None }
                    }).unwrap_or(command.channel_id);
                    let embed = CreateEmbed::new().title(title.chars().take(256).collect::<String>())
                        .description(description.chars().take(4000).collect::<String>())
                        .colour(serenity::all::Colour::new(color.unwrap()));
                    match target.send_message(http, CreateMessage::new().embed(embed)).await {
                        Ok(_) => format!("Embed sent to <#{target}>."),
                        Err(_) => "I couldn't send the embed. Check my channel permissions.".into()
                    }
                }
            }
        }
        "set-log" => {
            if user_id != 1021068313071210498 { "Only the bot developer can configure DM logs.".into() }
            else {
                let channel = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Channel(id) = &o.value { Some(id.get()) } else { None });
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
                let target = command.data.options.iter().find_map(|o| if o.name == "member" { if let CommandDataOptionValue::User(id) = &o.value { Some(*id) } else { None } } else { None });
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
                let amount = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Integer(value) = &o.value { Some(*value) } else { None }).unwrap_or(0).clamp(1,100) as u8;
                match command.channel_id.messages(http, GetMessages::new().limit(amount)).await {
                    Ok(messages) if !messages.is_empty() => {
                        let mut deleted = 0usize;
                        for message in messages {
                            if message.delete(http).await.is_ok() { deleted += 1; }
                        }
                        format!("Deleted {deleted} message(s).")
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
                let role = command.data.options.iter().find_map(|o| if let CommandDataOptionValue::Role(id) = &o.value { Some(id.get()) } else { None });
                match action {
                    "set" => if let Some(role) = role { store.set_autorole(guild_id, Some(role)); format!("Auto-role enabled. New members will receive <@&{role}>.") } else { "Choose a role to assign.".into() },
                    "disable" => { store.set_autorole(guild_id, None); "Auto-role has been disabled for this server.".into() },
                    _ => store.autorole(guild_id).map(|id| format!("New members will receive <@&{id}>.")).unwrap_or_else(|| "Auto-role is not configured. Use /autorole action:Set to choose a role.".into())
                }
            }
        }

        "rule" => {
            let permissions = command.member.as_ref().and_then(|member| member.permissions).unwrap_or_default();
            if !permissions.manage_roles() || !permissions.manage_channels() {
                "You need Manage Roles and Manage Channels permissions to configure verification.".into()
            } else {
                let option_bool = |key: &str| command.data.options.iter().find_map(|option| {
                    if option.name == key { if let CommandDataOptionValue::Boolean(value) = &option.value { Some(*value) } else { None } } else { None }
                });
                let option_string = |key: &str| command.data.options.iter().find_map(|option| {
                    if option.name == key { if let CommandDataOptionValue::String(value) = &option.value { Some(value.as_str()) } else { None } } else { None }
                });
                let make_role = option_bool("make_verified_role").unwrap_or(true);
                let lock_channels = option_bool("setup_channels").unwrap_or(false);
                let message_id = option_string("message_id").and_then(|value| value.parse::<u64>().ok());
                let emoji = option_string("reaction_emoji").unwrap_or("").trim();
                let role_name = option_string("role_name").unwrap_or("Verified").trim();
                let color_text = option_string("role_color").unwrap_or("").trim();
                let color_value = match color_text.to_ascii_lowercase().as_str() {
                    "red" => Some(0xED4245), "green" => Some(0x57F287), "blue" => Some(0x5865F2),
                    "yellow" => Some(0xFEE75C), "purple" => Some(0x9B59B6), "orange" => Some(0xE67E22),
                    "pink" => Some(0xEB459E), "black" => Some(0x23272A), "white" => Some(0xFFFFFF),
                    _ => u32::from_str_radix(color_text.trim_start_matches('#'), 16).ok().filter(|value| *value <= 0xFFFFFF),
                };
                if message_id.is_none() || emoji.is_empty() || emoji.len() > 100 || role_name.is_empty() || role_name.len() > 100 || color_value.is_none() {
                    "Provide a valid message ID, emoji, role name, and color (named color or hex such as #5865F2).".into()
                } else {
                    let guild = GuildId::new(guild_id);
                    let anchor_id = RoleId::new(1545275679845785640_u64);
                    match guild.roles(http).await {
                        Err(_) => "I couldn't read this server's roles. Check my Manage Roles permission.".into(),
                        Ok(existing_roles) => {
                            if !existing_roles.contains_key(&anchor_id) {
                                "The role that Verified must sit above (1545275679845785640) was not found in this server.".into()
                            } else {
                                let existing_verified = existing_roles.values().find(|role| role.name.eq_ignore_ascii_case(role_name)).map(|role| role.id);
                                let role_result = if make_role {
                                    guild.create_role(http, serenity::builder::EditRole::new()
                                        .name(role_name).colour(serenity::all::Colour::new(color_value.unwrap())))
                                        .await.map(|role| role.id)
                                } else {
                                    existing_verified.ok_or(serenity::Error::Other("verified role not found"))
                                };
                                match role_result {
                                    Err(_) => "I couldn't create or find the verified role. Make sure Mallow's highest role is high enough.".into(),
                                    Ok(role_id) => {
                                        let anchor_position = existing_roles.get(&anchor_id).map(|role| role.position).unwrap_or(0);
                                        if let Err(error) = guild.edit_role_position(http, role_id, anchor_position.saturating_add(1)).await {
                                            warn!(%error, role_id = role_id.get(), "Could not position the verified role");
                                        }
                                        let message_id = message_id.unwrap();
                                        let message = command.channel_id.message(http, serenity::all::MessageId::new(message_id)).await;
                                        match message {
                                            Err(_) => "I couldn't find that message in this channel. Run /rule in the channel containing your rules message.".into(),
                                            Ok(_) => {
                                                if command.channel_id.create_reaction(http, serenity::all::MessageId::new(message_id), parse_reaction_type(emoji)).await.is_err() {
                                                    "I couldn't add the verification reaction. Check Add Reactions and Read Message History permissions.".into()
                                                } else {
                                                    store.set_rule(guild_id, message_id, command.channel_id.get(), emoji, role_id.get(), true);
                                                    let mut locked = 0usize;
                                                    let mut lock_failures = 0usize;
                                                    if lock_channels {
                                                        match guild.channels(http).await {
                                                            Ok(channels) => {
                                                                for channel in channels.values() {
                                                                    let overwrite = PermissionOverwrite {
                                                                        allow: Permissions::empty(),
                                                                        deny: Permissions::VIEW_CHANNEL | Permissions::CONNECT,
                                                                        kind: PermissionOverwriteType::Role(RoleId::new(guild_id)),
                                                                    };
                                                                    let everyone_result = channel.create_permission(http, overwrite).await;
                                                                    let verified_overwrite = PermissionOverwrite {
                                                                        allow: Permissions::VIEW_CHANNEL | Permissions::CONNECT | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY,
                                                                        deny: Permissions::empty(),
                                                                        kind: PermissionOverwriteType::Role(role_id),
                                                                    };
                                                                    let verified_result = channel.create_permission(http, verified_overwrite).await;
                                                                    if everyone_result.is_ok() && verified_result.is_ok() { locked += 1; }
                                                                    else { lock_failures += 1; }
                                                                }
                                                            }
                                                            Err(_) => { lock_failures += 1; }
                                                        }
                                                    }
                                                    let channel_note = if lock_channels {
                                                        format!(" Locked {locked} channels for @everyone{}.", if lock_failures > 0 { format!("; {lock_failures} failed") } else { String::new() })
                                                    } else { " Channel permissions were left unchanged.".into() };
                                                    format!("Verification configured. React with {emoji} on message {message_id} to receive <@&{}>. Role: **{}**.{} The role is positioned above <@&1545275679845785640>.", role_id.get(), role_name, channel_note)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },

        "reaction-role" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|p| p.manage_roles()).unwrap_or(false);
            if !permitted { "You need Manage Roles permission to use this command.".into() }
            else {
                let action = command.data.options.iter().find_map(|o| if o.name == "action" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("");
                let message_id = command.data.options.iter().find_map(|o| if o.name == "message_id" { if let CommandDataOptionValue::String(v) = &o.value { v.parse::<u64>().ok() } else { None } } else { None });
                let emoji = command.data.options.iter().find_map(|o| if o.name == "emoji" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.clone()) } else { None } } else { None }).unwrap_or_default();
                let role = command.data.options.iter().find_map(|o| if o.name == "role" { if let CommandDataOptionValue::Role(id) = &o.value { Some(id.get()) } else { None } } else { None });
                match action {
                    "create" => {
                        let channel = command.data.options.iter().find_map(|o| if o.name == "channel" { if let CommandDataOptionValue::Channel(id) = &o.value { Some(*id) } else { None } } else { None }).unwrap_or(command.channel_id);
                        let title = command.data.options.iter().find_map(|o| if o.name == "title" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("Choose your roles");
                        let description = command.data.options.iter().find_map(|o| if o.name == "description" { if let CommandDataOptionValue::String(v) = &o.value { Some(v.as_str()) } else { None } } else { None }).unwrap_or("React below to get a role. Remove your reaction to remove it.");
                        match channel.send_message(http, CreateMessage::new().embed(CreateEmbed::new().title(title).description(description))).await {
                            Ok(posted) => {
                                store.set_record("reaction_roles", &posted.id.get().to_string(), Some(json!({"guild_id":guild_id.to_string(),"channel_id":channel.get().to_string(),"roles":{}})));
                                format!("Reaction-role panel created: https://discord.com/channels/{guild_id}/{}/{}", channel.get(), posted.id.get())
                            }
                            Err(_) => "I couldn't post in that channel. Check my channel permissions.".into()
                        }
                    }
                    "add" => {
                        if let (Some(message_id), Some(role)) = (message_id, role) {
                            let panel = store.record("reaction_roles", &message_id.to_string());
                            let channel = panel.as_ref().and_then(|v| v.get("channel_id")).and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok());
                            let panel_guild = panel.as_ref().and_then(|v| v.get("guild_id")).and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok());
                            if emoji.is_empty() || channel.is_none() || panel_guild != Some(guild_id) { "That panel or emoji was not found in this server.".into() }
                            else if serenity::all::ChannelId::new(channel.unwrap()).create_reaction(http, serenity::all::MessageId::new(message_id), parse_reaction_type(&emoji)).await.is_err() {
                                "I couldn't add that reaction to the panel.".into()
                            } else {
                                store.set_reaction_role(guild_id, message_id, &emoji, Some(role));
                                format!("Added {emoji} → <@&{role}>.")
                            }
                        } else { "Provide a panel message ID, emoji, and role.".into() }
                    }
                    "remove" => {
                        if let Some(message_id) = message_id {
                            let panel = store.record("reaction_roles", &message_id.to_string());
                            let panel_guild = panel.as_ref().and_then(|v| v.get("guild_id")).and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok());
                            if emoji.is_empty() || panel_guild != Some(guild_id) { "That panel or emoji was not found in this server.".into() }
                            else { store.set_reaction_role(guild_id, message_id, &emoji, None); format!("Removed the {emoji} role mapping.") }
                        } else { "Provide the panel message ID and emoji.".into() }
                    }
                    "list" => {
                        if let Some(message_id) = message_id {
                            let panel = store.record("reaction_roles", &message_id.to_string());
                            let panel_guild = panel.as_ref().and_then(|v| v.get("guild_id")).and_then(Value::as_str).and_then(|v| v.parse::<u64>().ok());
                            if panel_guild != Some(guild_id) { "That panel was not found in this server.".into() }
                            else {
                                let roles = panel.and_then(|v| v.get("roles").cloned()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
                                if roles.is_empty() { "This panel has no role mappings yet.".into() }
                                else { roles.iter().map(|(emoji, role)| format!("{emoji} → <@&{}>", role.as_u64().unwrap_or(0))).collect::<Vec<_>>().join("\\n") }
                            }
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
                let days = command.data.options.iter().find_map(|o| if o.name == "date" { if let CommandDataOptionValue::Integer(v) = &o.value { Some(*v) } else { None } } else { None }).unwrap_or(0).max(0) as u64;
                let minutes = command.data.options.iter().find_map(|o| if o.name == "time" { if let CommandDataOptionValue::Integer(v) = &o.value { Some(*v) } else { None } } else { None }).unwrap_or(5).max(0) as u64;
                let timestamp = now_seconds() + days * 86_400 + minutes * 60;
                let channel = serenity::all::ChannelId::new(1544630281485549589);
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
            else if cfg.tickets_config.get("enabled").and_then(Value::as_bool) != Some(true) { "Tickets are disabled for this server. Enable them in the Mallow dashboard first.".into() }
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
            let key = command.channel_id.get().to_string();
            if let Some(ticket) = store.record("tickets", &key) {
                let owner = ticket.get("user_id").and_then(Value::as_u64) == Some(user_id);
                let permissions = command.member.as_ref().and_then(|member| member.permissions).unwrap_or_default();
                let manage_channels = permissions.manage_channels();
                let support_roles: Vec<u64> = cfg.tickets_config.get("support_role_ids").and_then(Value::as_array).into_iter().flatten()
                    .filter_map(|v| v.as_str().and_then(|s| s.parse::<u64>().ok())).collect();
                let is_support = command.member.as_ref().map(|member| member.roles.iter().any(|role| support_roles.contains(&role.get()))).unwrap_or(false);
                if !owner && !manage_channels && !is_support { "Only the ticket owner or support staff can close this ticket.".into() }
                else {
                    if let Err(error) = export_ticket_transcript(http, command.channel_id, cfg).await { warn!(%error, channel_id = command.channel_id.get(), "Could not export ticket transcript"); }
                    match command.channel_id.delete(http).await {
                        Ok(_) => { store.set_record("tickets", &key, None); "Ticket closed.".into() }
                        Err(_) => "I couldn't close this ticket. Check my Manage Channels permission.".into()
                    }
                }
            } else { "This channel is not a ticket created by Mallow Bot.".into() }
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_boundaries_match_the_existing_progression() {
        assert_eq!(level(0), 0);
        assert_eq!(level(99), 0);
        assert_eq!(level(100), 1);
        assert_eq!(level(399), 1);
        assert_eq!(level(400), 2);
    }

    #[test]
    fn swear_counter_handles_leetspeak_and_punctuation() {
        let words = vec!["bad".to_string(), "bad word".to_string()];
        assert_eq!(count_swear_hits("B.A.D and b4d", &words), 2);
    }

    #[test]
    fn swear_counter_counts_overlapping_phrases_once() {
        let words = vec!["bad".to_string(), "bad word".to_string()];
        assert_eq!(count_swear_hits("bad word bad", &words), 2);
    }

    #[test]
    fn swear_counter_does_not_match_substrings_or_normal_everyday_words() {
        let configured = vec!["fuck".to_string(), "shit".to_string()];
        assert_eq!(count_swear_hits("class assignment, a fan, a pic, and a picnic", &configured), 0);
        assert_eq!(count_swear_hits("this is fuck and shit", &configured), 2);
        assert_eq!(count_swear_hits("this is fucking awesome", &configured), 0);
    }

    #[test]
    fn random_rolls_stay_inside_the_requested_range() {
        assert_eq!(pseudo_roll(0), 0);
        for _ in 0..100 {
            assert!(pseudo_roll(7) < 7);
        }
    }
}
