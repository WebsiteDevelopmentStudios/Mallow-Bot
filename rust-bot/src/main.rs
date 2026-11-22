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
use std::{env, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::Instant};
use tokio::sync::RwLock;

static STATUS_ROTATION_STARTED: AtomicBool = AtomicBool::new(false);
static COUNTDOWN_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_DESCRIPTION: AtomicUsize = AtomicUsize::new(usize::MAX);
use serenity::client::Context as DiscordContext;
use tracing::{error, info, warn};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

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

struct Handler {
    state: Arc<BotState>,
}

fn registered_global_commands() -> Vec<CreateCommand> {
    let mut commands = vec![
        CreateCommand::new("ping").description("Check Mallow Bot's response time"),
        CreateCommand::new("uptime").description("Show how long Mallow Bot has been online"),
        CreateCommand::new("commands").description("Show the main Mallow Bot command groups"),
        CreateCommand::new("sync").description("Refresh Mallow Bot slash command registrations"),
        CreateCommand::new("tos").description("Show Mallow Bot acceptable-use rules"),
        CreateCommand::new("play").description("Play a song or Spotify track in your voice channel").add_option(CreateCommandOption::new(CommandOptionType::String, "song", "Song name or URL").required(true)),
        CreateCommand::new("join").description("Join your current voice channel"),
        CreateCommand::new("leave").description("Leave the voice channel and clear the queue"),
        CreateCommand::new("pause").description("Pause the current song"),
        CreateCommand::new("resume").description("Resume the paused song"),
        CreateCommand::new("skip").description("Skip the currently playing song"),
        CreateCommand::new("queue").description("Show the current music queue"),
        CreateCommand::new("stop").description("Stop music and clear the queue"),
        CreateCommand::new("volume").description("Set the music volume").add_option(CreateCommandOption::new(CommandOptionType::Integer, "level", "Volume from 0 to 100").min_int_value(0).max_int_value(100).required(true)),
        CreateCommand::new("lyrics").description("Toggle synchronized lyrics on or off").add_option(CreateCommandOption::new(CommandOptionType::Boolean, "enabled", "Enable or disable synchronized lyrics").required(false)),
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
        let mut embed = if is_bliss {
            CreateEmbed::new()
                .title("BLISS SMP • NEW MEMBER")
                .description(format!("Welcome {user}! We're glad you've joined the realm. Get verified and jump into the adventure!"))
                .field("GET SERVER ACCESS", "Head to <#1544630280118206496> and react to the rules message with 💎 to unlock the server.", false)
                .field("FIND THE REALM CODE", "Once you're verified, the realm code is waiting in <#1544638277179678730>.", false)
                .footer(serenity::builder::CreateEmbedFooter::new("BLISS SMP • Your adventure starts here"))
        } else {
            CreateEmbed::new().description(body.chars().take(4000).collect::<String>())
        };
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
        let has_invite = content.split_whitespace().any(|part| {
            let folded = part.to_lowercase();
            folded.contains("discord.gg/") || folded.contains("discord.com/invite/") || folded.contains("discordapp.com/invite/")
        });
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
        let is_prefix = content.to_lowercase().starts_with(&cfg.prefix.to_lowercase());
        if is_prefix && community::handle_prefix(&ctx, &message, &cfg, &self.state.community, &self.state.music, &self.state.premium).await { return; }
        if !is_prefix && !content.is_empty() && cfg.features.get("xp").and_then(|v| v.as_bool()).unwrap_or(true) {
            let cooldown = cfg.xp_config.get("cooldown_seconds").and_then(|v| v.as_u64()).unwrap_or(60);
            let min_xp = cfg.xp_config.get("min_xp").and_then(serde_json::Value::as_i64).unwrap_or(10).max(0);
            let max_xp = cfg.xp_config.get("max_xp").and_then(serde_json::Value::as_i64).unwrap_or(20).max(min_xp);
            let add = min_xp + (message.id.get() % (max_xp - min_xp + 1).max(1) as u64) as i64;
            if let Some((old, new)) = self.state.community.add_xp(guild_id, message.author.id.get(), add, cooldown) {
                let old_level = ((old.max(0) as f64 / 100.0).sqrt()) as i64;
                let new_level = ((new.max(0) as f64 / 100.0).sqrt()) as i64;
                if new_level > old_level && cfg.xp_config.get("announce_level_up").and_then(|v| v.as_bool()).unwrap_or(true) {
                    let _ = message.channel_id.say(&ctx.http, format!("<@{}> reached level {}.", message.author.id.get(), new_level)).await;
                }
            }
        }
        if !is_prefix && cfg.messages_config.get("auto_reply_enabled").and_then(|v| v.as_bool()) == Some(true) {
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
        if !is_prefix && cfg.features.get("social").and_then(|v| v.as_bool()).unwrap_or(true) {
            let video_id = content.split_whitespace().find_map(|part| {
                if let Some(id) = part.split("youtu.be/").nth(1) { Some(id.chars().take(11).collect::<String>()) }
                else if let Some(id) = part.split("v=").nth(1) { Some(id.chars().take(11).collect::<String>()) }
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
        if !is_prefix && cfg.features.get("swear_counter").and_then(|v| v.as_bool()).unwrap_or(true) && !cfg.swear_words.is_empty() {
            let normalize = |value: &str| -> Vec<String> {
                let mut mapped = String::new();
                for raw in value.nfkd().flat_map(char::to_lowercase).filter(|ch| !is_combining_mark(*ch)) {
                    let ch = match raw {
                        '0' => 'o',
                        '1' | '!' | '|' => 'i',
                        '3' => 'e',
                        '4' | '@' => 'a',
                        '5' | '$' => 's',
                        '7' | '+' => 't',
                        '8' => 'b',
                        '9' => 'g',
                        '€' => 'e',
                        other => other,
                    };
                    if ch.is_alphanumeric() { mapped.push(ch); }
                    else if ch.is_whitespace() { mapped.push(' '); }
                }
                mapped.split_whitespace().map(str::to_owned).collect()
            };
            let words = normalize(content);
            let mut phrases: Vec<Vec<String>> = cfg.swear_words.iter()
                .map(|term| normalize(term))
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
                } else {
                    cursor += 1;
                }
            }
            if hits > 0 {
                let total = self.state.community.add_swears(guild_id, message.author.id.get(), hits);
                let adjustment = cfg.swear_count_adjustments.get(message.author.id.get().to_string()).and_then(|v| v.as_i64()).unwrap_or(0);
                let shown = (total + adjustment).max(0);
                let _ = message.reply(&ctx.http, format!("You now have {shown} swear(s) in this server.")).await;
            }
        }
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
                if self.state.premium.is_banned(command.user.id.get()).await {
                    let builder = serenity::builder::CreateInteractionResponse::Message(
                        serenity::builder::CreateInteractionResponseMessage::new()
                            .content("This account cannot use Mallow Bot.")
                            .ephemeral(true),
                    );
                    let _ = command.create_response(&ctx.http, builder).await;
                    return;
                }
                let guild_config = if let Some(guild_id) = command.guild_id {
                    self.state.dashboard.guild_config(guild_id.get()).await
                } else {
                    config::GuildConfig::default()
                };
                if !community::handle(ctx.http.as_ref(), &command, &self.state.community, &self.state.premium, &guild_config, false).await {
                    handle_command(ctx.http.as_ref(), &command, &self.state, &guild_config, false).await;
                }
            }
            Interaction::Component(component) => {
                handle_ticket_component(&ctx.http, &component, &self.state, false).await;
            }
            _ => {}
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
    let music_commands = ["play","join","leave","pause","resume","skip","queue","stop","volume","lyrics"];
    if music_commands.contains(&command.data.name.as_str())
        && (cfg.features.get("music").and_then(serde_json::Value::as_bool) == Some(false)
            || cfg.disabled_commands.iter().any(|disabled| disabled == &command.data.name)) {
        let content = "Music commands are disabled in this server.".to_string();
        if deferred { let _ = command.edit_response(http, EditInteractionResponse::new().content(content)).await; }
        else { let _ = command.create_response(http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(content).ephemeral(true))).await; }
        return;
    }
    let voice_channel = if let Some(guild) = guild { state.music.voice_channel_for_user(guild, command.user.id.get()).await } else { None };
    let content = match command.data.name.as_str() {
        "play" => match (guild, voice_channel) {
            (Some(guild), Some(channel)) => {
                let song = command.data.options.iter().find_map(|o| if let serenity::all::CommandDataOptionValue::String(value) = &o.value { Some(value.as_str()) } else { None }).unwrap_or("");
                state.music.request_voice_channel(guild, Some(channel)).await;
                match state.music.enqueue(guild, song).await { Ok(text) => text, Err(error) => format!("Music error: {error}") }
            }
            _ => "Join a voice channel first, then use /play.".into()
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
                        let _ = GuildId::new(1544628300217978932).set_commands(http, community::bliss_commands()).await;
                        "Global commands refreshed. Bliss SMP commands remain registered only in Bliss SMP.".into()
                    }
                    Err(error) => format!("Could not refresh slash commands: {error}"),
                }
            } else { "Use this command in a server.".into() }
        },
        "commands" => "Music: /play, /join, /leave, /pause, /resume, /skip, /queue, /volume, /lyrics, /stop.\nCommunity: /xp, /leaderboard, /balance, /daily, /work, /pay, /rob, /gamble, /coinflip, /slots, /roulette, /dice, /blackjack, /vibecheck, /ship, /fortune, /wyr.\nUtility: /ping, /uptime, /commands, /sync, /tos, /say, /update, /youtube, /purge.\nServer: /rules, /autorole, /reaction-role, /announcement, /event, /ticket, /ticket-panel, /ticket-close.\nPremium: /embed, /snipe, /sticky, /backup, /custom-command, /vanity, /premium-status.".to_string(),
        "tos" => include_str!("../../TERMS_OF_SERVICE.md").chars().take(1900).collect::<String>(),
        _ => return,
    };
    if deferred {
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
