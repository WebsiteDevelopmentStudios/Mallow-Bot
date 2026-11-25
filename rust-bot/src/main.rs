mod config;
mod community;
mod dashboard;
mod health;
mod halloween;
mod music;
mod member_cards;
mod premium;
mod tasks;

use anyhow::{Context, Result};
use axum::{extract::State, http::{HeaderMap, StatusCode}, routing::post, Json, Router};
use serenity::{
    cache::Cache,
    http::Http,
    all::{Command, CommandInteraction, ComponentInteraction, CommandOptionType, CreateCommand, CreateCommandOption, CreateAttachment, CreateEmbed, CreateMessage, Guild, GuildId, Interaction, Member, Message, Reaction, Ready, RoleId, User, VoiceServerUpdateEvent, VoiceState},
    async_trait,
    prelude::*,
};
use std::{env, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex as StdMutex, OnceLock}, collections::HashMap, time::Instant};
use tokio::sync::RwLock;

static STATUS_ROTATION_STARTED: AtomicBool = AtomicBool::new(false);
static COUNTDOWN_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_DESCRIPTION: AtomicUsize = AtomicUsize::new(usize::MAX);
static QUEUE_VOTES: OnceLock<StdMutex<HashMap<String, HashMap<u64, bool>>>> = OnceLock::new();
use serenity::client::Context as DiscordContext;
use tracing::{error, info, warn};

struct BotState {
    started: Instant,
    dashboard: dashboard::DashboardClient,
    premium: premium::PremiumClient,
    community: community::CommunityStore,
    music: Arc<music::MusicManager>,
    discord_http: Arc<RwLock<Option<Arc<Http>>>>,
    discord_cache: Arc<RwLock<Option<Arc<Cache>>>>,
    owner_token: String,
    ready: Arc<AtomicBool>,
}

