"""Two-way MallowCord <-> Discord bridge for the official Mallow Swamp server.

The bridge uses the existing Mallow-Bot service key to authenticate with the
MallowCord Worker. Discord messages are mirrored as read-only shadow profiles;
MallowCord messages are delivered through a channel webhook so the sender's
display name and avatar are preserved.
"""
from __future__ import annotations

import asyncio
import logging
import os
import re
from urllib.parse import urlsplit
from typing import Any

import aiohttp
import discord
from dotenv import load_dotenv

load_dotenv()

log = logging.getLogger("mallowcord-bridge")
API_BASE = os.getenv("MALLOWCORD_API_URL", "https://mallowcord-api.wowgrayhaha.workers.dev/api").rstrip("/")
SERVICE_KEY = os.getenv("MALLOWCORD_SERVICE_KEY", "").strip()
GUILD_ID = int((os.getenv("MALLOWCORD_DISCORD_GUILD_ID", "1560367587542433812").strip() or "1560367587542433812"))
HISTORY_LIMIT = max(-1, min(1000000, int(os.getenv("MALLOWCORD_HISTORY_LIMIT", "0"))))
POLL_SECONDS = max(2, int(os.getenv("MALLOWCORD_OUTBOX_POLL_SECONDS", "3")))


class MallowCordBridge:
    def __init__(self) -> None:
        # Expose the configured guild ID on the bridge instance because main.py
        # uses mallowcord_bridge.GUILD_ID in Discord event handlers.
        self.GUILD_ID = GUILD_ID
        self.session: aiohttp.ClientSession | None = None
        self.bot: discord.Client | None = None
        self.poll_task: asyncio.Task | None = None
        self.webhooks: dict[int, discord.Webhook] = {}
        self._sync_lock = asyncio.Lock()
        self._synced = False

    @property
    def enabled(self) -> bool:
        return bool(API_BASE and SERVICE_KEY and GUILD_ID)

    async def _http(self) -> aiohttp.ClientSession:
        if self.session is None or self.session.closed:
            timeout = aiohttp.ClientTimeout(total=210, connect=20, sock_read=180)
            self.session = aiohttp.ClientSession(
                timeout=timeout,
                headers={"X-MallowBot-Key": SERVICE_KEY, "User-Agent": "Mallow-Bot-MallowCord-Bridge/1.0"},
            )
        return self.session

    async def _request(self, method: str, path: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
        session = await self._http()
        for attempt in range(3):
            try:
                async with session.request(method, API_BASE + path, json=payload) as response:
                    data = await response.json(content_type=None)
                    if not isinstance(data, dict):
                        data = {"error": str(data)[:500]}
                    if response.status >= 500 and attempt < 2:
                        log.warning(
                            "MallowCord API returned HTTP %s for %s; retrying (%s/3).",
                            response.status, path, attempt + 2,
                        )
                        await asyncio.sleep(1.5 * (attempt + 1))
                        continue
                    if response.status >= 400:
                        detail = data.get("detail")
                        message = str(data.get("error") or "request failed")
                        if detail:
                            message += f" — {detail}"
                        raise RuntimeError(f"MallowCord API {response.status} on {path}: {message}")
                    return data
            except (aiohttp.ClientError, asyncio.TimeoutError) as exc:
                if attempt >= 2:
                    raise
                log.warning(
                    "MallowCord request %s %s failed (%s/3): %s; retrying.",
                    method, path, attempt + 1, exc,
                )
                await asyncio.sleep(1.5 * (attempt + 1))
        raise RuntimeError(f"MallowCord request failed after retries: {path}")

    async def start(self, bot: discord.Client) -> None:
        self.bot = bot
        if not self.enabled:
            log.info("MallowCord bridge disabled; set MALLOWCORD_SERVICE_KEY on the bot host.")
            return
        if self.poll_task is None or self.poll_task.done():
            self.poll_task = asyncio.create_task(self._poll_loop(), name="mallowcord-discord-outbox")
        if not self._synced:
            try:
                await self.sync_guild()
                self._synced = True
            except Exception:
                log.exception("Initial MallowCord synchronization failed; it will retry after reconnect.")

    async def sync_guild(self) -> None:
        if not self.enabled or self.bot is None:
            return
        async with self._sync_lock:
            guild = self.bot.get_guild(GUILD_ID)
            if guild is None:
                try:
                    guild = await self.bot.fetch_guild(GUILD_ID)
                except (discord.HTTPException, discord.Forbidden) as exc:
                    log.error("Cannot access configured MallowCord Discord guild %s: %s", GUILD_ID, exc)
                    return

            # Text and announcement channels are mirrored; voice channels remain Discord-only.
            channels = [
                {
                    "id": str(channel.id),
                    "name": channel.name,
                    "position": channel.position,
                    "category": channel.category.name if channel.category else "",
                    "locked": channel.name.casefold() in {"rules", "server-rules"},
                    "overwrites": [
                        {
                            "id": str(target.id),
                            "target_type": "role" if isinstance(target, discord.Role) else "user",
                            "allow": [name for name, value in overwrite if value is True],
                            "deny": [name for name, value in overwrite if value is False],
                        }
                        for target, overwrite in channel.overwrites.items()
                    ],
                }
                for channel in guild.text_channels
            ]
            roles = [
                {"id": str(role.id), "name": role.name, "position": role.position, "color": f"#{role.color.value:06x}"}
                for role in guild.roles if not role.is_default() and not role.managed
            ]
            result = await self._request("POST", "/integrations/mallowbot/discord/sync", {
                "guild_id": str(guild.id),
                "guild_name": guild.name,
                "icon_url": str(guild.icon.url) if guild.icon else None,
                "channels": channels,
                "roles": roles,
                "members": [],
                "everyone_role_id": str(guild.default_role.id),
                "emojis": [
                    {"id": str(emoji.id), "name": emoji.name, "image_url": str(emoji.url)}
                    for emoji in guild.emojis
                ],
            })
            log.info(
                "MallowCord Discord structure sync completed: %s channels, %s roles.",
                result.get("channels", len(channels)), result.get("roles", len(roles)),
            )

            # Fetch the complete member list when the privileged Members intent is enabled.
            members: list[dict[str, Any]] = []
            try:
                async for member in guild.fetch_members(limit=None):
                    if member.bot:
                        continue
                    members.append({
                        "id": str(member.id),
                        "username": member.name,
                        "display_name": member.display_name,
                        "avatar_url": str(member.display_avatar.url),
                        "roles": [str(role.id) for role in member.roles if not role.is_default()],
                        "status": str(member.status),
                        "bot": False,
                    })
                    if len(members) >= 10:
                        batch = members
                        members = []
                        await self._request("POST", "/integrations/mallowbot/discord/sync", {
                            "guild_id": str(guild.id), "channels": [], "roles": [], "members": batch,
                        })
                        log.debug("Synchronized a MallowCord Discord member batch (%s members).", len(batch))
                if members:
                    await self._request("POST", "/integrations/mallowbot/discord/sync", {
                        "guild_id": str(guild.id), "channels": [], "roles": [], "members": members,
                    })
                log.info("MallowCord member directory synchronized successfully.")
            except Exception as exc:
                log.warning(
                    "Could not import the full Discord member list; the member sync batch failed: %s", exc,
                )

            # Re-send channel overwrites after member batches so user-specific Discord
            # overwrites can resolve to their imported MallowCord shadow accounts.
            try:
                await self._request("POST", "/integrations/mallowbot/discord/sync", {
                    "guild_id": str(guild.id),
                    "channels": channels,
                    "roles": roles,
                    "members": [],
                    "everyone_role_id": str(guild.default_role.id),
                    "emojis": [],
                })
            except Exception as exc:
                log.warning("Could not apply Discord channel permission overwrites: %s", exc)

            # Import the most recent history once per channel. Message IDs are deduplicated by the Worker.
            if HISTORY_LIMIT >= 0:
                for channel in guild.text_channels:
                    try:
                        # A value of 0 means import the full accessible channel history.
                        history = [message async for message in channel.history(limit=HISTORY_LIMIT or None)]
                        for message in reversed(history):
                            await self.handle_discord_message(message)
                    except (discord.Forbidden, discord.HTTPException) as exc:
                        log.warning("Could not import history for #%s: %s", channel.name, exc)
            log.info("MallowCord Discord bridge initialized for guild %s (%s).", guild.name, guild.id)

    async def handle_discord_message(self, message: discord.Message) -> None:
        if not self.enabled or message.guild is None or message.guild.id != GUILD_ID:
            return
        if message.author.bot or message.webhook_id is not None:
            return
        content = message.content or ""
        if message.attachments:
            links: list[str] = []
            for item in message.attachments:
                image_type = str(item.content_type or "").lower().startswith("image/")
                image_name = item.filename.lower().endswith((".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif"))
                links.append(f"![{item.filename}]({item.url})" if image_type or image_name else item.url)
            content = (content + "\n" + "\n".join(links)).strip()
        # Some Discord image posts arrive as embeds rather than file attachments.
        for embed in message.embeds:
            image_url = getattr(getattr(embed, "image", None), "url", None) or getattr(getattr(embed, "thumbnail", None), "url", None)
            if image_url and str(image_url) not in content:
                content = (content + "\n" + f"![Discord image]({image_url})").strip()
        if not content:
            # Do not create blank rows for embed-only system messages.
            return
        try:
            await self._request("POST", "/integrations/mallowbot/discord/messages", {
                "guild_id": str(message.guild.id),
                "channel_id": str(message.channel.id),
                "message_id": str(message.id),
                "author_id": str(message.author.id),
                "username": message.author.name,
                "display_name": getattr(message.author, "display_name", message.author.name),
                "avatar_url": str(message.author.display_avatar.url),
                "content": content[:4000],
                "timestamp": int(message.created_at.timestamp()),
                "mentioned_user_ids": [str(member.id) for member in message.mentions],
            })
        except Exception as exc:
            log.warning("Could not mirror Discord message %s: %s", message.id, exc)

    async def handle_presence_update(self, member: discord.Member) -> None:
        if not self.enabled or member.guild.id != GUILD_ID or member.bot:
            return
        try:
            await self._request("POST", "/integrations/mallowbot/discord/presence", {
                "guild_id": str(member.guild.id),
                "user_id": str(member.id),
                "status": str(member.status),
            })
        except Exception as exc:
            log.warning("Could not synchronize Discord presence for %s: %s", member.id, exc)

    async def _get_webhook(self, channel: discord.TextChannel) -> discord.Webhook | None:
        cached = self.webhooks.get(channel.id)
        if cached:
            return cached
        try:
            hooks = await channel.webhooks()
            hook = next((item for item in hooks if item.name == "MallowCord Bridge"), None)
            if hook is None:
                hook = await channel.create_webhook(name="MallowCord Bridge", reason="Mirror MallowCord chat messages")
            self.webhooks[channel.id] = hook
            return hook
        except (discord.Forbidden, discord.HTTPException) as exc:
            log.warning("Webhook unavailable in #%s; will use bot-message fallback: %s", channel.name, exc)
            return None

    async def _deliver(self, event: dict[str, Any]) -> None:
        if self.bot is None:
            return
        try:
            channel_id = int(event["discord_channel_id"])
        except (KeyError, TypeError, ValueError):
            return
        channel = self.bot.get_channel(channel_id)
        if channel is None:
            try:
                channel = await self.bot.fetch_channel(channel_id)
            except (discord.NotFound, discord.Forbidden, discord.HTTPException):
                log.warning("Mapped Discord channel %s is unavailable.", channel_id)
                return
        if not isinstance(channel, discord.TextChannel):
            return
        content = str(event.get("content") or "")
        username = str(event.get("username") or "MallowCord user")[:80]
        avatar = event.get("avatar_url")
        if not isinstance(avatar, str) or not avatar.startswith("https://"):
            avatar = None
        # Explicit embeds make image URLs render consistently in Discord webhooks.
        image_urls: list[str] = []
        for match in re.findall(r'https://[^\s<>"\']+', content):
            image_url = match.rstrip(").,!?]")
            if urlsplit(image_url).path.lower().endswith((".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif")) and image_url not in image_urls:
                image_urls.append(image_url)
        embeds = [discord.Embed().set_image(url=url) for url in image_urls[:10]]
        for image_url in image_urls:
            content = content.replace(image_url, "").strip()
        content = re.sub(r'!\[[^\]]*\]\(\s*\)', "", content).strip()
        hook = await self._get_webhook(channel)
        sent: discord.Message | None = None
        chunks = [content[i:i + 1900] for i in range(0, len(content), 1900)] or [" "]
        try:
            for index, chunk in enumerate(chunks):
                current_embeds = embeds if index == 0 else []
                if hook:
                    sent = await hook.send(
                        chunk, username=username, avatar_url=avatar,
                        embeds=current_embeds,
                        allowed_mentions=discord.AllowedMentions.none(), wait=True,
                    )
                else:
                    sent = await channel.send(
                        f"**{discord.utils.escape_markdown(username)}**: {chunk}",
                        embeds=current_embeds,
                        allowed_mentions=discord.AllowedMentions.none(),
                    )
            await self._request(
                "POST", f"/integrations/mallowbot/discord/outbox/{event['id']}/ack",
                {"discord_message_id": str(sent.id) if sent else ""},
            )
        except (discord.Forbidden, discord.HTTPException, RuntimeError) as exc:
            log.warning("Could not deliver MallowCord message to #%s: %s", channel.name, exc)


    async def handle_link_command(self, message: discord.Message) -> bool:
        if message.guild is None or message.guild.id != GUILD_ID:
            return False
        match = re.fullmatch(r"-mb\s+link\s+([A-Za-z0-9-]{8,16})", message.content.strip(), re.I)
        if not match:
            return False
        try:
            result = await self._request("POST", "/integrations/mallowbot/discord/link/claim", {
                "code": match.group(1), "discord_user_id": str(message.author.id),
            })
            await message.reply("Your Discord account is now linked to MallowCord. You can run Mallow-Bot commands from MallowCord.", mention_author=False)
            return bool(result.get("linked"))
        except Exception as exc:
            await message.reply(f"Could not link your account: {exc}", mention_author=False)
            return True

    @staticmethod
    def _render_command_output(content: Any = None, embed: Any = None, embeds: Any = None) -> str:
        parts = [str(content)] if content else []
        items = list(embeds or [])
        if embed is not None:
            items.append(embed)
        for item in items:
            try:
                data = item.to_dict()
                if data.get("title"): parts.append("**" + str(data["title"]) + "**")
                if data.get("description"): parts.append(str(data["description"]))
                for field in data.get("fields", []):
                    parts.append("**" + str(field.get("name", "")) + "**\n" + str(field.get("value", "")))
                if data.get("footer", {}).get("text"): parts.append("_" + str(data["footer"]["text"]) + "_")
            except Exception:
                parts.append(str(item))
        return "\n\n".join(part for part in parts if part).strip()[:3900]

    async def _run_command_as_member(self, guild: discord.Guild, channel: discord.TextChannel,
                                    member: discord.Member, content: str) -> str:
        bridge = self
        output: list[str] = []
        class ChannelProxy:
            def __init__(self, wrapped): self._wrapped = wrapped
            def __getattr__(self, name): return getattr(self._wrapped, name)
            async def send(self, content=None, **kwargs):
                rendered = bridge._render_command_output(content, kwargs.get("embed"), kwargs.get("embeds"))
                if rendered: output.append(rendered)
                return None
        class MessageProxy:
            _mallowcord_proxy = True
            def __init__(self):
                self.content = content if content.startswith("-mb ") else "-mb " + content.lstrip("/")
                self.author, self.guild = member, guild
                self.channel = ChannelProxy(channel)
                self.mentions, self.role_mentions, self.channel_mentions = [], [], []
                self.attachments, self.embeds = [], []
                self.webhook_id, self.reference, self.id = None, None, 0
                self.created_at = discord.utils.utcnow()
            def __getattr__(self, name): return getattr(channel, name)
            async def reply(self, content=None, **kwargs):
                rendered = bridge._render_command_output(content, kwargs.get("embed"), kwargs.get("embeds"))
                if rendered: output.append(rendered)
                return None
        await self.bot.on_message(MessageProxy())
        return "\n\n".join(output)[-3900:] if output else "Command completed."

    async def _run_queued_command(self, job: dict[str, Any]) -> None:
        if self.bot is None: return
        result = ""
        try:
            guild = self.bot.get_guild(GUILD_ID) or await self.bot.fetch_guild(GUILD_ID)
            member = guild.get_member(int(job["discord_user_id"]))
            if member is None: member = await guild.fetch_member(int(job["discord_user_id"]))
            channel = self.bot.get_channel(int(job["discord_channel_id"]))
            if channel is None: channel = await self.bot.fetch_channel(int(job["discord_channel_id"]))
            if not isinstance(channel, discord.TextChannel): raise RuntimeError("The linked Discord channel is not a text channel.")
            permissions = channel.permissions_for(member)
            if not permissions.view_channel or not permissions.send_messages:
                raise RuntimeError("Your linked Discord account cannot view and send messages in this channel.")
            result = await self._run_command_as_member(guild, channel, member, str(job["content"]))
        except Exception as exc:
            log.warning("MallowCord command %s failed: %s", job.get("id"), exc)
            result = "Command failed: " + str(exc)
        try:
            await self._request("POST", "/integrations/mallowbot/discord/commands/result", {"id": str(job["id"]), "content": result})
        except Exception:
            log.exception("Could not post MallowCord command result for job %s.", job.get("id"))

    async def dispatch_slash(self, interaction: discord.Interaction, command: str) -> None:
        if interaction.guild_id != GUILD_ID or not isinstance(interaction.user, discord.Member) or not isinstance(interaction.channel, discord.TextChannel):
            await interaction.response.send_message("This command is only available in the official Mallow Swamp server's text channels.", ephemeral=True)
            return
        if not interaction.response.is_done(): await interaction.response.defer()
        try:
            permissions = interaction.channel.permissions_for(interaction.user)
            if not permissions.view_channel or not permissions.send_messages:
                raise RuntimeError("You cannot view and send messages in this channel.")
            result = await self._run_command_as_member(interaction.guild, interaction.channel, interaction.user, command)
            await interaction.followup.send(result[:1900], ephemeral=True)
        except Exception as exc:
            await interaction.followup.send("Command failed: " + str(exc), ephemeral=True)

    async def _poll_loop(self) -> None:
        while True:
            try:
                data = await self._request("GET", "/integrations/mallowbot/discord/outbox")
                for event in data.get("events", []):
                    await self._deliver(event)
                commands = await self._request("GET", "/integrations/mallowbot/discord/commands/next")
                for job in commands.get("jobs", []):
                    await self._run_queued_command(job)
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                log.warning("MallowCord outbox poll failed: %s", exc)
            await asyncio.sleep(POLL_SECONDS)

    async def close(self) -> None:
        if self.poll_task:
            self.poll_task.cancel()
            try:
                await self.poll_task
            except asyncio.CancelledError:
                pass
            self.poll_task = None
        if self.session and not self.session.closed:
            await self.session.close()
        self.session = None


mallowcord_bridge = MallowCordBridge()
