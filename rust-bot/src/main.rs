mod config;
mod community;
mod dashboard;
mod health;
mod premium;
mod tasks;

use anyhow::{Context, Result};
use serenity::{
    all::{Command, CommandInteraction, CreateCommand, CreateEmbed, CreateMessage, GuildId, Interaction, Member, Message, Ready, RoleId, User},
    async_trait,
    prelude::*,
};
use std::{env, sync::Arc, time::Instant};
use serenity::client::Context as DiscordContext;
use tracing::{error, info, warn};

struct BotState {
    started: Instant,
    dashboard: dashboard::DashboardClient,
    premium: premium::PremiumClient,
    community: community::CommunityStore,
}

struct Handler {
    state: Arc<BotState>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: DiscordContext, ready: Ready) {
        info!(user = %ready.user.name, "Mallow Bot Rust gateway connected");
        let mut commands = vec![
            CreateCommand::new("ping").description("Check Mallow Bot's response time"),
            CreateCommand::new("uptime").description("Show how long Mallow Bot has been online"),
            CreateCommand::new("commands").description("Show the main Mallow Bot command groups"),
            CreateCommand::new("tos").description("Show Mallow Bot acceptable-use rules"),
        ];
        commands.extend(community::commands());
        if let Err(err) = Command::set_global_commands(&ctx.http, commands).await {
            error!(%err, "Could not register initial Rust migration commands");
        }
    }

    async fn guild_member_addition(&self, ctx: DiscordContext, member: Member) {
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
        let count = guild.as_ref().and_then(|g| g.approximate_member_count).unwrap_or(0);
        let user = format!("<@{}>", member.user.id.get());
        let template = messages.get("welcome_message").and_then(|v| v.as_str()).unwrap_or("Welcome {user} to {server}!");
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
        let builder = CreateMessage::new().embed(embed);
        if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, builder).await {
            warn!(%error, guild_id, "Could not send configured welcome message");
        }
    }

    async fn guild_member_removal(&self, ctx: DiscordContext, guild_id: GuildId, user: User, _member: Option<Member>) {
        let cfg = self.state.dashboard.guild_config(guild_id.get()).await;
        let messages = &cfg.messages_config;
        if messages.get("goodbye_enabled").and_then(|v| v.as_bool()) != Some(true) { return; }
        let Some(channel) = messages.get("goodbye_channel_id").and_then(|v| v.as_str()).and_then(|v| v.parse::<u64>().ok()) else { return };
        let guild = guild_id.to_partial_guild(&ctx.http).await.ok();
        let server = guild.as_ref().map(|g| g.name.as_str()).unwrap_or("the server");
        let template = messages.get("goodbye_message").and_then(|v| v.as_str()).unwrap_or("{user} left {server}.");
        let body = template.replace("{user}", &format!("<@{}>", user.id.get())).replace("{server}", server);
        let title = messages.get("goodbye_title").and_then(|v| v.as_str()).unwrap_or("");
        let mut embed = CreateEmbed::new().description(body.chars().take(4000).collect::<String>());
        if !title.is_empty() { embed = embed.title(title.chars().take(256).collect::<String>()); }
        if let Err(error) = serenity::all::ChannelId::new(channel).send_message(&ctx.http, CreateMessage::new().embed(embed)).await {
            warn!(%error, guild_id = guild_id.get(), "Could not send configured goodbye message");
        }
    }

    async fn message(&self, ctx: DiscordContext, message: Message) {
        if message.author.bot { return; }
        let Some(guild_id) = message.guild_id.map(|id| id.get()) else { return };
        let cfg = self.state.dashboard.guild_config(guild_id).await;
        let content = message.content.trim();
        let is_prefix = content.to_lowercase().starts_with(&cfg.prefix.to_lowercase());
        if is_prefix && community::handle_prefix(&ctx, &message, &cfg, &self.state.community).await { return; }
        if !is_prefix && !content.is_empty() && cfg.features.get("xp").and_then(|v| v.as_bool()).unwrap_or(true) {
            let cooldown = cfg.xp_config.get("cooldown_seconds").and_then(|v| v.as_u64()).unwrap_or(60);
            let add = 10 + (message.id.get() % 11) as i64;
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
                else if let Some(id) = part.split("v=").nth(1) { Some(id.chars().take_11().collect::<String>()) }
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
                for ch in value.to_lowercase().chars() {
                    let ch = match ch { '0' => 'o', '1' | '!' | '|' => 'i', '3' => 'e', '4' | '@' => 'a', '5' | '$' => 's', '7' | '+' => 't', '8' => 'b', '9' => 'g', other => other };
                    if ch.is_alphanumeric() { mapped.push(ch); } else { mapped.push(' '); }
                }
                mapped.split_whitespace().map(str::to_owned).collect()
            };
            let words = normalize(content);
            let mut hits = 0_i64;
            for term in &cfg.swear_words {
                let phrase = normalize(term);
                if phrase.is_empty() || phrase.len() > words.len() { continue; }
                hits += words.windows(phrase.len()).filter(|window| *window == phrase.as_slice()).count() as i64;
            }
            if hits > 0 {
                let total = self.state.community.add_swears(guild_id, message.author.id.get(), hits);
                let adjustment = cfg.swear_count_adjustments.get(message.author.id.get().to_string()).and_then(|v| v.as_i64()).unwrap_or(0);
                let shown = (total + adjustment).max(0);
                let _ = message.reply(&ctx.http, format!("You now have {shown} swear(s) in this server.")).await;
            }
        }
    }

    async fn interaction_create(&self, ctx: DiscordContext, interaction: Interaction) {
        let Interaction::Command(command) = interaction else { return };
        if self.state.premium.is_banned(command.user.id.get()).await {
            let builder = serenity::builder::CreateInteractionResponse::Message(
                serenity::builder::CreateInteractionResponseMessage::new()
                    .content("This account cannot use Mallow Bot.")
                    .ephemeral(true),
            );
            let _ = command.create_response(&ctx.http, builder).await;
            return;
        }
        if !community::handle(&ctx, &command, &self.state.community).await {
            handle_command(&ctx, &command, &self.state).await;
        }
    }
}