fn contains_discord_invite(content: &str) -> bool {
    content.split_whitespace().any(|raw| {
        let candidate = raw.trim_matches(|ch: char| matches!(ch, '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | ',' | ';'));
        let candidate = candidate.trim_end_matches(['.', '!', '?']);
        if candidate.is_empty() { return false; }
        let normalized = if candidate.starts_with("discord.gg/") || candidate.starts_with("discord.com/") || candidate.starts_with("discordapp.com/") {
            format!("https://{candidate}")
        } else { candidate.to_string() };
        let Ok(url) = url::Url::parse(&normalized) else { return false; };
        let host = url.host_str().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
        let valid_host = host == "discord.gg" || host == "discord.com" || host == "discordapp.com";
        let path = url.path().trim_matches('/');
        let valid_path = if host == "discord.gg" {
            !path.is_empty() && !path.contains('/')
        } else {
            url.path().starts_with("/invite/") && !url.path().trim_start_matches("/invite/").trim_matches('/').is_empty()
        };
        matches!(url.scheme(), "http" | "https") && valid_host && valid_path
    })
}

struct Handler {
    state: Arc<BotState>,
}

pub(crate) fn registered_global_commands() -> Vec<CreateCommand> {
    let mut commands = vec![
        CreateCommand::new("ping").description("Check Mallow Bot's response time"),
        CreateCommand::new("uptime").description("Show how long Mallow Bot has been online"),
        CreateCommand::new("commands").description("Show the main Mallow Bot command groups"),
        CreateCommand::new("sync").description("Refresh Mallow Bot slash command registrations"),
        CreateCommand::new("tos").description("Show Mallow Bot acceptable-use rules"),
        CreateCommand::new("testwelcome").description("Preview the welcome message in this channel"),
        CreateCommand::new("play").description("Play a song or Spotify track in your voice channel").add_option(CreateCommandOption::new(CommandOptionType::String, "song", "Song name or URL").required(true)),
        CreateCommand::new("playlist").description("Add a YouTube playlist to the music queue").add_option(CreateCommandOption::new(CommandOptionType::String, "url", "YouTube playlist URL").required(true)),
        CreateCommand::new("join").description("Join your current voice channel"),
        CreateCommand::new("leave").description("Leave the voice channel and clear the queue"),
        CreateCommand::new("pause").description("Pause the current song"),
        CreateCommand::new("resume").description("Resume the paused song"),
        CreateCommand::new("skip").description("Skip the currently playing song"),
        CreateCommand::new("queue").description("Show the current music queue"),
        CreateCommand::new("music-manage").description("Open the Mallow music control panel"),
        CreateCommand::new("stop").description("Stop music and clear the queue"),
        CreateCommand::new("volume").description("Set the music volume").add_option(CreateCommandOption::new(CommandOptionType::Integer, "level", "Volume from 0 to 100").min_int_value(0).max_int_value(100).required(true)),
        CreateCommand::new("lyrics").description("Toggle synchronized lyrics on or off").add_option(CreateCommandOption::new(CommandOptionType::Boolean, "enabled", "Enable or disable synchronized lyrics").required(false)),
        CreateCommand::new("tts").description("Enable or disable voice-channel text-to-speech (Premium)").add_option(CreateCommandOption::new(CommandOptionType::Boolean, "enabled", "Turn voice text-to-speech on or off").required(true)),
        CreateCommand::new("chill-mode").description("Toggle Chill Mode: loop the chill video and lock other music commands"),
    ];
    commands.extend(community::commands());
    commands
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: DiscordContext, ready: Ready) {
        info!(user = %ready.user.name, "Mallow Bot Rust gateway connected");
        if !STATUS_ROTATION_STARTED.swap(true, Ordering::SeqCst) { tasks::spawn_status_rotation(ctx.shard.clone()); }
        if !COUNTDOWN_STARTED.swap(true, Ordering::SeqCst) { halloween::spawn(ctx.http.clone()); }
        self.state.ready.store(true, Ordering::Relaxed);
        *self.state.discord_http.write().await = Some(ctx.http.clone());
        *self.state.discord_cache.write().await = Some(ctx.cache.clone());
        self.state.music.set_shard(ctx.shard.clone()).await;
        self.state.music.set_bot_user_id(ready.user.id.get()).await;
        self.state.music.set_discord_http(ctx.http.clone()).await;
        if let Err(err) = Command::set_global_commands(&ctx.http, registered_global_commands()).await {
            error!(%err, "Could not register global Mallow Bot commands");
        }
        if let Err(err) = GuildId::new(1544628300217978932).set_commands(&ctx.http, community::bliss_commands()).await {
            error!(%err, "Could not register Bliss SMP guild-only commands");
        }
    }

    async fn guild_create(&self, ctx: DiscordContext, guild: Guild, _is_new: Option<bool>) {
        self.state.music.sync_guild_voice_states(&guild).await;
        let banned: std::collections::HashSet<u64> = env::var("BANNED_GUILD_IDS").unwrap_or_default()
            .split(',').filter_map(|item| item.trim().parse::<u64>().ok()).collect();
        if banned.contains(&guild.id.get()) {
            if let Err(error) = guild.id.leave(&ctx.http).await {
                warn!(%error, guild_id = guild.id.get(), "Could not leave a blocked server");
            } else {
                info!(guild_id = guild.id.get(), "Left a blocked server");
            }
        }
    }

    async fn guild_member_addition(&self, ctx: DiscordContext, member: Member) {
        if self.state.premium.is_banned(member.user.id.get()).await { return; }
        let guild_id = member.guild_id.get();
        if let Some(role_id) = self.state.community.autorole(guild_id) {
            if let Err(error) = member.add_role(&ctx.http, RoleId::new(role_id)).await {
                warn!(%error, guild_id, role_id, "Could not apply configured auto-role");
            }
        }
        let cfg = self.state.dashboard.guild_config(guild_id).await;
        let messages = &cfg.messages_config;
        let is_bliss = guild_id == 1544628300217978932;
        if !is_bliss && messages.get("welcome_enabled").and_then(|v| v.as_bool()) != Some(true) { return; }
        let channel = if is_bliss { Some(1574137525583224963) } else {
            messages.get("welcome_channel_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<u64>().ok())
        };
        let Some(channel) = channel else { return };
        let guild = member.guild_id.to_partial_guild(&ctx.http).await.ok();
        let server = guild.as_ref().map(|g| g.name.as_str()).unwrap_or("the server");
        let count = ctx.cache.guild(member.guild_id).map(|g| g.member_count).unwrap_or(1);
        let user = format!("<@{}>", member.user.id.get());
        let template = messages.get("welcome_message").and_then(|v| v.as_str()).unwrap_or("╭─〔 👤 **Welcomer** 〕─╮\n│ WELCOME {user} TO {server}!!\n│ we now have {member_count} members!!!\n╰──────────────────────────╯");
        let body = template.replace("{user}", &user).replace("{server}", server).replace("{member_count}", &count.to_string());
        let title = messages.get("welcome_title").and_then(|v| v.as_str()).unwrap_or("");
        if is_bliss {
            // Send the Bliss SMP banner as the first embed, with the welcome information directly below it.
            let banner = CreateEmbed::new()
                .image("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/Bliss%20smp%20banner.png");
            let welcome = CreateEmbed::new()
                .title("BLISS SMP • NEW MEMBER")
                .description(format!("Welcome {user}! We're glad you've joined the realm. Get verified and jump into the adventure!"))
                .field("GET SERVER ACCESS", "Head to <#1544630280118206496> and react to the rules message with 💎 to unlock the server.", false)
                .field("FIND THE REALM CODE", "Once you're verified, the realm code is waiting in <#1544638277179678730>.", false)
                .footer(serenity::builder::CreateEmbedFooter::new("BLISS SMP • Your adventure starts here"));
            if let Err(error) = serenity::all::ChannelId::new(channel)
                .send_message(&ctx.http, CreateMessage::new().add_embed(banner).add_embed(welcome))
                .await
            {
                warn!(%error, guild_id, "Could not send Bliss SMP welcome banner");
            }
            return;
        }
        let mut embed = CreateEmbed::new().description(body.chars().take(4000).collect::<String>());
        if !is_bliss && !title.is_empty() { embed = embed.title(title.chars().take(256).collect::<String>()); }
        let display_name = member.nick.as_deref().unwrap_or(&member.user.name);
        let image_url = messages.get("welcome_image_url").and_then(|v| v.as_str()).unwrap_or("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png");
        let avatar_url = member.user.avatar_url().unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());
        let member_number = count.max(1);
        if let Some(card) = member_cards::render(image_url, &avatar_url, display_name, member_number, false).await {
            embed = embed.image("attachment://mallow-member-card.png");
            let builder = CreateMessage::new().embed(embed).add_file(CreateAttachment::bytes(card, "mallow-member-card.png"));
            if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, builder).await {
                warn!(%error, guild_id, "Could not send rendered welcome card");
            }
        } else {
            embed = embed.thumbnail(&avatar_url).image(image_url).footer(serenity::builder::CreateEmbedFooter::new(format!("{} · This user is #{} on the server", display_name, member_number)));
            if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
                warn!(%error, guild_id, "Could not send fallback welcome message");
            }
        }
    }

    async fn guild_member_removal(&self, ctx: DiscordContext, guild_id: GuildId, user: User, member: Option<Member>) {
        let cfg = self.state.dashboard.guild_config(guild_id.get()).await;
        let messages = &cfg.messages_config;
        if messages.get("goodbye_enabled").and_then(|v| v.as_bool()) != Some(true) { return; }
        let Some(channel) = messages.get("goodbye_channel_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<u64>().ok()) else { return };
        let guild = guild_id.to_partial_guild(&ctx.http).await.ok();
        let server = guild.as_ref().map(|g| g.name.as_str()).unwrap_or("the server");
        let display_name = member.as_ref().and_then(|m| m.nick.as_deref()).unwrap_or(&user.name);
        let member_count = ctx.cache.guild(guild_id).map(|g| g.member_count).unwrap_or(1);
        let template = messages.get("goodbye_message").and_then(|v| v.as_str()).unwrap_or("{user} left {server}.");
        let body = template.replace("{user}", display_name).replace("{server}", server).replace("{member_count}", &member_count.to_string());
        let title = messages.get("goodbye_title").and_then(|v| v.as_str()).unwrap_or("");
        let mut embed = CreateEmbed::new().description(body.chars().take(4000).collect::<String>());
        if !title.is_empty() { embed = embed.title(title.chars().take(256).collect::<String>()); }
        let image_url = messages.get("goodbye_image_url").or_else(|| messages.get("welcome_image_url")).and_then(|v| v.as_str()).unwrap_or("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png");
        let avatar_url = user.avatar_url().unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());
        let member_number = member_count.saturating_add(1).max(1);
        if let Some(card) = member_cards::render(image_url, &avatar_url, display_name, member_number, true).await {
            embed = embed.image("attachment://mallow-member-card.png");
            let builder = CreateMessage::new().embed(embed).add_file(CreateAttachment::bytes(card, "mallow-member-card.png"));
            if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, builder).await {
                warn!(%error, guild_id = guild_id.get(), "Could not send rendered goodbye card");
            }
        } else {
            embed = embed.thumbnail(&avatar_url).image(image_url).footer(serenity::builder::CreateEmbedFooter::new(format!("{display_name} · This user was #{member_number} on the server")));
            if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
                warn!(%error, guild_id = guild_id.get(), "Could not send fallback goodbye message");
            }
        }
    }

    async fn voice_state_update(&self, _ctx: DiscordContext, _old: Option<VoiceState>, new: VoiceState) {
        self.state.music.voice_state_update(&new).await;
    }

    async fn voice_server_update(&self, _ctx: DiscordContext, event: VoiceServerUpdateEvent) {
        self.state.music.voice_server_update(&event).await;
    }

    async fn reaction_add(&self, ctx: DiscordContext, reaction: Reaction) {
        apply_reaction_role(&self.state, &ctx, &reaction, true).await;
    }

    async fn reaction_remove(&self, ctx: DiscordContext, reaction: Reaction) {
        apply_reaction_role(&self.state, &ctx, &reaction, false).await;
    }

    async fn message_delete(&self, _ctx: DiscordContext, channel_id: serenity::all::ChannelId, deleted_message_id: serenity::all::MessageId, guild_id: Option<GuildId>) {
        if guild_id.is_none() { return; }
        let key = channel_id.get().to_string();
        if let Some(history) = self.state.community.record("recent_messages", &key).and_then(|v| v.as_array().cloned()) {
            if let Some(deleted) = history.iter().find(|entry| entry.get("id").and_then(serde_json::Value::as_u64) == Some(deleted_message_id.get())) {
                self.state.community.set_record("snipes", &key, Some(deleted.clone()));
            }
        }
    }

    async fn message(&self, ctx: DiscordContext, message: Message) {
        if message.author.bot || self.state.premium.is_banned(message.author.id.get()).await { return; }
        let Some(guild_id) = message.guild_id.map(|id| id.get()) else {
            let route = self.state.community.record("dm_routes", &message.author.id.get().to_string());
            let channel = route.as_ref().and_then(|v| v.get("channel_id")).and_then(serde_json::Value::as_u64)
                .or_else(|| self.state.community.first_log_channel());
            if let Some(channel) = channel {
                let server = route.as_ref().and_then(|v| v.get("guild_id")).and_then(serde_json::Value::as_u64).map(|id| id.to_string()).unwrap_or_else(|| "Unsolicited DM".into());
                let content = if message.content.trim().is_empty() { "[No text content]" } else { &message.content };
                let forwarded = format!("**Mallow Bot DM received**\nFrom: {} (ID {})\nServer: {}\nMessage:\n{}", message.author.name, message.author.id.get(), server, content);
                if serenity::all::ChannelId::new(channel).say(&ctx.http, forwarded.chars().take(1900).collect::<String>()).await.is_ok() {
                    let _ = message.channel_id.say(&ctx.http, "Your message was forwarded to the server's mod channel.").await;
                } else {
                    let _ = message.channel_id.say(&ctx.http, "I couldn't forward your message because the log channel is unavailable.").await;
                }
            } else {
                let _ = message.channel_id.say(&ctx.http, "I couldn't forward your message because no DM log channel is configured.").await;
            }
            return;
        };
        let cfg = self.state.dashboard.guild_config(guild_id).await;
        let history_key = message.channel_id.get().to_string();
        let mut history = self.state.community.record("recent_messages", &history_key).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        history.push(serde_json::json!({"id":message.id.get(),"content":message.content,"author":message.author.name}));
        if history.len() > 20 { history.drain(0..history.len() - 20); }
        self.state.community.set_record("recent_messages", &history_key, Some(serde_json::Value::Array(history)));
        if let Some(sticky) = self.state.community.record("sticky_messages", &message.channel_id.get().to_string()) {
            let text = sticky.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
            let old_id = sticky.get("message_id").and_then(serde_json::Value::as_u64);
            if !text.is_empty() {
                if let Some(old_id) = old_id { let _ = message.channel_id.delete_message(&ctx.http, serenity::all::MessageId::new(old_id)).await; }
                if let Ok(posted) = message.channel_id.say(&ctx.http, text).await {
                    let mut updated = sticky.clone();
                    updated["message_id"] = serde_json::json!(posted.id.get());
                    self.state.community.set_record("sticky_messages", &message.channel_id.get().to_string(), Some(updated));
                }
            }
        }
        let content = message.content.trim();
        let has_invite = contains_discord_invite(content);
        let exempt = message.member.as_ref()
            .and_then(|member| member.permissions)
            .map(|permissions| permissions.manage_messages())
            .unwrap_or(false);
        if cfg.features.get("moderation").and_then(|v| v.as_bool()).unwrap_or(true) && has_invite && !exempt {
            let minutes = self.state.community.timeout_minutes(guild_id);
            if message.delete(&ctx.http).await.is_ok() {
                match serenity::all::GuildId::new(guild_id).member(&ctx.http, message.author.id).await {
                    Ok(mut member) => {
                        let until = serenity::model::Timestamp::from_unix_timestamp(
                            (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() + minutes * 60) as i64
                        );
                        match until {
                            Ok(until) if member.disable_communication_until_datetime(&ctx.http, until).await.is_ok() => {
                                let _ = message.channel_id.say(&ctx.http, format!("<@{}> you cannot post Discord invite links here. You have been timed out for {minutes} minutes.", message.author.id.get())).await;
                            }
                            _ => { let _ = message.channel_id.say(&ctx.http, "I detected a Discord invite, but I need Manage Messages and Moderate Members permissions.").await; }
                        }
                    }
                    Err(_) => { let _ = message.channel_id.say(&ctx.http, "I detected a Discord invite, but I couldn't apply the configured timeout.").await; }
                }
            }
            return;
        }

        // Refresh the complete voice-state snapshot from Serenity's guild cache before
        // checking the author. This avoids relying only on voice-state events received
        // since startup, which could leave other members missing from our local map.
        let cached_guild = ctx.cache.guild(GuildId::new(guild_id)).map(|guild| guild.clone());
        let author_voice_channel = cached_guild.as_ref()
            .and_then(|guild| guild.voice_states.get(&message.author.id))
            .and_then(|voice_state| voice_state.channel_id)
            .map(|channel| channel.get())
            .or_else(|| None);
        if let Some(guild) = cached_guild.as_ref() {
            self.state.music.sync_guild_voice_states(guild).await;
        }

        // Only read messages sent in the text chat attached to the voice channel
        // Mallow is currently connected to. Voice-channel chat messages use the
        // voice channel's own channel ID; ordinary server text channels do not.
        // The server toggle is persisted in community_data.json (missing means disabled).
        let tts_enabled = self.state.community.record("tts_enabled", &guild_id.to_string())
            .and_then(|value| value.as_bool()).unwrap_or(false);
        if tts_enabled && cfg.features.get("music").and_then(|value| value.as_bool()) != Some(false) {
            let bot_channel = self.state.music.bot_voice_channel(guild_id).await;
            if let Some(bot_channel) = bot_channel {
                let is_voice_channel_chat = message.channel_id.get() == bot_channel;
                if is_voice_channel_chat && !content.is_empty() {
                    let user_channel = author_voice_channel.or_else(|| None)
                        .or(self.state.music.voice_channel_for_user(guild_id, message.author.id.get()).await);
                    if user_channel == Some(bot_channel) {
                        let speaker = message.member.as_ref().and_then(|member| member.nick.as_deref())
                            .unwrap_or(&message.author.name);
                        if let Err(error) = self.state.music.speak_tts(guild_id, message.author.id.get(), speaker, content).await {
                            warn!(%error, guild_id, user_id = message.author.id.get(), "Could not enqueue voice-channel TTS message");
                        }
                    }
                }
            }
        }

        let is_prefix = content.to_lowercase().starts_with(&cfg.prefix.to_lowercase());
        if !content.is_empty() && cfg.features.get("xp").and_then(|v| v.as_bool()).unwrap_or(true) {
            let cooldown = cfg.xp_config.get("cooldown_seconds").and_then(|v| v.as_u64()).unwrap_or(60);
            let min_xp = cfg.xp_config.get("min_xp").and_then(serde_json::Value::as_i64).unwrap_or(10).clamp(0, 10_000);
            let max_xp = cfg.xp_config.get("max_xp").and_then(serde_json::Value::as_i64).unwrap_or(20).clamp(min_xp, 10_000);
            let add = if max_xp <= min_xp { min_xp } else { rand::Rng::gen_range(&mut rand::thread_rng(), min_xp..=max_xp) };
            if let Some((old, new)) = self.state.community.add_xp(guild_id, message.author.id.get(), add, cooldown) {
                let old_level = ((old.max(0) as f64 / 100.0).sqrt()) as i64;
                let new_level = ((new.max(0) as f64 / 100.0).sqrt()) as i64;
                if new_level > old_level && cfg.xp_config.get("announce_level_up").and_then(|v| v.as_bool()).unwrap_or(true) {
                    let _ = message.channel_id.say(&ctx.http, format!("<@{}> reached level {}.", message.author.id.get(), new_level)).await;
                }
            }
        }
        if cfg.messages_config.get("auto_reply_enabled").and_then(|v| v.as_bool()) == Some(true) {
            if let Some(items) = cfg.messages_config.get("auto_replies").and_then(|v| v.as_array()) {
                for item in items.iter().take(30) {
                    let trigger = item.get("trigger").and_then(|v| v.as_str()).unwrap_or("").trim();
                    let response = item.get("response").and_then(|v| v.as_str()).unwrap_or("").trim();
                    let contains = item.get("match_type").and_then(|v| v.as_str()) == Some("contains");
                    let matched = if contains { content.to_lowercase().contains(&trigger.to_lowercase()) } else { content.eq_ignore_ascii_case(trigger) };
                    if !trigger.is_empty() && !response.is_empty() && matched {
                        let _ = message.reply(&ctx.http, response.chars().take(1900).collect::<String>()).await;
                        break;
                    }
                }
            }
        }
        if cfg.features.get("social").and_then(|v| v.as_bool()).unwrap_or(true) {
            let video_id = content.split_whitespace().find_map(|part| {
                if let Some(id) = part.split("youtu.be/").nth(1) { Some(id.chars().take(11).collect::<String>()) }
                else if let Some(id) = part.split("v=").nth(1) { Some(id.chars().take(11).collect::<String>()) }
                else if let Some(id) = part.split("/shorts/").nth(1) { Some(id.chars().take(11).collect::<String>()) }
                else { None }
            }).filter(|id| id.len() == 11);
            if let Some(video_id) = video_id {
                let url = format!("https://www.youtube.com/watch?v={video_id}");
                if let Ok(result) = reqwest::Client::new().get("https://www.youtube.com/oembed").query(&[("url", url.as_str()), ("format", "json")]).send().await {
                    if let Ok(info) = result.json::<serde_json::Value>().await {
                        let title = info.get("title").and_then(|v| v.as_str()).unwrap_or("YouTube video");
                        let channel = info.get("author_name").and_then(|v| v.as_str()).unwrap_or("Unknown");
                        let _ = message.reply(&ctx.http, format!("**{title}**\nChannel: {channel}\n{url}")).await;
                    }
                }
            }
        }
        if cfg.features.get("swear_counter").and_then(|v| v.as_bool()).unwrap_or(true) && !cfg.swear_words.is_empty() {
            let hits = community::count_swear_hits(content, &cfg.swear_words);
            if hits > 0 {
                if let Some(total) = self.state.community.add_swears_once(guild_id, message.author.id.get(), message.id.get(), hits) {
                    let adjustment = cfg.swear_count_adjustments.get(message.author.id.get().to_string()).and_then(|v| v.as_i64()).unwrap_or(0);
                    let shown = (total + adjustment).max(0);
                    let _ = message.reply(&ctx.http, format!("You now have {shown} swear(s) in this server.")).await;
                }
            }
        }
        if is_prefix && community::handle_prefix(&ctx, &message, &cfg, &self.state.community, &self.state.music, &self.state.premium, self.state.started).await { return; }
        if message.message_reference.is_none() && message.mentions.iter().any(|user| user.id == ctx.cache.current_user().id) {
            let descriptions: Vec<&str> = include_str!("../../description.txt").split("\n\n")
                .map(str::trim).filter(|value| !value.is_empty()).collect();
            if !descriptions.is_empty() {
                let mut index = rand::Rng::gen_range(&mut rand::thread_rng(), 0..descriptions.len());
                let previous = LAST_DESCRIPTION.swap(index, Ordering::SeqCst);
                if index == previous && descriptions.len() > 1 { index = (index + 1) % descriptions.len(); LAST_DESCRIPTION.store(index, Ordering::SeqCst); }
                let _ = message.reply(&ctx.http, descriptions[index].chars().take(1900).collect::<String>()).await;
            }
        }
    }

    async fn interaction_create(&self, ctx: DiscordContext, interaction: Interaction) {
        match interaction {
            Interaction::Command(command) => {
                // Acknowledge immediately: dashboard and premium lookups plus command
                // registration can exceed Discord's 3-second interaction deadline.
                let music_command = ["play","join","leave","pause","resume","skip","queue","music-manage","stop","volume","lyrics","tts","chill-mode"]
                    .contains(&command.data.name.as_str());
                if let Err(error) = command.create_response(
                    &ctx.http,
                    serenity::builder::CreateInteractionResponse::Defer(
                        serenity::builder::CreateInteractionResponseMessage::new().ephemeral(!music_command),
                    ),
                ).await {
                    warn!(%error, command = %command.data.name, "Could not defer slash command interaction");
                    return;
                }
                if self.state.premium.is_banned(command.user.id.get()).await {
                    let _ = command.edit_response(
                        &ctx.http,
                        serenity::builder::EditInteractionResponse::new()
                            .content("This account cannot use Mallow Bot."),
                    ).await;
                    return;
                }
                let guild_config = if let Some(guild_id) = command.guild_id {
                    self.state.dashboard.guild_config(guild_id.get()).await
                } else {
                    config::GuildConfig::default()
                };
                if !community::handle(ctx.http.as_ref(), &command, &self.state.community, &self.state.premium, &guild_config, true).await {
                    handle_command(ctx.http.as_ref(), &command, &self.state, &guild_config, true).await;
                }
            }
            Interaction::Component(component) => {
                if component.data.custom_id.starts_with("mallow:queue:") {
                    handle_queue_component(&ctx.http, &component, &self.state).await;
                } else {
                    handle_ticket_component(&ctx.http, &component, &self.state, false).await;
                }
            }
            _ => {}
        }
    }
}

