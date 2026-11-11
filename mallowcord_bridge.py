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
import shlex
import time
from types import SimpleNamespace
from urllib.parse import urlsplit
from typing import Any

import aiohttp
import discord
from discord import app_commands
from dotenv import load_dotenv

load_dotenv()

log = logging.getLogger("mallowcord-bridge")
API_BASE = os.getenv("MALLOWCORD_API_URL", "https://mallowcord-api.wowgrayhaha.workers.dev/api").rstrip("/")
SERVICE_KEY = os.getenv("MALLOWCORD_SERVICE_KEY", "").strip()
GUILD_ID = int((os.getenv("MALLOWCORD_DISCORD_GUILD_ID", "1560367587542433812").strip() or "1560367587542433812"))
HISTORY_LIMIT = max(0, min(1000000, int(os.getenv("MALLOWCORD_HISTORY_LIMIT", "0"))))  # 0 imports the full available channel history
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
            timeout = aiohttp.ClientTimeout(total=90, connect=15, sock_read=75)
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

            # Mirror every supported guild channel and its Discord metadata.
            categories = [
                {"id": str(category.id), "name": category.name, "position": category.position}
                for category in guild.categories
            ]
            channels: list[dict[str, Any]] = []
            for channel in guild.channels:
                if isinstance(channel, discord.CategoryChannel):
                    continue
                kind = "voice" if channel.type in {discord.ChannelType.voice, discord.ChannelType.stage_voice} else "text"
                overwrites: list[dict[str, Any]] = []
                for target, overwrite in channel.overwrites.items():
                    allow: list[str] = []
                    deny: list[str] = []
                    for permission, value in overwrite:
                        if value is True:
                            allow.append(permission)
                        elif value is False:
                            deny.append(permission)
                    overwrites.append({
                        "target_type": "role" if isinstance(target, discord.Role) else "user",
                        "target_discord_id": str(target.id),
                        "target_name": "@everyone" if isinstance(target, discord.Role) and target.is_default() else getattr(target, "name", getattr(target, "display_name", str(target))),
                        "allow": allow,
                        "deny": deny,
                    })
                channels.append({
                    "id": str(channel.id),
                    "name": channel.name,
                    "position": getattr(channel, "position", 0),
                    "kind": kind,
                    "discord_type": int(channel.type.value),
                    "category_id": str(channel.category_id) if getattr(channel, "category_id", None) else None,
                    "category_name": channel.category.name if getattr(channel, "category", None) else "",
                    "topic": getattr(channel, "topic", None) or "",
                    "nsfw": bool(getattr(channel, "nsfw", False)),
                    "slowmode": int(getattr(channel, "slowmode_delay", 0) or 0),
                    "extra_settings": {
                        "bitrate": getattr(channel, "bitrate", None),
                        "user_limit": getattr(channel, "user_limit", None),
                        "rtc_region": str(getattr(channel, "rtc_region", None)) if getattr(channel, "rtc_region", None) else None,
                        "video_quality_mode": int(channel.video_quality_mode.value) if getattr(channel, "video_quality_mode", None) else None,
                        "default_auto_archive_duration": getattr(channel, "default_auto_archive_duration", None),
                        "default_thread_slowmode_delay": getattr(channel, "default_thread_slowmode_delay", None),
                        "default_sort_order": str(getattr(channel, "default_sort_order", None)) if getattr(channel, "default_sort_order", None) else None,
                        "default_forum_layout": str(getattr(channel, "default_forum_layout", None)) if getattr(channel, "default_forum_layout", None) else None,
                    },
                    "permission_overwrites": overwrites,
                })
            roles = [
                {
                    "id": str(role.id),
                    "name": role.name,
                    "position": role.position,
                    "color": f"#{role.color.value:06x}" if role.color.value else "",
                    "permissions": str(role.permissions.value),
                    "hoist": role.hoist,
                    "mentionable": role.mentionable,
                    "managed": role.managed,
                    "icon_url": str(role.icon.url) if role.icon else None,
                    "unicode_emoji": role.unicode_emoji,
                }
                for role in guild.roles if not role.is_default()
            ]
            emojis = [
                {
                    "id": str(emoji.id),
                    "name": emoji.name,
                    "image_url": str(emoji.url),
                    "animated": emoji.animated,
                    "available": emoji.available,
                    "roles": [str(role.id) for role in emoji.roles],
                }
                for emoji in guild.emojis
            ]
            result = await self._request("POST", "/integrations/mallowbot/discord/sync", {
                "guild_id": str(guild.id),
                "guild_name": guild.name,
                "icon_url": str(guild.icon.url) if guild.icon else None,
                "categories": categories,
                "channels": channels,
                "roles": roles,
                "emojis": emojis,
                "members": [],
            })
            log.info(
                "MallowCord Discord sync completed: %s channels, %s roles; response=%s",
                len(channels), len(roles), result,
            )
            log.info(
                "MallowCord server sync ready: %s channels, %s roles.",
                result.get("channels", 0), result.get("roles", 0),
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
                    if len(members) >= 200:
                        await self._request("POST", "/integrations/mallowbot/discord/sync", {
                            "guild_id": str(guild.id), "channels": [], "roles": [], "members": members,
                        })
                        members = []
                if members:
                    await self._request("POST", "/integrations/mallowbot/discord/sync", {
                        "guild_id": str(guild.id), "channels": [], "roles": [], "members": members,
                    })
                log.info("MallowCord member directory synchronized.")
            except Exception as exc:
                log.warning(
                    "Could not import the full Discord member list. Enable the Server Members Intent "
                    "for the bot application and check its guild permissions: %s", exc,
                )

            # Import as much available text-channel history as configured (0 = unlimited).
            for channel in guild.text_channels:
                try:
                    batch: list[discord.Message] = []
                    async for message in channel.history(limit=HISTORY_LIMIT or None, oldest_first=True):
                        if message.author.bot or message.webhook_id is not None:
                            continue
                        batch.append(message)
                        if len(batch) >= 20:
                            await asyncio.gather(*(self.handle_discord_message(item) for item in batch))
                            batch.clear()
                    if batch:
                        await asyncio.gather(*(self.handle_discord_message(item) for item in batch))
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

    async def complete_account_link(self, discord_user_id: int, code: str, guild_id: int) -> bool:
        if not self.enabled or int(guild_id) != GUILD_ID:
            return False
        result = await self._request("POST", "/integrations/mallowbot/discord/link/complete", {
            "guild_id": str(guild_id),
            "discord_user_id": str(discord_user_id),
            "code": str(code).strip().replace("-", "").upper(),
        })
        return bool(result.get("linked"))

    @staticmethod
    def _format_embed(embed: discord.Embed) -> str:
        parts = []
        if embed.title:
            parts.append(str(embed.title))
        if embed.description:
            parts.append(str(embed.description))
        for field in embed.fields:
            parts.append(f"{field.name}: {field.value}")
        return "\n".join(parts)[:3500]

    async def _execute_command(self, event: dict[str, Any]) -> None:
        if self.bot is None:
            return
        result_text = ""
        try:
            guild = self.bot.get_guild(GUILD_ID)
            if guild is None:
                raise RuntimeError("The official Mallow Swamp Discord server is unavailable.")
            member_id = int(event["discord_user_id"])
            member = guild.get_member(member_id)
            if member is None:
                member = await guild.fetch_member(member_id)
            channel_id = int(event["discord_channel_id"])
            channel = self.bot.get_channel(channel_id)
            if channel is None:
                channel = await self.bot.fetch_channel(channel_id)
            if not isinstance(channel, discord.TextChannel):
                raise RuntimeError("The linked Discord channel is not a text channel.")

            outputs: list[str] = []
            sent_messages: list[discord.Message] = []
            private_output = False
            started = discord.utils.utcnow()

            async def send_output(content: Any = None, *, embed: discord.Embed | None = None,
                                  embeds: list[discord.Embed] | None = None, **kwargs: Any) -> discord.Message | None:
                nonlocal private_output
                if kwargs.get("ephemeral") or private_output:
                    private_output = True
                if content:
                    outputs.append(str(content))
                if embed:
                    outputs.append(self._format_embed(embed))
                for item in embeds or []:
                    outputs.append(self._format_embed(item))
                if private_output:
                    return None
                send_kwargs: dict[str, Any] = {"allowed_mentions": discord.AllowedMentions.none()}
                if embed is not None:
                    send_kwargs["embed"] = embed
                if embeds is not None:
                    send_kwargs["embeds"] = embeds
                for key in ("view", "file", "files", "delete_after"):
                    if kwargs.get(key) is not None:
                        send_kwargs[key] = kwargs[key]
                sent = await channel.send(content, **send_kwargs)
                sent_messages.append(sent)
                return sent

            if str(event.get("command_style")) == "prefix":
                command_text = str(event.get("command_text") or "")
                fake_id = int(time.time() * 1_000_000)
                fake = SimpleNamespace(
                    id=fake_id, author=member, guild=guild, channel=channel,
                    content=command_text, created_at=started, webhook_id=None,
                    attachments=[], embeds=[], mentions=[], role_mentions=[], channel_mentions=[],
                    reference=None, mention_everyone=False, clean_content=command_text, _mallowcord_bridge=True,
                )

                async def fake_reply(content: Any = None, **kwargs: Any) -> discord.Message:
                    return await send_output(content, embed=kwargs.get("embed"), embeds=kwargs.get("embeds"))

                async def fake_delete(*args: Any, **kwargs: Any) -> None:
                    return None

                fake.reply = fake_reply
                fake.delete = fake_delete
                handler = getattr(self.bot, "on_message", None)
                if handler is None:
                    raise RuntimeError("Mallow-Bot's message command handler is unavailable.")
                await handler(fake)
            else:
                command_text = str(event.get("command_text") or "").strip()
                tokens = shlex.split(command_text[1:]) if command_text.startswith("/") else shlex.split(command_text)
                if not tokens:
                    raise RuntimeError("Enter a slash command name.")
                command_name = tokens.pop(0)
                command = self.bot.tree.get_command(command_name) or self.bot.tree.get_command(command_name.replace("-", "_"))
                if command is None:
                    raise RuntimeError("That slash command is not registered with Mallow-Bot.")
                if isinstance(command, app_commands.Group) and tokens:
                    subcommand = command.get_command(tokens[0]) or command.get_command(tokens[0].replace("-", "_"))
                    if subcommand is not None:
                        tokens.pop(0)
                        command = subcommand
                parameters = list(getattr(command, "parameters", []))
                values: dict[str, Any] = {}
                for index, parameter in enumerate(parameters):
                    if not tokens:
                        if getattr(parameter, "required", False):
                            raise RuntimeError(f"Missing required option: {parameter.name}.")
                        continue
                    option_type = getattr(getattr(parameter, "type", None), "value", None)
                    take_rest = index == len(parameters) - 1 and option_type == 3
                    raw = " ".join(tokens) if take_rest else tokens.pop(0)
                    if take_rest:
                        tokens.clear()
                    if option_type == 4:
                        values[parameter.name] = int(raw)
                    elif option_type == 10:
                        values[parameter.name] = float(raw)
                    elif option_type == 5:
                        values[parameter.name] = raw.casefold() in {"true", "yes", "1", "on"}
                    elif option_type == 6:
                        cleaned = re.sub(r"[<@!>]", "", raw)
                        target = guild.get_member(int(cleaned)) if cleaned.isdecimal() else discord.utils.find(
                            lambda item: item.name.casefold() == raw.casefold()
                            or item.display_name.casefold() == raw.casefold(), guild.members
                        )
                        if target is None:
                            raise RuntimeError(f"Could not find Discord member: {raw}.")
                        values[parameter.name] = target
                    elif option_type == 7:
                        cleaned = re.sub(r"[<#>]", "", raw)
                        target_channel = guild.get_channel(int(cleaned)) if cleaned.isdecimal() else discord.utils.find(
                            lambda item: item.name.casefold() == raw.casefold(), guild.channels
                        )
                        if target_channel is None:
                            raise RuntimeError(f"Could not find Discord channel: {raw}.")
                        values[parameter.name] = target_channel
                    elif option_type == 8:
                        cleaned = re.sub(r"[<@&>]", "", raw)
                        target_role = guild.get_role(int(cleaned)) if cleaned.isdecimal() else discord.utils.find(
                            lambda item: item.name.casefold() == raw.casefold(), guild.roles
                        )
                        if target_role is None:
                            raise RuntimeError(f"Could not find Discord role: {raw}.")
                        values[parameter.name] = target_role
                    else:
                        values[parameter.name] = raw

                class ResponseAdapter:
                    def __init__(self) -> None:
                        self.done = False
                    def is_done(self) -> bool:
                        return self.done
                    async def defer(self, **kwargs: Any) -> None:
                        nonlocal private_output
                        if kwargs.get("ephemeral"):
                            private_output = True
                        self.done = True
                    async def send_message(self, content: Any = None, **kwargs: Any) -> Any:
                        self.done = True
                        return await send_output(content, embed=kwargs.get("embed"), embeds=kwargs.get("embeds"), ephemeral=kwargs.get("ephemeral"))
                    async def edit_message(self, content: Any = None, **kwargs: Any) -> Any:
                        self.done = True
                        return await send_output(content, embed=kwargs.get("embed"), embeds=kwargs.get("embeds"), ephemeral=kwargs.get("ephemeral"))

                response = ResponseAdapter()

                class FollowupAdapter:
                    async def send(self, content: Any = None, **kwargs: Any) -> Any:
                        return await send_output(content, embed=kwargs.get("embed"), embeds=kwargs.get("embeds"))

                async def edit_original_response(content: Any = None, **kwargs: Any) -> Any:
                    if sent_messages:
                        return await sent_messages[-1].edit(content=content, embed=kwargs.get("embed"), view=kwargs.get("view"))
                    return await send_output(content, embed=kwargs.get("embed"), embeds=kwargs.get("embeds"), view=kwargs.get("view"))

                async def original_response() -> Any:
                    return sent_messages[-1] if sent_messages else None

                async def delete_original_response() -> None:
                    if sent_messages:
                        await sent_messages[-1].delete()

                interaction = SimpleNamespace(
                    user=member, guild=guild, guild_id=guild.id, channel=channel,
                    channel_id=channel.id, client=self.bot, command=command,
                    response=response, followup=FollowupAdapter(), data={},
                    created_at=started, extras={}, namespace=SimpleNamespace(**values),
                    permissions=channel.permissions_for(member),
                    app_permissions=channel.permissions_for(self.bot.user),
                    locale=discord.Locale.american_english,
                    guild_locale=discord.Locale.american_english,
                    command_failed=False,
                    id=int(time.time() * 1_000_000),
                    token="mallowcord-bridge",
                    type=discord.InteractionType.application_command,
                    edit_original_response=edit_original_response,
                    original_response=original_response,
                    delete_original_response=delete_original_response,
                )

                tree_check = getattr(self.bot.tree, "interaction_check", None)
                if tree_check and not await tree_check(interaction):
                    if not outputs:
                        outputs.append("You do not have permission to run that command.")
                else:
                    local_check = getattr(command, "_check_can_run", None)
                    if local_check and not await local_check(interaction):
                        outputs.append("You do not meet this command's requirements.")
                    else:
                        callback = getattr(command, "callback", None)
                        if callback is None:
                            raise RuntimeError("That slash command cannot be invoked from MallowCord.")
                        await callback(interaction, **values)

            # Capture bot-authored channel replies as well as explicit response calls.
            try:
                async for sent in channel.history(limit=20, after=started, oldest_first=True):
                    if self.bot.user and sent.author.id == self.bot.user.id:
                        rendered = sent.content or ""
                        if sent.embeds:
                            rendered += ("\n" if rendered else "") + "\n".join(self._format_embed(item) for item in sent.embeds)
                        if rendered and rendered not in outputs:
                            outputs.append(rendered)
            except discord.HTTPException:
                log.debug("Could not fetch follow-up messages after MallowCord command %s.", event.get("id"))
            result_text = "\n\n".join(outputs).strip() or "Command completed. Check the linked Discord channel for any additional output."
        except Exception as exc:
            log.exception("MallowCord command execution failed for request %s", event.get("id"))
            result_text = f"Command failed: {exc}"
        try:
            await self._request(
                "POST",
                f"/integrations/mallowbot/discord/commands/{event['id']}/complete",
                {"result": result_text[:4000], "private": private_output},
            )
        except Exception:
            log.exception("Could not return MallowCord command result for request %s", event.get("id"))

    async def _poll_commands(self) -> None:
        data = await self._request("GET", "/integrations/mallowbot/discord/commands")
        for event in data.get("commands", []):
            await self._execute_command(event)

    async def _poll_loop(self) -> None:
        while True:
            try:
                data = await self._request("GET", "/integrations/mallowbot/discord/outbox")
                for event in data.get("events", []):
                    await self._deliver(event)
                await self._poll_commands()
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
