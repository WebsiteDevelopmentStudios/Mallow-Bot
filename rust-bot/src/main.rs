mod config;
mod community;
mod dashboard;
mod health;
mod premium;

use anyhow::{Context, Result};
use serenity::{
    all::{Command, CommandInteraction, CreateCommand, Interaction, Message, Ready},
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

    async fn message(&self, _ctx: DiscordContext, message: Message) {
        if message.author.bot { return; }
        let Some(guild_id) = message.guild_id.map(|id| id.get()) else { return };
        let cfg = self.state.dashboard.guild_config(guild_id).await;
        if cfg.features.get("xp").and_then(|v| v.as_bool()).unwrap_or(true) && !message.content.trim().is_empty() {
            let old = self.state.community.xp(guild_id, message.author.id.get());
            let add = 10 + (message.id.get() % 11) as i64;
            let new = self.state.community.add_xp(guild_id, message.author.id.get(), add);
            let old_level = ((old.max(0) as f64 / 100.0).sqrt()) as i64;
            let new_level = ((new.max(0) as f64 / 100.0).sqrt()) as i64;
            if new_level > old_level && cfg.xp_config.get("announce_level_up").and_then(|v| v.as_bool()).unwrap_or(true) {
                let _ = message.channel_id.say(&_ctx.http, format!("<@{}> reached level {}.", message.author.id.get(), new_level)).await;
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

    info!("Starting Mallow Bot Rust migration runtime");
    client.start().await.context("Discord gateway stopped")?;
    Ok(())
}