async fn handle_queue_component(http: &Http, component: &ComponentInteraction, state: &BotState) {
    use serenity::builder::{CreateActionRow, CreateButton, CreateInteractionResponse, CreateInteractionResponseMessage};
    if component.data.custom_id.starts_with("mallow:manage:") {
        let Some(guild) = component.guild_id.map(|id| id.get()) else { return; };
        let action = component.data.custom_id.trim_start_matches("mallow:manage:");
        let message = match action {
            "pause" => state.music.pause(guild, true).await.unwrap_or_else(|e| e.to_string()),
            "resume" => state.music.pause(guild, false).await.unwrap_or_else(|e| e.to_string()),
            "skip" => state.music.skip(guild).await.unwrap_or_else(|e| e.to_string()),
            "volume-down" => state.music.volume(guild, 50).await.unwrap_or_else(|e| e.to_string()),
            "volume-up" => state.music.volume(guild, 100).await.unwrap_or_else(|e| e.to_string()),
            "lyrics" => state.music.set_lyrics(guild, None).await,
            "chill" => state.music.toggle_chill_mode(guild).await.unwrap_or_else(|e| e.to_string()),
            _ => "Unknown music control.".into(),
        };
        let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(message).ephemeral(true))).await;
        return;
    }
    let parts: Vec<&str> = component.data.custom_id.split(':').collect();
    let Some(guild) = component.guild_id.map(|id| id.get()) else { return; };
    if parts.len() < 5 { return; }
    let index = parts[3].parse::<usize>().unwrap_or(usize::MAX);
    let owner = parts[4].parse::<u64>().unwrap_or(0);
    let answer = parts.get(5).copied();
    if parts[2] == "remove" {
        if component.user.id.get() == owner {
            let result = state.music.remove_queued_track(guild, index).await;
            let message = match result { Ok((title, _)) => format!("Removed **{title}** from the queue."), Err(error) => error.to_string() };
            let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(message).ephemeral(true))).await;
        } else {
            let yes = CreateButton::new(format!("mallow:queue:vote:{index}:{owner}:yes")).label("Yes").style(serenity::all::ButtonStyle::Success);
            let no = CreateButton::new(format!("mallow:queue:vote:{index}:{owner}:no")).label("No").style(serenity::all::ButtonStyle::Danger);
            let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("This song was added by someone else. Should it be removed?").components(vec![CreateActionRow::Buttons(vec![yes, no])]).ephemeral(true))).await;
        }
    } else if parts[2] == "vote" {
        let voice_channel = state.music.bot_voice_channel(guild).await;
        if state.music.voice_channel_for_user(guild, component.user.id.get()).await != voice_channel {
            let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("Only people in the bot's voice channel can vote.").ephemeral(true))).await;
            return;
        }
        let eligible = state.discord_cache.read().await.as_ref().and_then(|cache| cache.guild(GuildId::new(guild)).map(|g| g.voice_states.iter().filter(|(_, voice)| voice.channel_id.map(|id| id.get()) == voice_channel).count())).unwrap_or(1).max(1);
        let key = format!("{guild}:{index}:{owner}");
        let store = QUEUE_VOTES.get_or_init(|| StdMutex::new(HashMap::new()));
        let (yes, no, passed, failed) = {
            let mut votes = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let ballot = votes.entry(key.clone()).or_default();
            ballot.insert(component.user.id.get(), answer == Some("yes"));
            let yes = ballot.values().filter(|vote| **vote).count();
            let no = ballot.len().saturating_sub(yes);
            (yes, no, yes * 2 > eligible, no * 2 >= eligible)
        };
        if passed || failed {
            if let Some(store) = QUEUE_VOTES.get() { store.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&key); }
            let result = if passed { state.music.remove_queued_track(guild, index).await } else { Err(anyhow::anyhow!("Vote failed")) };
            let message = match result { Ok((title, _)) => format!("Vote passed ({yes}/{eligible}). **{title}** was removed."), Err(_) => format!("Vote did not pass ({yes}/{eligible} yes). The song stays in the queue.") };
            let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(message).ephemeral(true))).await;
        } else {
            let _ = component.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(format!("Vote recorded: {yes} yes, {no} no. More than 50% of {eligible} voice-channel members must vote yes.")).ephemeral(true))).await;
        }
    }
}
async fn handle_ticket_component(http: &Http, component: &ComponentInteraction, state: &BotState, deferred: bool) {
    use serenity::builder::{CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse};
    let Some(guild_id) = component.guild_id.map(|id| id.get()) else { return };
    let respond = |content: String| async move {
        if deferred {
            let _ = component.edit_response(http, EditInteractionResponse::new().content(content)).await;
        } else {
            let _ = component.create_response(http, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content(content).ephemeral(true)
            )).await;
        }
    };
    match component.data.custom_id.as_str() {
        "mallow:ticket:open" => {
            let cfg = state.dashboard.guild_config(guild_id).await;
            match community::create_ticket_channel(http, guild_id, component.user.id.get(), &component.user.name, &cfg, &state.community).await {
                Ok(channel) => respond(format!("Your private support ticket is ready: <#{channel}>.")).await,
                Err(error) => respond(error).await,
            }
        }
        "mallow:ticket:close" => {
            let key = component.channel_id.get().to_string();
            let Some(ticket) = state.community.record("tickets", &key) else {
                respond("This channel is not a Mallow Bot ticket.".into()).await;
                return;
            };
            let owner = ticket.get("user_id").and_then(serde_json::Value::as_u64);
            let cfg = state.dashboard.guild_config(guild_id).await;
            let support: Vec<u64> = cfg.tickets_config.get("support_role_ids").and_then(serde_json::Value::as_array)
                .into_iter().flatten().filter_map(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).collect();
            let member = component.member.as_ref();
            let has_manage = member.and_then(|m| m.permissions).map(|p| p.manage_channels()).unwrap_or(false);
            let has_support = member.map(|m| m.roles.iter().any(|role| support.contains(&role.get()))).unwrap_or(false);
            if owner != Some(component.user.id.get()) && !has_manage && !has_support {
                respond("Only the ticket creator or configured support staff can close this ticket.".into()).await;
                return;
            }
            if let Err(error) = community::export_ticket_transcript(http, component.channel_id, &cfg).await {
                warn!(%error, channel_id = component.channel_id.get(), "Could not export ticket transcript");
            }
            respond("Closing this ticket in a moment…".into()).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if component.channel_id.delete(http).await.is_ok() {
                state.community.set_record("tickets", &key, None);
            }
        }
        _ => {}
    }
}