async fn handle_command(ctx: &DiscordContext, command: &CommandInteraction, state: &BotState) {
    use serenity::builder::{CreateInteractionResponse, CreateInteractionResponseMessage};
    let content = match command.data.name.as_str() {
        "ping" => format!("Pong! Gateway latency is managed by Discord; process is responsive."),
        "uptime" => {
            let elapsed = state.started.elapsed();
            format!("Mallow Bot has been online for {}d {:02}h {:02}m {:02}s.",
                elapsed.as_secs() / 86_400,
                (elapsed.as_secs() / 3_600) % 24,
                (elapsed.as_secs() / 60) % 60,
                elapsed.as_secs() % 60)
        }
        "commands" => "Mallow Bot commands are being migrated in parity groups. The Python production bot remains active until every command and event is ported.".to_string(),
        "tos" => "Mallow Bot's acceptable-use rules are published in the repository's TERMS_OF_SERVICE.md.".to_string(),
        _ => return,
    };
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(content).ephemeral(true),
    );
    if let Err(err) = command.create_response(&ctx.http, response).await {
        warn!(%err, command = %command.data.name, "Could not respond to interaction");
    }
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
    });

    let port = env::var("PORT").or_else(|_| env::var("SERVER_PORT")).unwrap_or_else(|_| "25572".into());
    let health_state = health::HealthState { started: state.started };
    let health_app = health::router(health_state);
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
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::DIRECT_MESSAGES;

    let handler = Handler { state: state.clone() };
    let mut client = Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .context("Could not create Discord client")?;

    tasks::spawn_keepalives();
    info!("Starting Mallow Bot Rust migration runtime");
    client.start().await.context("Discord gateway stopped")?;
    Ok(())
}