async fn apply_reaction_role(state: &BotState, ctx: &DiscordContext, reaction: &Reaction, added: bool) {
    let (Some(guild_id), Some(user_id)) = (reaction.guild_id, reaction.user_id) else { return };
    let Some(role_id) = state.community.role_for_reaction(guild_id.get(), reaction.message_id.get(), &reaction.emoji.to_string(), added) else { return };
    let Ok(member) = guild_id.member(&ctx.http, user_id).await else { return };
    if member.user.bot { return; }
    let role = RoleId::new(role_id);
    let result = if added { member.add_role(&ctx.http, role).await } else { member.remove_role(&ctx.http, role).await };
    if let Err(error) = result { warn!(%error, guild_id = guild_id.get(), user_id = user_id.get(), role_id, "Could not apply reaction-role change"); }
}

async fn handle_command(http: &Http, command: &CommandInteraction, state: &BotState, cfg: &config::GuildConfig, deferred: bool) {
    use serenity::builder::{CreateInteractionResponse, CreateInteractionResponseMessage, EditInteractionResponse};
    let guild = command.guild_id.map(|id| id.get());
    let music_commands = ["play","join","leave","pause","resume","skip","queue","music-manage","stop","volume","lyrics","tts","chill-mode"];
    let chill_mode = command.data.name == "chill-mode";
    if music_commands.contains(&command.data.name.as_str()) && !chill_mode
        && (cfg.features.get("music").and_then(serde_json::Value::as_bool) == Some(false)
            || cfg.disabled_commands.iter().any(|disabled| disabled == &command.data.name)) {
        let content = "Music commands are disabled in this server.".to_string();
        if deferred { let _ = command.edit_response(http, EditInteractionResponse::new().content(content)).await; }
        else { let _ = command.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(content).ephemeral(true))).await; }
        return;
    }
    if !chill_mode {
        if let Some(guild) = guild {
            if state.music.is_chill_mode(guild).await {
                let message = "Chill Mode is active. Run /chill-mode again to turn it off, or make the bot leave/join a voice channel.";
                if deferred { let _ = command.edit_response(http, EditInteractionResponse::new().content(message)).await; }
                else { let _ = command.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(message).ephemeral(true))).await; }
                return;
            }
        }
    }
    let voice_channel = if let Some(guild) = guild { state.music.voice_channel_for_user(guild, command.user.id.get()).await } else { None };
    let content = match command.data.name.as_str() {
        "chill-mode" => {
            if let Some(guild) = guild {
                match state.music.toggle_chill_mode(guild).await { Ok(text) => text, Err(error) => format!("Chill Mode error: {error}") }
            } else { "Use this command in a server.".into() }
        },
        "play" => match (guild, voice_channel) {
            (Some(guild), Some(channel)) => {
                let song = command.data.options.iter().find_map(|o| if let serenity::all::CommandDataOptionValue::String(value) = &o.value { Some(value.as_str()) } else { None }).unwrap_or("");
                // Do not send another Discord voice-state join request when the bot
                // is already in the user's channel: Lavalink may restart the current track.
                if state.music.bot_voice_channel(guild).await != Some(channel) {
                    state.music.request_voice_channel(guild, Some(channel)).await;
                }
                match state.music.enqueue(guild, song, command.user.id.get()).await { Ok(text) => text, Err(error) => format!("Music error: {error}") }
            }
            _ => "Join a voice channel first, then use /play.".into()
        },
        "playlist" => match (guild, voice_channel) {
            (Some(guild), Some(channel)) => {
                let url = command.data.options.iter().find_map(|o| {
                    if o.name == "url" {
                        if let serenity::all::CommandDataOptionValue::String(value) = &o.value { Some(value.as_str()) } else { None }
                    } else { None }
                }).unwrap_or("");
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    "Please provide a YouTube playlist URL.".into()
                } else {
                    // Keep the existing voice connection when already in the user's channel.
                    if state.music.bot_voice_channel(guild).await != Some(channel) {
                        state.music.request_voice_channel(guild, Some(channel)).await;
                    }
                    match state.music.enqueue(guild, url, command.user.id.get()).await {
                        Ok(text) => text,
                        Err(error) => format!("Playlist error: {error}")
                    }
                }
            }
            _ => "Join a voice channel first, then use /playlist.".into()
        },
        "join" => match (guild, voice_channel) {
            (Some(guild), Some(channel)) => { state.music.request_voice_channel(guild, Some(channel)).await; "Joining your voice channel.".into() }
            _ => "Join a voice channel first.".into()
        },
        "leave" => {
            if let Some(guild) = guild { state.music.request_voice_channel(guild, None).await; let _ = state.music.stop(guild).await; "Leaving the voice channel and clearing the queue.".into() }
            else { "Use this command in a server.".into() }
        }
        "pause" | "resume" => {
            if let Some(guild) = guild { match state.music.pause(guild, command.data.name == "pause").await { Ok(text) => text, Err(error) => format!("Music error: {error}") } }
            else { "Use this command in a server.".into() }
        }
        "skip" => {
            if let Some(guild) = guild { match state.music.skip(guild).await { Ok(text) => text, Err(error) => format!("Music error: {error}") } }
            else { "Use this command in a server.".into() }
        }
        "stop" => {
            if let Some(guild) = guild { match state.music.stop(guild).await { Ok(text) => text, Err(error) => format!("Music error: {error}") } }
            else { "Use this command in a server.".into() }
        }
        "queue" => {
            if let Some(guild) = guild { state.music.queue_text(guild).await }
            else { "Use this command in a server.".into() }
        }
        "music-manage" => {
            if let Some(guild) = guild { state.music.queue_text(guild).await }
            else { "Use this command in a server.".into() }
        }
        "volume" => {
            let level = command.data.options.iter().find_map(|o| if let serenity::all::CommandDataOptionValue::Integer(value) = &o.value { Some(*value) } else { None }).unwrap_or(100).clamp(0,100) as u16;
            if let Some(guild) = guild { match state.music.volume(guild, level).await { Ok(text) => text, Err(error) => format!("Music error: {error}") } }
            else { "Use this command in a server.".into() }
        }
        "lyrics" => {
            let enabled = command.data.options.iter().find_map(|o| if let serenity::all::CommandDataOptionValue::Boolean(value) = &o.value { Some(*value) } else { None });
            if let Some(guild) = guild { state.music.set_lyrics(guild, enabled).await }
            else { "Use this command in a server.".into() }
        }
        "tts" => {
            if let Some(guild) = guild {
                let enabled = command.data.options.iter().find_map(|o| {
                    if let serenity::all::CommandDataOptionValue::Boolean(value) = &o.value { Some(*value) } else { None }
                }).unwrap_or(false);
                if command.user.id.get() != 1021068313071210498 {
                    let (banned, active, _) = state.premium.status(command.user.id.get()).await;
                    if banned { "This account cannot use Mallow Bot.".into() }
                    else if !active { "Mallow Premium is required to change the TTS setting.".into() }
                    else {
                        state.community.set_record("tts_enabled", &guild.to_string(), Some(serde_json::json!(enabled)));
                        if enabled { "Voice-channel TTS is now enabled. Anyone in your voice channel can type and Mallow will read it aloud.".into() }
                        else { "Voice-channel TTS is now disabled.".into() }
                    }
                } else {
                    state.community.set_record("tts_enabled", &guild.to_string(), Some(serde_json::json!(enabled)));
                    if enabled { "Voice-channel TTS is now enabled. Anyone in your voice channel can type and Mallow will read it aloud.".into() }
                    else { "Voice-channel TTS is now disabled.".into() }
                }
            } else { "Use this command in a server.".into() }
        }

        "testwelcome" => {
            if guild.is_none() { "Use this command in a server.".to_string() }
            else {
                let guild_id = guild.unwrap();
                let messages = &cfg.messages_config;
                // Bliss SMP has its own branded welcome. Keep the test command in sync
                // with the real member-join welcome instead of using the generic card.
                if guild_id == 1544628300217978932 {
                    let banner = CreateEmbed::new()
                        .image("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/Bliss%20smp%20banner.png");
                    let welcome = CreateEmbed::new()
                        .title("BLISS SMP • NEW MEMBER")
                        .description(format!("Welcome <@{}>! We're glad you've joined the realm. Get verified and jump into the adventure!", command.user.id.get()))
                        .field("GET SERVER ACCESS", "Head to <#1544630280118206496> and react to the rules message with 💎 to unlock the server.", false)
                        .field("FIND THE REALM CODE", "Once you're verified, the realm code is waiting in <#1544638277179678730>.", false)
                        .footer(serenity::builder::CreateEmbedFooter::new("BLISS SMP • Your adventure starts here"));
                    if command.channel_id.send_message(http, CreateMessage::new().add_embed(banner).add_embed(welcome)).await.is_ok() {
                        "Bliss SMP welcome preview sent in this channel.".to_string()
                    } else {
                        "I couldn't send the Bliss SMP welcome preview. Check my channel permissions.".to_string()
                    }
                } else {
                let cached_guild = state.discord_cache.read().await.clone()
                    .and_then(|cache| cache.guild(GuildId::new(guild_id))
                        .map(|cached| (cached.member_count, cached.name.clone())));
                let member_count = cached_guild.as_ref().map(|cached| cached.0).unwrap_or(1);
                let guild_name = cached_guild.as_ref().map(|cached| cached.1.clone()).unwrap_or_else(|| "the server".into());
                let display_name = command.member.as_ref().and_then(|member| member.nick.as_deref())
                    .unwrap_or(&command.user.name);
                let user_mention = format!("<@{}>", command.user.id.get());
                let template = messages.get("welcome_message").and_then(serde_json::Value::as_str)
                    .unwrap_or("╭─〔 👤 **Welcomer** 〕─╮\n│ WELCOME {user} TO {server}!!\n│ we now have {member_count} members!!!\n╰──────────────────────────╯");
                let body = template.replace("{user}", &user_mention).replace("{server}", &guild_name)
                    .replace("{member_count}", &member_count.to_string());
                let title = messages.get("welcome_title").and_then(serde_json::Value::as_str).unwrap_or("");
                let image_url = messages.get("welcome_image_url").and_then(serde_json::Value::as_str)
                    .unwrap_or("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png");
                let avatar_url = command.user.avatar_url().unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());
                if let Some(card) = member_cards::render(image_url, &avatar_url, display_name, member_count.max(1) as u64, false).await {
                    let mut embed = CreateEmbed::new().description(body.chars().take(4000).collect::<String>())
                        .image("attachment://mallow-member-card.png");
                    if !title.is_empty() { embed = embed.title(title.chars().take(256).collect::<String>()); }
                    if command.channel_id.send_message(http, CreateMessage::new().embed(embed)
                        .add_file(CreateAttachment::bytes(card, "mallow-member-card.png"))).await.is_ok() {
                        "Welcome preview sent in this channel.".to_string()
                    } else { "I couldn't send the welcome preview. Check my channel permissions.".to_string() }
                } else {
                    let mut embed = CreateEmbed::new().description(body.chars().take(4000).collect::<String>())
                        .thumbnail(&avatar_url).image(image_url)
                        .footer(serenity::builder::CreateEmbedFooter::new(format!("{display_name} · This user is #{} on the server", member_count.max(1))));
                    if !title.is_empty() { embed = embed.title(title.chars().take(256).collect::<String>()); }
                    if command.channel_id.send_message(http, CreateMessage::new().embed(embed)).await.is_ok() {
                        "Welcome preview sent in this channel (fallback image mode).".to_string()
                    } else { "I couldn't send the welcome preview. Check my channel permissions.".to_string() }
                }
                }
            }
        },
        "ping" => "Pong! The Rust process is responsive.".to_string(),
        "uptime" => {
            let elapsed = state.started.elapsed();
            format!("Mallow Bot has been online for {}d {:02}h {:02}m {:02}s.",
                elapsed.as_secs() / 86_400,
                (elapsed.as_secs() / 3_600) % 24,
                (elapsed.as_secs() / 60) % 60,
                elapsed.as_secs() % 60)
        }
        "sync" => {
            let permitted = command.member.as_ref().and_then(|member| member.permissions).map(|permissions| permissions.administrator()).unwrap_or(false);
            if !permitted { "You need Administrator permission to refresh slash commands.".into() }
            else if let Some(guild) = command.guild_id {
                if guild.get() != 1544628300217978932 {
                    if let Err(error) = guild.set_commands(http, Vec::new()).await {
                        warn!(%error, "Could not clear stale guild command registrations");
                    }
                }
                match Command::set_global_commands(http, registered_global_commands()).await {
                    Ok(_) => {
                        // Also publish the standard commands as guild commands here.
                        // Discord exposes guild commands immediately, avoiding the
                        // global-command propagation delay that hid /tts after /sync.
                        // Keep standard commands global only. Registering the same
                        // names as both global and guild commands makes Discord show
                        // duplicate entries. Only publish Bliss-specific commands here.
                        if guild.get() == 1544628300217978932 {
                            match guild.set_commands(http, community::bliss_commands()).await {
                                Ok(_) => "Global commands refreshed; Bliss server commands updated.".into(),
                                Err(error) => format!("Global commands were refreshed, but Bliss server commands could not be updated: {error}"),
                            }
                        } else {
                            "Global commands refreshed for this server.".into()
                        }
                    }
                    Err(error) => format!("Could not refresh slash commands: {error}"),
                }
            } else { "Use this command in a server.".into() }
        },
        "commands" => "**MUSIC**\n/play, /join, /leave, /pause, /resume, /skip, /queue, /music-manage, /volume, /lyrics, /stop, /tts, /chill-mode.\n\n**COMMUNITY**\nXP, leaderboards, Mallow Coins, daily rewards, work, pay, rob, and games.\n\n**FUN**\nVibecheck, ship, fortune, and Would You Rather.\n\n**SERVER & MODERATION**\nRules, welcome cards, autorole, reaction roles, announcements, events, tickets, purge, and utilities.\n\n**PREMIUM**\nCustom embeds, snipe, sticky messages, backups, custom commands, vanity, and Premium status.".to_string(),
        "tos" => include_str!("../../TERMS_OF_SERVICE.md").chars().take(1900).collect::<String>(),
        _ => format!("The /{} command is not available.", command.data.name),
    };
    let is_music = music_commands.contains(&command.data.name.as_str());
    if is_music {
        let title = match command.data.name.as_str() {
            "play" => "NOW PLAYING / QUEUED",
            "join" => "VOICE CHANNEL",
            "leave" => "MUSIC DISCONNECTED",
            "pause" => "PLAYBACK PAUSED",
            "resume" => "PLAYBACK RESUMED",
            "skip" => "TRACK SKIPPED",
            "queue" => "UP NEXT",
            "music-manage" => "MUSIC CONTROL PANEL",
            "stop" => "PLAYBACK STOPPED",
            "volume" => "VOLUME UPDATED",
            "lyrics" => "LYRICS SETTINGS",
            "tts" => "VOICE TTS",
            "chill-mode" => "CHILL MODE",
            _ => "MALLOW MUSIC",
        };
        let mut components = Vec::new();
        if command.data.name == "queue" {
            if let Some(guild_id) = guild {
                let tracks = state.music.queued_tracks(guild_id).await;
                for (page, chunk) in tracks.iter().take(25).collect::<Vec<_>>().chunks(5).enumerate() {
                    let buttons = chunk.iter().enumerate().map(|(offset, (_, owner))| {
                        let index = page * 5 + offset;
                        serenity::builder::CreateButton::new(format!("mallow:queue:remove:{index}:{owner}")).label(format!("❌ {}", index + 1)).style(serenity::all::ButtonStyle::Danger)
                    }).collect();
                    components.push(serenity::builder::CreateActionRow::Buttons(buttons));
                }
            }
        }
        else if command.data.name == "music-manage" {
            use serenity::builder::CreateButton;
            components.push(serenity::builder::CreateActionRow::Buttons(vec![
                CreateButton::new("mallow:manage:pause").label("Pause").style(serenity::all::ButtonStyle::Secondary),
                CreateButton::new("mallow:manage:resume").label("Resume").style(serenity::all::ButtonStyle::Success),
                CreateButton::new("mallow:manage:skip").label("Skip").style(serenity::all::ButtonStyle::Primary),
                CreateButton::new("mallow:manage:lyrics").label("Lyrics").style(serenity::all::ButtonStyle::Secondary),
                CreateButton::new("mallow:manage:chill").label("Chill Mode").style(serenity::all::ButtonStyle::Secondary),
            ]));
            components.push(serenity::builder::CreateActionRow::Buttons(vec![
                CreateButton::new("mallow:manage:volume-down").label("Volume 50%").style(serenity::all::ButtonStyle::Secondary),
                CreateButton::new("mallow:manage:volume-up").label("Volume 100%").style(serenity::all::ButtonStyle::Secondary),
            ]));
        }
        let embed = CreateEmbed::new()
            .title(format!("MALLOW MUSIC  //  {title}"))
            .description(content.chars().take(4000).collect::<String>())
            .colour(serenity::all::Colour::new(0x000000))
            .image("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/embed-bottom.png")
            .footer(serenity::builder::CreateEmbedFooter::new(format!("Requested by {}", command.user.name)));
        if deferred {
            if let Err(err) = command.edit_response(http, EditInteractionResponse::new().embeds(community::branded_embeds(embed)).components(components.clone())).await {
                warn!(%err, command = %command.data.name, "Could not edit deferred music response");
            }
        } else if let Err(err) = command.create_response(http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().embeds(community::branded_embeds(embed)).components(components)
        )).await {
            warn!(%err, command = %command.data.name, "Could not respond to music interaction");
        }
    } else if ["commands", "help", "tos", "update"].contains(&command.data.name.as_str()) {
        let title = match command.data.name.as_str() { "tos" => "TERMS OF SERVICE", "update" => "LATEST UPDATES", _ => "MALLOW BOT HELP" };
        let embed = CreateEmbed::new()
            .title(format!("MALLOW BOT  //  {title}"))
            .description(content.chars().take(4000).collect::<String>())
            .colour(serenity::all::Colour::new(0x000000))
            .image("https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/embed-bottom.png")
            .footer(serenity::builder::CreateEmbedFooter::new(format!("Requested by {}", command.user.name)));
        if deferred {
            if let Err(err) = command.edit_response(http, EditInteractionResponse::new().embeds(community::branded_embeds(embed))).await {
                warn!(%err, command = %command.data.name, "Could not edit information embed");
            }
        } else if let Err(err) = command.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().embeds(community::branded_embeds(embed)).ephemeral(true))).await {
            warn!(%err, command = %command.data.name, "Could not send information embed");
        }
    } else if deferred {
        if let Err(err) = command.edit_response(http, EditInteractionResponse::new().content(content)).await {
            warn!(%err, command = %command.data.name, "Could not edit deferred interaction response");
        }
    } else {
        let response = CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(content).ephemeral(true));
        if let Err(err) = command.create_response(http, response).await {
            warn!(%err, command = %command.data.name, "Could not respond to interaction");
        }
    }
}

async fn owner_authorized(state: &BotState, headers: &HeaderMap) -> bool {
    let supplied = headers.get("Authorization").and_then(|value| value.to_str().ok()).unwrap_or("");
    !state.owner_token.is_empty() && supplied.strip_prefix("Bot ").map(|token| token.trim() == state.owner_token).unwrap_or(false)
}

async fn owner_guilds(
    State(state): State<Arc<BotState>>,
    headers: HeaderMap,
) -> (StatusCode, Json<serde_json::Value>) {
    if !owner_authorized(&state, &headers).await {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"})));
    }
    let Some(cache) = state.discord_cache.read().await.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"Mallow Bot is still starting."})));
    };
    let mut guilds = Vec::new();
    for guild_id in cache.guilds() {
        if let Some(guild) = cache.guild(guild_id) {
            let icon_url = guild.icon_url();
            guilds.push(serde_json::json!({
                "id": guild_id.get().to_string(),
                "name": guild.name,
                "icon_url": icon_url,
                "member_count": guild.member_count
            }));
        }
    }
    guilds.sort_by(|a,b| a.get("name").and_then(serde_json::Value::as_str).unwrap_or("").to_lowercase()
        .cmp(&b.get("name").and_then(serde_json::Value::as_str).unwrap_or("").to_lowercase()));
    (StatusCode::OK, Json(serde_json::json!({"guilds":guilds})))
}

async fn owner_leave(
    State(state): State<Arc<BotState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !owner_authorized(&state, &headers).await {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"})));
    }
    let id = payload.get("guild_id").and_then(|v| v.as_str()).unwrap_or("");
    if !id.chars().all(|ch| ch.is_ascii_digit()) || !(17..=20).contains(&id.len()) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"Invalid Discord server ID."})));
    }
    let Ok(guild_id) = id.parse::<u64>() else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"Invalid Discord server ID."})));
    };
    let Some(cache) = state.discord_cache.read().await.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"Mallow Bot is still starting."})));
    };
    if cache.guild(GuildId::new(guild_id)).is_none() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"Mallow is not in that server anymore."})));
    }
    let Some(http) = state.discord_http.read().await.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"Mallow Bot is still starting."})));
    };
    match http.leave_guild(GuildId::new(guild_id)).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok":true}))),
        Err(_) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error":"Discord refused the leave request."})))
    }
}

async fn http_interaction(
    State(state): State<Arc<BotState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let expected = env::var("WAKE_SECRET").unwrap_or_default();
    let supplied = headers.get("X-Mallow-Bot-Secret").or_else(|| headers.get("X-Mochi-Bot-Secret"))
        .and_then(|value| value.to_str().ok()).unwrap_or("");
    if expected.is_empty() || supplied != expected {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"})));
    }
    let Some(http) = state.discord_http.read().await.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"Discord client is still starting"})));
    };
    if payload.get("type").and_then(serde_json::Value::as_u64) == Some(3) {
        let component: ComponentInteraction = match serde_json::from_value(payload) {
            Ok(component) => component,
            Err(error) => {
                warn!(%error, "Could not decode forwarded Discord component interaction");
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid component interaction"})));
            }
        };
        handle_ticket_component(http.as_ref(), &component, &state, true).await;
        return (StatusCode::OK, Json(serde_json::json!({"ok":true})));
    }
    let interaction_type = payload.get("type").and_then(serde_json::Value::as_u64).unwrap_or(0);
    if interaction_type != 2 && interaction_type != 3 {
        return (StatusCode::OK, Json(serde_json::json!({"ok":true})));
    }
    let command: CommandInteraction = match serde_json::from_value(payload) {
        Ok(command) => command,
        Err(error) => {
            warn!(%error, "Could not decode forwarded Discord command interaction");
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid interaction"})));
        }
    };
    if state.premium.is_banned(command.user.id.get()).await {
        let _ = command.edit_response(http.as_ref(), serenity::builder::EditInteractionResponse::new().content("This account cannot use Mallow Bot.")).await;
        return (StatusCode::OK, Json(serde_json::json!({"ok":true})));
    }
    let cfg = if let Some(guild) = command.guild_id { state.dashboard.guild_config(guild.get()).await } else { config::GuildConfig::default() };
    if !community::handle(http.as_ref(), &command, &state.community, &state.premium, &cfg, true).await {
        handle_command(http.as_ref(), &command, &state, &cfg, true).await;
    }
    (StatusCode::OK, Json(serde_json::json!({"ok":true})))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| "mallow_bot=info,warn".into()))
        .init();

    let token = env::var("DISCORD_TOKEN").context("DISCORD_TOKEN is required")?;
    let state = Arc::new(BotState {
        started: Instant::now(),
        dashboard: dashboard::DashboardClient::from_env()?,
        premium: premium::PremiumClient::from_env()?,
        community: community::CommunityStore::open(),
        music: Arc::new(music::MusicManager::from_env()?),
        discord_http: Arc::new(RwLock::new(None)),
        discord_cache: Arc::new(RwLock::new(None)),
        owner_token: token.clone(),
        ready: Arc::new(AtomicBool::new(false)),
    });

    let port = env::var("PORT").or_else(|_| env::var("SERVER_PORT")).unwrap_or_else(|_| "25572".into());
    let health_state = health::HealthState { started: state.started, ready: state.ready.clone() };
    let bridge_app = Router::new()
        .route("/discord/interactions", post(http_interaction))
        .route("/owner/guilds", axum::routing::get(owner_guilds))
        .route("/owner/leave", post(owner_leave))
        .with_state(state.clone());
    let health_app = health::router(health_state).merge(bridge_app);
    let health_listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tokio::spawn(async move {
        if let Err(err) = axum::serve(health_listener, health_app).await {
            error!(%err, "Health/API server stopped");
        }
    });

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::DIRECT_MESSAGES;

    let handler = Handler { state: state.clone() };
    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .context("Could not create Discord client")?;

    tasks::spawn_keepalives();
    tokio::spawn(state.music.clone().run_node());
    info!("Starting Mallow Bot Rust migration runtime");
    client.start().await.context("Discord gateway stopped")?;
    Ok(())
}
