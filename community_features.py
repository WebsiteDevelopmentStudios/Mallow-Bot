import asyncio
import json
import os
import random
import re
import time
import io
from pathlib import Path
from datetime import timedelta

import aiohttp
from PIL import Image, ImageDraw, ImageFont, ImageOps
import discord
from discord import app_commands

from dashboard_config import get_guild_config
from premium_access import is_bot_access_banned

OWNER_USER_ID = 1021068313071210498
DATA_FILE = Path(__file__).resolve().parent / "community_data.json"
INVITE_RE = re.compile(r"(?:https?://)?(?:www\.)?(?:discord\.gg|discord(?:app)?\.com/invite)/[A-Za-z0-9-]+", re.I)
YT_RE = re.compile(r"https?://(?:www\.)?(?:youtube\.com/watch\?[^\s]*v=|youtu\.be/|youtube\.com/shorts/)([A-Za-z0-9_-]{11})", re.I)

def load():
    try:
        value = json.loads(DATA_FILE.read_text(encoding="utf-8"))
        if isinstance(value, dict):
            for key in ("xp", "currency", "log_channels", "dm_routes", "timeout_minutes", "last_xp", "daily_claims", "snipes", "sticky_messages", "custom_commands", "vanity", "autoroles", "reaction_roles", "swears"):
                value.setdefault(key, {})
            return value
    except (OSError, json.JSONDecodeError):
        pass
    return {key: {} for key in ("xp", "currency", "log_channels", "dm_routes", "timeout_minutes", "last_xp", "daily_claims", "snipes", "sticky_messages", "custom_commands", "vanity", "autoroles", "reaction_roles", "swears") }

data = load()
_last_description = None
def save():
    temp = DATA_FILE.with_suffix(".tmp")
    temp.write_text(json.dumps(data, indent=2), encoding="utf-8")
    temp.replace(DATA_FILE)

def key(guild_id, user_id):
    return f"{guild_id}:{user_id}"

def level(xp):
    return int((max(0, xp) / 100) ** 0.5)

def xp_next(lvl):
    return (lvl + 1) ** 2 * 100

async def process_message(bot, message):
    if message.author.bot or await is_bot_access_banned(message.author.id):
        return
    if message.guild is None:
        # Route replies to the single server whose /dm command most recently
        # contacted this user. Never fan a private DM out across every server.
        if bot.user and message.author.id == bot.user.id:
            return
        route = data.get("dm_routes", {}).get(str(message.author.id))
        channel_id = route.get("channel_id") if isinstance(route, dict) else None
        if not channel_id:
            # Unsolicited DMs go to at most one configured owner log channel.
            channel_id = next(iter(data.get("log_channels", {}).values()), None)
        delivered = False
        if channel_id:
            try:
                channel = bot.get_channel(int(channel_id)) or await bot.fetch_channel(int(channel_id))
                if isinstance(channel, discord.TextChannel):
                    embed = discord.Embed(title="Mallow-Bot DM received", color=discord.Color.orange())
                    embed.add_field(name="From", value=f"{message.author} (ID {message.author.id})", inline=False)
                    if isinstance(route, dict) and route.get("guild_id"):
                        embed.add_field(name="Server", value=str(route.get("guild_id")), inline=False)
                    embed.add_field(name="Message", value=(message.content or "[No text content]")[:4000], inline=False)
                    embed.timestamp = message.created_at
                    await channel.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())
                    delivered = True
            except (discord.Forbidden, discord.NotFound, discord.HTTPException, ValueError, TypeError):
                pass
        try:
            await message.channel.send(
                "Your message was forwarded to the server's mod channel." if delivered
                else "I couldn't forward your message because no DM log channel is configured or accessible."
            )
        except discord.HTTPException:
            pass
        return

    guild = message.guild
    content = message.content or ""
    dashboard_config = await get_guild_config(guild.id)
    command_prefix = dashboard_config.get("prefix", "-mb")
    message_cfg = dashboard_config.get("messages_config", {})
    swear_words = [str(w).casefold().strip() for w in dashboard_config.get("swear_words", []) if isinstance(w, str) and w.strip()]
    # Count every occurrence of a configured swear word.
    # This is a counter only; it does not delete messages or punish members.
    if dashboard_config.get("features", {}).get("swear_counter", True) and content.strip() and not content.casefold().startswith(command_prefix.casefold()) and swear_words:
        folded_content = content.casefold()
        swear_count = sum(len(re.findall(r"(?<!\w)" + re.escape(word) + r"(?!\w)", folded_content)) for word in swear_words)
        if swear_count:
            skey = key(guild.id, message.author.id)
            data.setdefault("swears", {})[skey] = int(data.setdefault("swears", {}).get(skey, 0)) + swear_count
            save()

    if message_cfg.get("auto_reply_enabled") and content.strip() and not content.casefold().startswith(command_prefix.casefold()):
        folded = content.strip().casefold()
        for item in message_cfg.get("auto_replies", [])[:30]:
            trigger = str(item.get("trigger", "")).strip()
            response = str(item.get("response", "")).strip()
            mode = item.get("match_type", "exact")
            if trigger and response and ((folded == trigger.casefold()) if mode != "contains" else (trigger.casefold() in folded)):
                try:
                    await message.reply(response[:1900], mention_author=False, allowed_mentions=discord.AllowedMentions.none())
                except discord.HTTPException:
                    pass
                break
    if dashboard_config.get("features", {}).get("moderation", True) and INVITE_RE.search(content) and not message.author.guild_permissions.manage_messages:
        minutes = int(data["timeout_minutes"].get(str(guild.id), 10))
        try:
            await message.delete()
            await message.author.timeout(discord.utils.utcnow() + timedelta(minutes=minutes), reason="Discord invite links are not allowed.")
            await message.channel.send(f"{message.author.mention} you cannot post Discord links here, you have been automatically timed out for {minutes} minutes", delete_after=15, allowed_mentions=discord.AllowedMentions(users=[message.author]))
        except discord.Forbidden:
            await message.channel.send("I detected a Discord invite, but I need Manage Messages and Moderate Members permissions.", delete_after=15)
        except (discord.NotFound, discord.HTTPException):
            pass
        return

    if dashboard_config.get("features", {}).get("xp", True) and content.strip() and not content.casefold().startswith(command_prefix.casefold()):
        k = key(guild.id, message.author.id)
        now = time.time()
        xp_cfg = dashboard_config.get("xp_config", {})
        cooldown = int(xp_cfg.get("cooldown_seconds", 60))
        if now - float(data["last_xp"].get(k, 0)) >= cooldown:
            old = int(data["xp"].get(k, 0))
            new = old + random.randint(int(xp_cfg.get("min_xp", 10)), int(xp_cfg.get("max_xp", 20)))
            data["xp"][k] = new
            data["last_xp"][k] = now
            save()
            if xp_cfg.get("announce_level_up", True) and level(new) > level(old):
                await message.channel.send(f"{message.author.mention} reached level {level(new)}. Try not to let it go to your head.", delete_after=12)

    # A -mb youtube command is handled by the prefix-command listener below.
    # Skip the automatic URL preview here to avoid posting a second embed.
    yt = None if (not dashboard_config.get("features", {}).get("social", True) or content.casefold().startswith((command_prefix + " youtube").casefold())) else YT_RE.search(content)
    if yt:
        url = "https://www.youtube.com/watch?v=" + yt.group(1)
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=8)) as session:
                async with session.get("https://www.youtube.com/oembed", params={"url": url, "format": "json"}) as response:
                    if response.status == 200:
                        info = await response.json()
                        embed = discord.Embed(title=str(info.get("title", "YouTube video"))[:256], url=url, color=discord.Color.red())
                        embed.add_field(name="Channel", value=str(info.get("author_name", "Unknown"))[:256])
                        await message.reply(embed=embed, mention_author=False, allowed_mentions=discord.AllowedMentions.none())
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError, discord.HTTPException):
            pass

    # Reply to direct bot mentions with a random static description; no AI provider is called.
    # Only respond to a direct @Mallow Bot mention in a new message.
    # Mentions inside replies must not trigger the conversational greeting.
    if bot.user and bot.user in message.mentions and message.reference is None:
        description_file = Path(__file__).resolve().parent / "description.txt"
        descriptions = []
        try:
            descriptions = [
                item.strip()
                for item in description_file.read_text(encoding="utf-8").split("\n\n")
                if item.strip()
            ]
        except OSError as exc:
            print(f"[MallowBot] Could not read description.txt: {exc}", flush=True)

        if not descriptions:
            descriptions = [
                "hello im Mallow Bot... I can help with movie and YouTube links, XP and leaderboards, Mallow Coins, games, music features, and server utilities. Try -mb help or /commands to see what is available."
            ]
        global _last_description
        choices = [item for item in descriptions if item != _last_description] or descriptions
        reply = random.choice(choices)
        _last_description = reply
        if not reply.lower().startswith("hello im Mallow Bot..."):
            reply = "hello im Mallow Bot... " + reply
        await message.reply(
            reply[:1900],
            mention_author=False,
            allowed_mentions=discord.AllowedMentions.none(),
        )

async def prefix(bot, message, command, argument):
    if message.guild is None:
        return False
    gid, uid = message.guild.id, message.author.id
    guild_cfg = await get_guild_config(gid)
    feature_groups = {
        "economy": {"balance","daily","gamble","coinflip","slots","roulette","dice","blackjack","work","pay","rob"},
        "xp": {"xp","level","leaderboard"},
        "moderation": {"dm","set-log","timeout-minutes"},
        "movies": {"movie","channel"},
        "fun": {"vibecheck","ship","fortune","wyr"},
        "social": {"youtube"},
    }
    feature = next((group for group, names in feature_groups.items() if command in names), None)
    if feature and not guild_cfg.get("features", {}).get(feature, True):
        await message.reply("The " + command + " feature is disabled in this server.", mention_author=False)
        return True
    if command in set(guild_cfg.get("disabled_commands", [])):
        await message.reply("The " + command + " command is disabled in this server.", mention_author=False)
        return True
    economy_cfg = guild_cfg.get("economy_config", {})
    starting_money = int(economy_cfg.get("starting_money", 100))
    premium_commands = {"embed", "snipe", "sticky", "backup", "custom-command", "vanity", "premium-status"}
    if command in premium_commands:
        if uid != OWNER_USER_ID and command != "premium-status":
            try:
                async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=5)) as session:
                    async with session.get("https://mallowbot.kz.ci/api/premium-check", params={"discord_id": str(uid)}) as response:
                        premium_result = await response.json()
                premium_active = response.status == 200 and bool(premium_result.get("active"))
            except (aiohttp.ClientError, asyncio.TimeoutError, ValueError):
                await message.reply("I couldn't check Premium right now. Please try again shortly.", mention_author=False)
                return True
            if not premium_active:
                await message.reply("Mallow Premium is required. Get Premium at https://mallowbot.kz.ci/subscriptions.html", mention_author=False)
                return True
        if command == "premium-status":
            if uid == OWNER_USER_ID:
                await message.reply("Mallow Premium: developer access is enabled.", mention_author=False)
                return True
            try:
                async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=5)) as session:
                    async with session.get("https://mallowbot.kz.ci/api/premium-check", params={"discord_id": str(uid)}) as response:
                        result = await response.json()
                if response.status == 200 and result.get("active"):
                    await message.reply(f"Mallow Premium is active. Expires: {result.get('expires_at') or 'unknown'}", mention_author=False)
                else:
                    await message.reply("Mallow Premium is not active for your Discord account. Get Premium at https://mallowbot.kz.ci/subscriptions.html", mention_author=False)
            except (aiohttp.ClientError, asyncio.TimeoutError, ValueError):
                await message.reply("I couldn't check Premium right now. Please try again shortly.", mention_author=False)
            return True
        if command == "embed":
            if not message.author.guild_permissions.manage_messages:
                await message.reply("You need Manage Messages permission to use this command.", mention_author=False); return True
            parts = argument.split("|", 3)
            if len(parts) < 2 or not parts[0].strip() or not parts[1].strip():
                await message.reply("Usage: -mb embed <title> | <description> [| #hexcolor] [| #channel or channel ID]", mention_author=False); return True
            title, description = parts[0].strip(), parts[1].strip()
            color_text = parts[2].strip() if len(parts) > 2 and parts[2].strip() else "#8B5CF6"
            target = message.channel
            if len(parts) > 3 and parts[3].strip():
                match = re.fullmatch(r"<#(\d+)>|(\d+)", parts[3].strip())
                if not match:
                    await message.reply("Use a channel mention or channel ID for the final argument.", mention_author=False); return True
                target = message.guild.get_channel(int(match.group(1) or match.group(2)))
                if not isinstance(target, discord.TextChannel):
                    await message.reply("I couldn't find that text channel.", mention_author=False); return True
            try:
                color_value = int(color_text.lstrip("#"), 16)
                if not 0 <= color_value <= 0xFFFFFF: raise ValueError
            except ValueError:
                await message.reply("Use a valid hex color such as #8B5CF6.", mention_author=False); return True
            embed = discord.Embed(title=title[:256], description=description[:4000], color=discord.Color(color_value))
            await target.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())
            await message.reply(f"Embed sent to {target.mention}.", mention_author=False); return True
        if command == "snipe":
            entry = data.get("snipes", {}).get(str(message.channel.id))
            if not entry:
                await message.reply("There is no recently deleted message saved for this channel.", mention_author=False); return True
            embed = discord.Embed(title="Recently deleted message", description=str(entry.get("content") or "[No text content]")[:4000], color=discord.Color.orange())
            embed.add_field(name="Author", value=str(entry.get("author", "Unknown"))[:256])
            embed.set_footer(text="Deleted message content is only retained while the bot is running.")
            await message.reply(embed=embed, mention_author=False, allowed_mentions=discord.AllowedMentions.none()); return True
        if command == "sticky":
            if not message.author.guild_permissions.manage_messages:
                await message.reply("You need Manage Messages permission to use this command.", mention_author=False); return True
            sticks = data.setdefault("sticky_messages", {})
            cid = str(message.channel.id)
            if not argument.strip():
                sticks.pop(cid, None); save()
                await message.reply("Sticky message cleared for this channel.", mention_author=False); return True
            sticks[cid] = {"guild_id": gid, "content": argument[:1800], "message_id": None}
            posted = await message.channel.send("📌 " + argument[:1800], allowed_mentions=discord.AllowedMentions.none())
            sticks[cid]["message_id"] = posted.id; save()
            await message.reply("Sticky message set. Use -mb sticky without text to clear it.", mention_author=False); return True
        if command == "backup":
            if not message.author.guild_permissions.administrator:
                await message.reply("You need Administrator permission to use this command.", mention_author=False); return True
            guild = message.guild
            payload = {"guild_id": str(guild.id), "guild_name": guild.name, "created_at": discord.utils.utcnow().isoformat(),
              "roles": [{"name": r.name, "permissions": r.permissions.value, "color": r.color.value, "hoist": r.hoist, "mentionable": r.mentionable} for r in guild.roles if not r.is_default()],
              "categories": [{"name": ch.name, "position": ch.position} for ch in guild.categories],
              "channels": [{"name": ch.name, "type": str(ch.type), "position": ch.position, "category": ch.category.name if ch.category else None, "topic": getattr(ch, "topic", None)} for ch in guild.channels if isinstance(ch, (discord.TextChannel, discord.VoiceChannel, discord.CategoryChannel))]}
            raw = json.dumps(payload, indent=2).encode("utf-8")
            import io
            await message.reply("Server layout export attached. This is not a full message/permission restore backup.", file=discord.File(io.BytesIO(raw), filename=f"mallow-backup-{guild.id}.json"), mention_author=False); return True
        if command == "custom-command":
            if not message.author.guild_permissions.manage_guild:
                await message.reply("You need Manage Server permission to use this command.", mention_author=False); return True
            parts = argument.split(maxsplit=2)
            action = parts[0].lower() if parts else ""
            name = re.sub(r"[^a-z0-9_-]", "", parts[1].casefold())[:32] if len(parts) > 1 else ""
            response_text = parts[2] if len(parts) > 2 else ""
            commands_by_guild = data.setdefault("custom_commands", {}).setdefault(str(gid), {})
            if action == "list":
                await message.reply("Custom commands: " + (", ".join(commands_by_guild) if commands_by_guild else "none"), mention_author=False); return True
            if action == "delete":
                if not name or name not in commands_by_guild:
                    await message.reply("Usage: -mb custom-command delete <name>", mention_author=False); return True
                del commands_by_guild[name]; save()
                await message.reply(f"Deleted custom command {name}.", mention_author=False); return True
            if action == "create":
                if not name or not response_text.strip():
                    await message.reply("Usage: -mb custom-command create <name> <response>", mention_author=False); return True
                commands_by_guild[name] = response_text[:1500]; save()
                await message.reply(f"Created custom command {name}. Members can use -mb {name}.", mention_author=False); return True
            await message.reply("Usage: -mb custom-command create <name> <response> | list | delete <name>", mention_author=False); return True
        if command == "vanity":
            if not message.author.guild_permissions.manage_guild:
                await message.reply("You need Manage Server permission to use this command.", mention_author=False); return True
            vanity = data.setdefault("vanity", {})
            gid_text = str(gid)
            if argument.strip(): vanity[gid_text] = argument.strip()[:160]; save()
            await message.reply(f"{message.guild.name} tagline: {vanity.get(gid_text) or 'No tagline set.'}", mention_author=False); return True

    custom_commands = data.get("custom_commands", {}).get(str(gid), {})
    if command in custom_commands:
        await message.reply(str(custom_commands[command])[:1900], mention_author=False, allowed_mentions=discord.AllowedMentions.none())
        return True
    if command == "dm":
        if not message.author.guild_permissions.manage_messages:
            await message.reply("You need Manage Messages permission to use this command.", mention_author=False)
            return True
        target = message.mentions[0] if message.mentions else None
        raw_parts = argument.split(maxsplit=1)
        dm_text = raw_parts[1].strip() if len(raw_parts) > 1 else ""
        if target is None or target.bot or not dm_text or len(dm_text) > 1900:
            await message.reply("Usage: -mb dm @member <message> (message limit: 1,900 characters)", mention_author=False)
            return True
        configured_channel_id = data.get("log_channels", {}).get(str(gid))
        destination = message.guild.get_channel(int(configured_channel_id)) if configured_channel_id else None
        if not isinstance(destination, discord.TextChannel):
            await message.reply("This server doesn't have an accessible DM log/mod channel configured. Ask the bot owner to set one with -mb set-log first.", mention_author=False)
            return True
        try:
            await target.send(dm_text, allowed_mentions=discord.AllowedMentions.none())
        except (discord.Forbidden, discord.HTTPException):
            await message.reply("I couldn't DM that member. They may have DMs disabled.", mention_author=False)
            return True
        data.setdefault("dm_routes", {})[str(target.id)] = {
            "guild_id": gid,
            "channel_id": destination.id,
            "set_by": uid,
            "updated_at": int(time.time()),
        }
        save()
        await message.reply(f"Sent a DM to {target.mention}. Their replies will be forwarded only to {destination.mention}.", mention_author=False)
        return True
    if command == "set-log":
        if uid != OWNER_USER_ID:
            await message.reply("Only the bot developer can configure DM logs.", mention_author=False)
            return True
        channel = message.channel
        match = re.fullmatch(r"<#(\d+)>|(\d+)", argument.strip()) if argument else None
        if match:
            channel = message.guild.get_channel(int(match.group(1) or match.group(2)))
        if not isinstance(channel, discord.TextChannel):
            await message.reply("Usage: -mb set-log [#channel]", mention_author=False)
            return True
        data["log_channels"][str(gid)] = channel.id
        save()
        await message.reply(f"DM conversations will be logged to {channel.mention}.", mention_author=False)
        return True
    if command == "timeout-minutes":
        if not message.author.guild_permissions.manage_guild:
            await message.reply("You need Manage Server permission to change this setting.", mention_author=False)
            return True
        try:
            minutes = int(argument)
            if not 1 <= minutes <= 10080:
                raise ValueError
        except ValueError:
            await message.reply("Usage: -mb timeout-minutes 1-10080", mention_author=False)
            return True
        data["timeout_minutes"][str(gid)] = minutes
        save()
        await message.reply(f"Discord invite timeout set to {minutes} minutes.", mention_author=False)
        return True
    if command in ("xp", "level"):
        xp = int(data["xp"].get(key(gid, uid), 0))
        lvl = level(xp)
        await message.reply(f"{message.author.mention}: Level {lvl} · {xp} XP · {max(0, xp_next(lvl)-xp)} XP to next level.", mention_author=False)
        return True
    if command == "leaderboard":
        entries = [(int(k.split(":")[1]), int(v)) for k, v in data["xp"].items() if k.startswith(f"{gid}:")]
        entries.sort(key=lambda item: item[1], reverse=True)
        lines = []
        for i, (user_id, xp) in enumerate(entries[:10], 1):
            member = message.guild.get_member(user_id)
            mention = member.mention if member else f"<@{user_id}>"
            lines.append(f"**{i}.** {mention} — **{xp:,} XP** · Level {level(xp)}")
        embed = discord.Embed(
            title=f"{message.guild.name} · XP Leaderboard",
            description="\n".join(lines) if lines else "No XP has been earned yet.",
            color=discord.Color.gold(),
        )
        embed.set_footer(text="XP is earned from eligible chat messages.")
        await message.reply(
            embed=embed,
            mention_author=False,
            allowed_mentions=discord.AllowedMentions(users=True, roles=False, everyone=False),
        )
        return True
    if command in ("youtube",):
        match = YT_RE.search(argument)
        if not match:
            await message.reply("Usage: -mb youtube <YouTube URL>", mention_author=False)
            return True
        video_url = "https://www.youtube.com/watch?v=" + match.group(1)
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=8)) as session:
                async with session.get("https://www.youtube.com/oembed", params={"url": video_url, "format": "json"}) as response:
                    if response.status != 200:
                        await message.reply("YouTube didn't return video information for that link.", mention_author=False)
                        return True
                    info = await response.json()
            embed = discord.Embed(title=str(info.get("title", "YouTube video"))[:256], url=video_url, color=discord.Color.red())
            embed.add_field(name="Channel", value=str(info.get("author_name", "Unknown"))[:256])
            await message.reply(embed=embed, mention_author=False)
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError, discord.HTTPException):
            await message.reply("I couldn't look up that video right now.", mention_author=False)
        return True
    if command == "gamble":
        k = key(gid, uid)
        balance = int(data["currency"].get(k, starting_money))
        try:
            bet = int(argument.split()[0]) if argument else 10
        except ValueError:
            bet = 0
        if bet < 1 or bet > balance:
            await message.reply(f"Bet must be between 1 and your balance ({balance}).", mention_author=False)
            return True
        won = random.choice([True, False])
        if won:
            balance += bet
            result = f"You won {bet} Mallow Coins."
        else:
            balance -= bet
            result = f"You lost {bet} Mallow Coins."
        data["currency"][k] = balance
        save()
        await message.reply(f"{result} Balance: {balance} Mallow Coins.", mention_author=False)
        return True
    if command in ("coinflip", "slots", "roulette", "dice", "blackjack"):
        k = key(gid, uid)
        balance = int(data["currency"].get(k, 100))
        try:
            bet = int(argument.split()[0]) if argument else 10
        except ValueError:
            bet = 0
        if bet < 1 or bet > balance:
            await message.reply(f"Bet must be between 1 and your balance ({balance}).", mention_author=False)
            return True
        if command == "coinflip":
            won = random.choice([True, False])
            payout = bet if won else 0
            detail = "Heads!" if won else "Tails!"
        elif command == "slots":
            symbols = [random.choice(["🍒", "🍋", "🍇", "🔔", "7️⃣"]) for _ in range(3)]
            won = len(set(symbols)) == 1 or len(set(symbols)) == 2
            payout = bet * (5 if len(set(symbols)) == 1 else 2) if won else 0
            detail = " ".join(symbols)
        elif command == "roulette":
            roll = random.randint(0, 36)
            won = roll % 2 == 0
            payout = bet if won else 0
            detail = f"Roulette landed on {roll} ({'even' if roll % 2 == 0 else 'odd'})."
        elif command == "dice":
            roll = random.randint(1, 6)
            won = roll >= 4
            payout = bet if won else 0
            detail = f"You rolled {roll}."
        else:
            player = random.randint(12, 21)
            dealer = random.randint(15, 21)
            won = player > dealer
            payout = bet if won else (bet * 2 if player == 21 else 0)
            detail = f"You: {player} · Dealer: {dealer}."
        balance = balance - bet + payout
        data["currency"][k] = balance
        save()
        outcome = f"Won {payout} Mallow Coins." if payout else f"Lost {bet} Mallow Coins."
        await message.reply(f"{detail} {outcome} Balance: {balance} Mallow Coins.", mention_author=False)
        return True
    if command == "work":
        k = key(gid, uid)
        earned = random.randint(int(economy_cfg.get("work_min", 20)), int(economy_cfg.get("work_max", 80)))
        data["currency"][k] = int(data["currency"].get(k, starting_money)) + earned
        save()
        await message.reply(f"You worked a shift and earned {earned} Mallow Coins.", mention_author=False)
        return True
    if command == "rob":
        target = message.mentions[0] if message.mentions else None
        if target is None or target.bot or target.id == uid:
            await message.reply("Usage: -mb rob @member", mention_author=False)
            return True
        target_key = key(gid, target.id)
        target_balance = int(data["currency"].get(target_key, starting_money))
        if target_balance < 1:
            await message.reply("That member has no Mallow Coins to steal.", mention_author=False)
            return True
        if random.random() < 0.35:
            stolen = min(target_balance, random.randint(1, max(1, target_balance // 4)))
            data["currency"][target_key] = target_balance - stolen
            data["currency"][key(gid, uid)] = int(data["currency"].get(key(gid, uid), starting_money)) + stolen
            result = f"You stole {stolen} Mallow Coins from {target.display_name}."
        else:
            fine = min(int(data["currency"].get(key(gid, uid), starting_money)), 25)
            data["currency"][key(gid, uid)] = int(data["currency"].get(key(gid, uid), 100)) - fine
            result = f"You got caught and paid a {fine}-coin fine."
        save()
        await message.reply(result, mention_author=False)
        return True
    if command == "pay":
        target = message.mentions[0] if message.mentions else None
        parts = argument.split()
        try:
            amount = int(parts[-1])
        except (ValueError, IndexError):
            amount = 0
        k = key(gid, uid)
        if target is None or target.bot or target.id == uid or amount < 1:
            await message.reply("Usage: -mb pay @member <amount>", mention_author=False)
            return True
        balance = int(data["currency"].get(k, 100))
        if amount > balance:
            await message.reply(f"You only have {balance} Mallow Coins.", mention_author=False)
            return True
        tk = key(gid, target.id)
        data["currency"][k] = balance - amount
        data["currency"][tk] = int(data["currency"].get(tk, starting_money)) + amount
        save()
        await message.reply(f"Paid {amount} Mallow Coins to {target.display_name}.", mention_author=False)
        return True
    if command == "vibecheck":
        target = message.mentions[0] if message.mentions else message.author
        score = random.randint(0, 100)
        verdict = "legendary energy" if score >= 90 else "main-character energy" if score >= 70 else "mysterious side-character energy" if score >= 40 else "needs a snack and a nap"
        await message.reply(f"📡 Vibe scan for **{target.display_name}**: **{score}/100** — {verdict}.", mention_author=False)
        return True
    if command == "ship":
        targets = message.mentions
        if len(targets) >= 2:
            first, second = targets[0], targets[1]
        elif len(targets) == 1:
            first, second = message.author, targets[0]
        else:
            await message.reply(
                "Usage: -mb ship [optional @member] @member — if you provide one member, I'll pair them with you.",
                mention_author=False,
            )
            return True
        if first.bot or second.bot or first.id == second.id:
            await message.reply("Choose two different human members. This is just for fun!", mention_author=False)
            return True
        score = random.randint(0, 100)
        verdict = "cinematic soulmates 💞" if score >= 90 else "power couple material ✨" if score >= 70 else "chaotic duo 🤝" if score >= 40 else "better as movie buddies 🍿"
        await message.reply(f"💘 **{first.display_name} + {second.display_name}** = **{score}%** — {verdict}. (For fun only!)", mention_author=False)
        return True
    if command == "fortune":
        fortunes = ["A plot twist is closer than you think.", "Today, your side quest becomes the main story.", "The next message you send will be oddly important. Probably.", "A small win is loading. Please do not refresh.", "Your future contains snacks and questionable decisions.", "The universe says yes—but asks you to read the patch notes first.", "Someone in this server owes you a high five.", "Avoid suspiciously confident people bearing spoilers."]
        await message.reply(f"🔮 **Your Mallow Bot fortune:** {random.choice(fortunes)}", mention_author=False)
        return True
    if command == "wyr":
        questions = ["Would you rather live inside your favorite movie or have your life directed by your least favorite director?", "Would you rather know every movie spoiler in advance or never be able to watch a trailer again?", "Would you rather have unlimited Mallow Coins or pick the movie every movie night forever?", "Would you rather be the hero with terrible luck or the villain with a perfect plan?", "Would you rather only watch sequels or only watch movies with cliffhanger endings?", "Would you rather have a personal theme song or dramatic slow motion whenever you enter a room?", "Would you rather swap lives with a fictional character for a week or meet the actor who plays them?", "Would you rather erase one bad movie from existence or make one cancelled show get a perfect finale?"]
        await message.reply(f"🎬 **Would You Rather?**\n{random.choice(questions)}\n\nReact with your choice or explain your answer!", mention_author=False)
        return True
    if command == "balance":
        await message.reply(f"You have {int(data['currency'].get(key(gid,uid),starting_money))} Mallow Coins.", mention_author=False)
        return True
    if command == "daily":
        k = key(gid, uid)
        now = time.time()
        last_claim = float(data["daily_claims"].get(k, 0))
        remaining = 86400 - (now - last_claim)
        if remaining > 0:
            hours, remainder = divmod(int(remaining), 3600)
            minutes = (remainder + 59) // 60
            await message.reply(f"You've already claimed your daily Mallow Coins. Try again in {hours}h {minutes}m.", mention_author=False)
            return True
        data["currency"][k] = int(data["currency"].get(k, starting_money)) + int(economy_cfg.get("daily_reward", 100))
        data["daily_claims"][k] = now
        save()
        await message.reply("Claimed " + str(int(economy_cfg.get("daily_reward", 100))) + " Mallow Coins. Try not to lose them immediately.", mention_author=False)
        return True
    if command in ("coinflip", "slots"):
        k = key(gid,uid)
        balance = int(data["currency"].get(k,starting_money))
        try:
            bet = int(argument.split()[0]) if argument else 10
        except ValueError:
            bet = 0
        if bet < 1 or bet > balance:
            await message.reply(f"Bet must be between 1 and your balance ({balance}).", mention_author=False)
            return True
        won = random.choice([True,False]) if command == "coinflip" else random.random() < 0.38
        if won:
            balance += bet if command == "coinflip" else bet * 2
            result = f"You won {bet if command == 'coinflip' else bet*2} Mallow Coins."
        else:
            balance -= bet
            result = f"You lost {bet} Mallow Coins. Excellent financial strategy."
        data["currency"][k] = balance
        save()
        await message.reply(f"{result} Balance: {balance} Mallow Coins.", mention_author=False)
        return True
    return False

async def play_game(interaction: discord.Interaction, game: str, bet: int):
    if interaction.guild_id is None:
        await interaction.response.send_message("Use this in a server.", ephemeral=True); return
    cfg = await get_guild_config(interaction.guild_id)
    start = int(cfg.get("economy_config", {}).get("starting_money", 100))
    k=key(interaction.guild_id,interaction.user.id)
    balance=int(data["currency"].get(k,start))
    if bet>balance:
        await interaction.response.send_message(f"You only have {balance} Mallow Coins.",ephemeral=True); return
    if game == "coinflip":
        won=random.choice([True,False]); payout=bet if won else 0; detail="You won the coin flip!" if won else "You lost the coin flip."
    elif game == "slots":
        symbols=[random.choice(["🍒","🍋","🍇","🔔","7️⃣"]) for _ in range(3)]
        mult=5 if len(set(symbols))==1 else (2 if len(set(symbols))==2 else 0)
        payout=bet*mult; detail=" ".join(symbols); won=payout>0
    elif game == "roulette":
        roll=random.randint(0,36); won=roll%2==0; payout=bet if won else 0; detail=f"Roulette: {roll} ({'even' if roll%2==0 else 'odd'})."
    elif game == "dice":
        roll=random.randint(1,6); won=roll>=4; payout=bet if won else 0; detail=f"You rolled {roll}."
    else:
        player=random.randint(12,21); dealer=random.randint(15,21); won=player>dealer; payout=bet if won else (bet*2 if player==21 else 0); detail=f"Your hand: {player} · Dealer: {dealer}."
    balance=balance-bet+payout
    data["currency"][k]=balance; save()
    outcome=f"Won {payout} Mallow Coins." if payout else f"Lost {bet} Mallow Coins."
    await interaction.response.send_message(f"{detail} {outcome} Balance: {balance} Mallow Coins.",ephemeral=True)


def setup(bot):
    autorole_group = app_commands.Group(name="autorole", description="Configure automatic member roles.")
    reaction_role_group = app_commands.Group(name="reaction-role", description="Create and manage reaction-role panels.")

    def role_is_assignable(guild: discord.Guild, role: discord.Role) -> bool:
        me = guild.me
        return bool(me and me.guild_permissions.manage_roles and not role.is_default() and not role.managed and role < me.top_role)

    @autorole_group.command(name="set", description="Give a role to every new member.")
    @app_commands.checks.has_permissions(manage_roles=True)
    @app_commands.describe(role="Role automatically given to new members")
    async def autorole_set(interaction: discord.Interaction, role: discord.Role):
        guild = interaction.guild
        if guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        if not role_is_assignable(guild, role):
            await interaction.response.send_message("I can't assign that role. Make sure Mallow has Manage Roles and its highest role is above the selected role.", ephemeral=True)
            return
        data["autoroles"][str(guild.id)] = role.id
        save()
        await interaction.response.send_message(f"Auto-role enabled. New members will receive {role.mention}.", ephemeral=True)

    @autorole_group.command(name="disable", description="Disable automatic member roles.")
    @app_commands.checks.has_permissions(manage_roles=True)
    async def autorole_disable(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        data["autoroles"].pop(str(interaction.guild_id), None)
        save()
        await interaction.response.send_message("Auto-role has been disabled for this server.", ephemeral=True)

    @autorole_group.command(name="status", description="Show the configured automatic member role.")
    @app_commands.checks.has_permissions(manage_roles=True)
    async def autorole_status(interaction: discord.Interaction):
        if interaction.guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        role_id = data["autoroles"].get(str(interaction.guild.id))
        role = interaction.guild.get_role(int(role_id)) if str(role_id or "").isdigit() else None
        await interaction.response.send_message(
            f"New members will receive {role.mention}." if role else "Auto-role is not configured. Use /autorole set to choose a role.",
            ephemeral=True,
        )

    @reaction_role_group.command(name="create", description="Create a reaction-role panel in a channel.")
    @app_commands.checks.has_permissions(manage_roles=True)
    @app_commands.describe(channel="Where to post the panel", title="Panel title", description="Instructions shown above the role list")
    async def reaction_role_create(interaction: discord.Interaction, channel: discord.TextChannel, title: str = "Choose your roles", description: str = "React below to get a role. Remove your reaction to remove it."):
        if interaction.guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        embed = discord.Embed(title=title[:256], description=description[:3500], color=discord.Color.blurple())
        embed.set_footer(text="Server reaction roles")
        try:
            message = await channel.send(embed=embed)
        except (discord.Forbidden, discord.HTTPException):
            await interaction.response.send_message("I couldn't post in that channel. Check my View Channel, Send Messages, and Embed Links permissions.", ephemeral=True)
            return
        data["reaction_roles"][str(message.id)] = {
            "guild_id": str(interaction.guild.id),
            "channel_id": str(channel.id),
            "title": title[:256],
            "roles": {},
        }
        save()
        await interaction.response.send_message(f"Reaction-role panel created: {message.jump_url}\nUse /reaction-role add to connect an emoji and role.", ephemeral=True)

    @reaction_role_group.command(name="add", description="Connect an emoji on a panel to a role.")
    @app_commands.checks.has_permissions(manage_roles=True)
    @app_commands.describe(message_id="ID of a panel created with /reaction-role create", emoji="Emoji users react with", role="Role to give while the reaction is present")
    async def reaction_role_add(interaction: discord.Interaction, message_id: str, emoji: str, role: discord.Role):
        guild = interaction.guild
        if guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        if not message_id.isdigit():
            await interaction.response.send_message("Message ID must contain only numbers.", ephemeral=True)
            return
        panel = data["reaction_roles"].get(message_id)
        if not panel or panel.get("guild_id") != str(guild.id):
            await interaction.response.send_message("That panel wasn't found in this server. Create one with /reaction-role create first.", ephemeral=True)
            return
        if not role_is_assignable(guild, role):
            await interaction.response.send_message("I can't assign that role. Make sure Mallow has Manage Roles and its highest role is above the selected role.", ephemeral=True)
            return
        emoji_value = emoji.strip()
        if not emoji_value or len(emoji_value) > 100:
            await interaction.response.send_message("Enter a valid Unicode emoji or custom emoji.", ephemeral=True)
            return
        channel = guild.get_channel(int(panel.get("channel_id", "0")))
        if not isinstance(channel, discord.TextChannel):
            await interaction.response.send_message("The panel channel no longer exists.", ephemeral=True)
            return
        try:
            message = await channel.fetch_message(int(message_id))
            await message.add_reaction(emoji_value)
        except (discord.NotFound, discord.Forbidden, discord.HTTPException):
            await interaction.response.send_message("I couldn't find the panel or add that reaction. Check my channel permissions and the emoji.", ephemeral=True)
            return
        panel.setdefault("roles", {})[emoji_value] = role.id
        save()
        await interaction.response.send_message(f"Added {emoji_value} → {role.mention}. React to the panel to receive the role; remove your reaction to lose it.", ephemeral=True)

    @reaction_role_group.command(name="remove", description="Remove an emoji-to-role mapping from a panel.")
    @app_commands.checks.has_permissions(manage_roles=True)
    @app_commands.describe(message_id="ID of the reaction-role panel", emoji="Emoji mapping to remove")
    async def reaction_role_remove(interaction: discord.Interaction, message_id: str, emoji: str):
        if interaction.guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        panel = data["reaction_roles"].get(message_id)
        if not panel or panel.get("guild_id") != str(interaction.guild.id):
            await interaction.response.send_message("That panel wasn't found in this server.", ephemeral=True)
            return
        emoji_value = emoji.strip()
        roles = panel.get("roles", {})
        if emoji_value not in roles:
            await interaction.response.send_message("That emoji isn't connected to a role on this panel.", ephemeral=True)
            return
        roles.pop(emoji_value, None)
        save()
        await interaction.response.send_message(f"Removed the {emoji_value} role mapping. The reaction no longer changes roles.", ephemeral=True)

    @reaction_role_group.command(name="list", description="List the role mappings on a panel.")
    @app_commands.checks.has_permissions(manage_roles=True)
    @app_commands.describe(message_id="ID of the reaction-role panel")
    async def reaction_role_list(interaction: discord.Interaction, message_id: str):
        if interaction.guild is None:
            await interaction.response.send_message("Use this command in a server.", ephemeral=True)
            return
        panel = data["reaction_roles"].get(message_id)
        if not panel or panel.get("guild_id") != str(interaction.guild.id):
            await interaction.response.send_message("That panel wasn't found in this server.", ephemeral=True)
            return
        lines = []
        for emoji_value, role_id in panel.get("roles", {}).items():
            role = interaction.guild.get_role(int(role_id))
            lines.append(f"{emoji_value} → {role.mention if role else 'deleted role'}")
        await interaction.response.send_message("\n".join(lines) if lines else "This panel has no role mappings yet.", ephemeral=True)

    async def auto_role_member_join_listener(member: discord.Member):
        role_id = data["autoroles"].get(str(member.guild.id))
        if not str(role_id or "").isdigit():
            return
        role = member.guild.get_role(int(role_id))
        if role is None or not role_is_assignable(member.guild, role):
            return
        try:
            await member.add_roles(role, reason="Mallow Bot automatic join role")
        except (discord.Forbidden, discord.HTTPException):
            pass

    async def reaction_role_listener(payload: discord.RawReactionActionEvent, *, added: bool):
        if payload.guild_id is None or bot.user is None or payload.user_id == bot.user.id:
            return
        panel = data["reaction_roles"].get(str(payload.message_id))
        if not panel or panel.get("guild_id") != str(payload.guild_id):
            return
        role_id = panel.get("roles", {}).get(str(payload.emoji))
        if not str(role_id or "").isdigit():
            return
        guild = bot.get_guild(payload.guild_id)
        if guild is None:
            return
        member = payload.member if added else guild.get_member(payload.user_id)
        if member is None:
            try:
                member = await guild.fetch_member(payload.user_id)
            except (discord.NotFound, discord.Forbidden, discord.HTTPException):
                return
        role = guild.get_role(int(role_id))
        if role is None or not role_is_assignable(guild, role):
            return
        try:
            if added:
                await member.add_roles(role, reason="Mallow Bot reaction role")
            else:
                await member.remove_roles(role, reason="Mallow Bot reaction role removed")
        except (discord.Forbidden, discord.HTTPException):
            pass

    async def reaction_role_added(payload: discord.RawReactionActionEvent):
        await reaction_role_listener(payload, added=True)

    async def reaction_role_removed(payload: discord.RawReactionActionEvent):
        await reaction_role_listener(payload, added=False)

    bot.tree.add_command(autorole_group)
    bot.tree.add_command(reaction_role_group)
    @bot.tree.command(name="set-log", description="Set the bot DM log channel (developer only).")
    @app_commands.describe(channel="Channel that receives incoming bot DMs")
    async def set_log_slash(interaction: discord.Interaction, channel: discord.TextChannel):
        if interaction.user.id != OWNER_USER_ID:
            await interaction.response.send_message("Only the bot developer can configure DM logs.", ephemeral=True)
            return
        data["log_channels"][str(interaction.guild_id)] = channel.id
        save()
        await interaction.response.send_message(f"DM conversations will be logged to {channel.mention}.", ephemeral=True)

    @bot.tree.command(name="xp", description="Show your XP and level.")
    async def xp_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        xp = int(data["xp"].get(key(interaction.guild_id, interaction.user.id), 0))
        lvl = level(xp)
        await interaction.response.send_message(f"Level {lvl} · {xp} XP · {max(0,xp_next(lvl)-xp)} XP to next level.", ephemeral=True)

    @bot.tree.command(name="leaderboard", description="Show the server XP leaderboard.")
    async def leaderboard_slash(interaction: discord.Interaction):
        if interaction.guild is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        gid = interaction.guild.id
        entries = [(int(k.split(":")[1]), int(v)) for k, v in data["xp"].items() if k.startswith(f"{gid}:")]
        entries.sort(key=lambda item: item[1], reverse=True)
        lines = []
        for i, (uid, xp) in enumerate(entries[:10], 1):
            member = interaction.guild.get_member(uid)
            mention = member.mention if member else f"<@{uid}>"
            lines.append(f"**{i}.** {mention} — **{xp:,} XP** · Level {level(xp)}")
        embed = discord.Embed(
            title=f"{interaction.guild.name} · XP Leaderboard",
            description="\n".join(lines) if lines else "No XP has been earned yet.",
            color=discord.Color.gold(),
        )
        embed.set_footer(text="XP is earned from eligible chat messages.")
        await interaction.response.send_message(
            embed=embed,
            allowed_mentions=discord.AllowedMentions(users=True, roles=False, everyone=False),
        )

    @bot.tree.command(name="coinflip", description="Bet virtual Mallow Coins on a coin flip.")
    @app_commands.describe(bet="Amount of virtual coins to wager")
    async def coinflip_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        await play_game(interaction, "coinflip", bet)

    @bot.tree.command(name="slots", description="Play the virtual slot machine.")
    @app_commands.describe(bet="Amount of virtual coins to wager")
    async def slots_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        await play_game(interaction, "slots", bet)

    @bot.tree.command(name="roulette", description="Bet virtual coins on even or odd roulette.")
    @app_commands.describe(bet="Amount of virtual coins to wager")
    async def roulette_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        await play_game(interaction, "roulette", bet)

    @bot.tree.command(name="dice", description="Roll dice and bet virtual coins.")
    @app_commands.describe(bet="Amount of virtual coins to wager")
    async def dice_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        await play_game(interaction, "dice", bet)

    @bot.tree.command(name="blackjack", description="Play a quick virtual blackjack round.")
    @app_commands.describe(bet="Amount of virtual coins to wager")
    async def blackjack_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        await play_game(interaction, "blackjack", bet)

    @bot.tree.command(name="work", description="Work to earn virtual Mallow Coins.")
    async def work_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        eco = cfg.get("economy_config", {})
        start = int(eco.get("starting_money", 100))
        earned = random.randint(int(eco.get("work_min", 20)), int(eco.get("work_max", 80)))
        k=key(interaction.guild_id, interaction.user.id)
        data["currency"][k]=int(data["currency"].get(k,start))+earned
        save()
        await interaction.response.send_message(f"You earned {earned} Mallow Coins. Balance: {data['currency'][k]}.",ephemeral=True)

    @bot.tree.command(name="pay", description="Give virtual Mallow Coins to a member.")
    @app_commands.describe(member="Member to pay", amount="Amount of virtual coins")
    async def pay_slash(interaction: discord.Interaction, member: discord.Member, amount: app_commands.Range[int,1,1000000]):
        if interaction.guild_id is None or member.bot or member.id == interaction.user.id:
            await interaction.response.send_message("Choose another server member.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        start = int(cfg.get("economy_config", {}).get("starting_money", 100))
        k=key(interaction.guild_id,interaction.user.id); tk=key(interaction.guild_id,member.id)
        balance=int(data["currency"].get(k,start))
        if amount>balance:
            await interaction.response.send_message(f"You only have {balance} Mallow Coins.",ephemeral=True); return
        data["currency"][k]=balance-amount
        data["currency"][tk]=int(data["currency"].get(tk,start))+amount
        save()
        await interaction.response.send_message(f"Paid {amount} Mallow Coins to {member.display_name}.",ephemeral=True)

    @bot.tree.command(name="rob", description="Try to steal virtual Mallow Coins from a member.")
    @app_commands.describe(member="Member to rob")
    async def rob_slash(interaction: discord.Interaction, member: discord.Member):
        if interaction.guild_id is None or member.bot or member.id == interaction.user.id:
            await interaction.response.send_message("Choose another server member.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        start = int(cfg.get("economy_config", {}).get("starting_money", 100))
        tk=key(interaction.guild_id,member.id); k=key(interaction.guild_id,interaction.user.id)
        target_balance=int(data["currency"].get(tk,start))
        if target_balance<1:
            await interaction.response.send_message("That member has no Mallow Coins.",ephemeral=True); return
        if random.random()<0.35:
            stolen=min(target_balance,random.randint(1,max(1,target_balance//4)))
            data["currency"][tk]=target_balance-stolen
            data["currency"][k]=int(data["currency"].get(k,start))+stolen
            result=f"You stole {stolen} Mallow Coins from {member.display_name}."
        else:
            fine=min(int(data["currency"].get(k,100)),25)
            data["currency"][k]=int(data["currency"].get(k,100))-fine
            result=f"You got caught and paid a {fine}-coin fine."
        save()
        await interaction.response.send_message(result,ephemeral=True)

    @bot.tree.command(name="balance", description="Show your virtual Mallow Coins.")
    async def balance_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        start = int(cfg.get("economy_config", {}).get("starting_money", 100))
        balance = int(data["currency"].get(key(interaction.guild_id, interaction.user.id), start))
        await interaction.response.send_message(f"You have {balance} Mallow Coins.", ephemeral=True)

    @bot.tree.command(name="daily", description="Claim 100 virtual Mallow Coins once every 24 hours.")
    async def daily_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        eco = cfg.get("economy_config", {})
        start = int(eco.get("starting_money", 100))
        reward = int(eco.get("daily_reward", 100))
        k = key(interaction.guild_id, interaction.user.id)
        now = time.time()
        last_claim = float(data["daily_claims"].get(k, 0))
        remaining = 86400 - (now - last_claim)
        if remaining > 0:
            hours, remainder = divmod(int(remaining), 3600)
            minutes = (remainder + 59) // 60
            await interaction.response.send_message(
                f"You've already claimed your daily Mallow Coins. Try again in {hours}h {minutes}m.",
                ephemeral=True,
            )
            return
        data["currency"][k] = int(data["currency"].get(k, start)) + reward
        data["daily_claims"][k] = now
        save()
        await interaction.response.send_message("Claimed " + str(reward) + " Mallow Coins.", ephemeral=True)

    @bot.tree.command(name="gamble", description="Bet virtual Mallow Coins on a coin flip.")
    @app_commands.describe(bet="Amount of virtual currency to wager")
    async def gamble_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        cfg = await get_guild_config(interaction.guild_id)
        start = int(cfg.get("economy_config", {}).get("starting_money", 100))
        k=key(interaction.guild_id,interaction.user.id)
        balance=int(data["currency"].get(k,start))
        if bet>balance:
            await interaction.response.send_message(f"You only have {balance} Mallow Coins.",ephemeral=True); return
        if random.choice([True,False]):
            balance += bet; result=f"You won {bet} Mallow Coins."
        else:
            balance -= bet; result=f"You lost {bet} Mallow Coins."
        data["currency"][k]=balance; save()
        await interaction.response.send_message(f"{result} New balance: {balance} Mallow Coins.",ephemeral=True)

    @bot.tree.command(name="vibecheck", description="Run a completely unscientific vibe scan.")
    @app_commands.describe(member="Who should get their vibe checked? Leave empty for yourself.")
    async def vibecheck_slash(interaction: discord.Interaction, member: discord.Member | None = None):
        target = member or interaction.user
        score = random.randint(0, 100)
        verdict = "legendary energy" if score >= 90 else "main-character energy" if score >= 70 else "mysterious side-character energy" if score >= 40 else "needs a snack and a nap"
        await interaction.response.send_message(f"📡 Vibe scan for **{target.display_name}**: **{score}/100** — {verdict}.", allowed_mentions=discord.AllowedMentions.none())

    @bot.tree.command(name="ship", description="Get a just-for-fun compatibility score for two members.")
    @app_commands.describe(member="The other half of this totally scientific pairing")
    async def ship_slash(interaction: discord.Interaction, member: discord.Member):
        if member.bot or member.id == interaction.user.id:
            await interaction.response.send_message("Choose another human member. This is just for fun!", ephemeral=True)
            return
        pair = ":".join(map(str, sorted((interaction.user.id, member.id)))) + f":{interaction.guild_id}"
        score = int(__import__("hashlib").sha256(pair.encode()).hexdigest()[:8], 16) % 101
        verdict = "cinematic soulmates 💞" if score >= 90 else "power couple material ✨" if score >= 70 else "chaotic duo 🤝" if score >= 40 else "better as movie buddies 🍿"
        await interaction.response.send_message(f"💘 **{interaction.user.display_name} + {member.display_name}** = **{score}%** — {verdict}. (For fun only!)", allowed_mentions=discord.AllowedMentions.none())

    @bot.tree.command(name="fortune", description="Draw a random Mallow Bot fortune.")
    async def fortune_slash(interaction: discord.Interaction):
        fortunes = ["A plot twist is closer than you think.", "Today, your side quest becomes the main story.", "The next message you send will be oddly important. Probably.", "A small win is loading. Please do not refresh.", "Your future contains snacks and questionable decisions.", "The universe says yes—but asks you to read the patch notes first.", "Someone in this server owes you a high five.", "Avoid suspiciously confident people bearing spoilers."]
        await interaction.response.send_message(f"🔮 **Your Mallow Bot fortune:** {random.choice(fortunes)}")

    @bot.tree.command(name="wyr", description="Get a movie-themed Would You Rather question.")
    async def wyr_slash(interaction: discord.Interaction):
        questions = ["Would you rather live inside your favorite movie or have your life directed by your least favorite director?", "Would you rather know every movie spoiler in advance or never be able to watch a trailer again?", "Would you rather have unlimited Mallow Coins or pick the movie every movie night forever?", "Would you rather be the hero with terrible luck or the villain with a perfect plan?", "Would you rather only watch sequels or only watch movies with cliffhanger endings?", "Would you rather have a personal theme song or dramatic slow motion whenever you enter a room?", "Would you rather swap lives with a fictional character for a week or meet the actor who plays them?", "Would you rather erase one bad movie from existence or make one cancelled show get a perfect finale?"]
        await interaction.response.send_message(f"🎬 **Would You Rather?**\n{random.choice(questions)}\n\nReact with your choice or explain your answer!")


    async def premium_gate(interaction: discord.Interaction) -> bool:
        if interaction.guild is None:
            await interaction.response.send_message("Use this Premium command in a server.", ephemeral=True)
            return False
        if interaction.user.id == OWNER_USER_ID:
            return True
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=5)) as session:
                async with session.get("https://mallowbot.kz.ci/api/premium-check", params={"discord_id": str(interaction.user.id)}) as response:
                    result = await response.json()
            if response.status == 200 and result.get("active"):
                return True
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError):
            pass
        await interaction.response.send_message("👑 **Mallow Premium required**\nYou can run this command, but need an active Premium subscription. Get Premium at https://mallowbot.kz.ci/subscriptions.html", ephemeral=True)
        return False

    @bot.tree.command(name="embed", description="Create and send a custom embed (Premium).")
    @app_commands.describe(title="Embed title", description="Embed text", color="Hex color such as #8B5CF6", channel="Where to send the embed")
    @app_commands.checks.has_permissions(manage_messages=True)
    async def premium_embed(interaction: discord.Interaction, title: str, description: str, color: str = "#8B5CF6", channel: discord.TextChannel | None = None):
        if not await premium_gate(interaction): return
        try:
            color_value = int(color.strip().lstrip("#"), 16)
            if not 0 <= color_value <= 0xFFFFFF: raise ValueError
        except ValueError:
            await interaction.response.send_message("Use a valid hex color, for example #8B5CF6.", ephemeral=True); return
        embed = discord.Embed(title=title[:256], description=description[:4000], color=discord.Color(color_value))
        target = channel or interaction.channel
        await target.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())
        await interaction.response.send_message(f"Embed sent to {target.mention}.", ephemeral=True)

    @bot.tree.command(name="snipe", description="Show the most recently deleted message in this channel (Premium).")
    async def premium_snipe(interaction: discord.Interaction):
        if not await premium_gate(interaction): return
        entry = data.get("snipes", {}).get(str(interaction.channel_id))
        if not entry:
            await interaction.response.send_message("There is no recently deleted message saved for this channel.", ephemeral=True); return
        embed = discord.Embed(title="Recently deleted message", description=str(entry.get("content") or "[No text content]")[:4000], color=discord.Color.orange())
        embed.add_field(name="Author", value=str(entry.get("author", "Unknown"))[:256])
        embed.set_footer(text="Deleted message content is only retained while the bot is running.")
        await interaction.response.send_message(embed=embed, ephemeral=True, allowed_mentions=discord.AllowedMentions.none())

    @bot.tree.command(name="sticky", description="Set or clear a sticky message in this channel (Premium).")
    @app_commands.describe(message="Sticky text; leave blank to clear the sticky message")
    @app_commands.checks.has_permissions(manage_messages=True)
    async def premium_sticky(interaction: discord.Interaction, message: str = ""):
        if not await premium_gate(interaction): return
        sticks = data.setdefault("sticky_messages", {})
        cid = str(interaction.channel_id)
        if not message.strip():
            sticks.pop(cid, None); save()
            await interaction.response.send_message("Sticky message cleared for this channel.", ephemeral=True); return
        sticks[cid] = {"guild_id": interaction.guild_id, "content": message[:1800], "message_id": None}
        posted = await interaction.channel.send("📌 " + message[:1800], allowed_mentions=discord.AllowedMentions.none())
        sticks[cid]["message_id"] = posted.id; save()
        await interaction.response.send_message("Sticky message set. Use /sticky without a message to clear it.", ephemeral=True)

    @bot.tree.command(name="backup", description="Export this server's roles and channel layout (Premium).")
    @app_commands.checks.has_permissions(administrator=True)
    async def premium_backup(interaction: discord.Interaction):
        if not await premium_gate(interaction): return
        guild = interaction.guild
        payload = {"guild_id": str(guild.id), "guild_name": guild.name, "created_at": discord.utils.utcnow().isoformat(),
          "roles": [{"name": r.name, "permissions": r.permissions.value, "color": r.color.value, "hoist": r.hoist, "mentionable": r.mentionable} for r in guild.roles if not r.is_default()],
          "categories": [{"name": ch.name, "position": ch.position} for ch in guild.categories],
          "channels": [{"name": ch.name, "type": str(ch.type), "position": ch.position, "category": ch.category.name if ch.category else None, "topic": getattr(ch, "topic", None)} for ch in guild.channels if isinstance(ch, (discord.TextChannel, discord.VoiceChannel, discord.StageChannel, discord.ForumChannel))]}
        import io
        raw = json.dumps(payload, indent=2, ensure_ascii=False).encode("utf-8")
        await interaction.response.send_message("Server configuration backup (JSON). This exports structure only; it does not automatically restore roles, permissions, or messages.", file=discord.File(io.BytesIO(raw), filename=f"mallow-backup-{guild.id}.json"), ephemeral=True)

    @bot.tree.command(name="custom-command", description="Create, list, or remove a custom text command (Premium).")
    @app_commands.describe(action="Create, list, or delete", name="Command name", response="Text the bot should reply with")
    @app_commands.choices(action=[app_commands.Choice(name="Create", value="create"), app_commands.Choice(name="List", value="list"), app_commands.Choice(name="Delete", value="delete")])
    @app_commands.checks.has_permissions(manage_guild=True)
    async def premium_custom_command(interaction: discord.Interaction, action: app_commands.Choice[str], name: str = "", response: str = ""):
        if not await premium_gate(interaction): return
        commands_by_guild = data.setdefault("custom_commands", {}).setdefault(str(interaction.guild_id), {})
        key_name = re.sub(r"[^a-z0-9_-]", "", name.casefold())[:32]
        if action.value == "list":
            await interaction.response.send_message("Custom commands: " + (", ".join(key for key in commands_by_guild) or "none"), ephemeral=True); return
        if not key_name:
            await interaction.response.send_message("Provide a command name.", ephemeral=True); return
        if action.value == "delete":
            commands_by_guild.pop(key_name, None); save()
            await interaction.response.send_message(f"Deleted custom command {key_name} (if it existed).", ephemeral=True); return
        if not response.strip():
            await interaction.response.send_message("Provide a response for the custom command.", ephemeral=True); return
        if len(commands_by_guild) >= 25 and key_name not in commands_by_guild:
            await interaction.response.send_message("This server has reached the 25 custom-command limit.", ephemeral=True); return
        commands_by_guild[key_name] = response[:1500]; save()
        await interaction.response.send_message(f"Created {key_name}. Members can use it with the server prefix followed by the name.", ephemeral=True)

    @bot.tree.command(name="vanity", description="Set or view your server's custom tagline (Premium).")
    @app_commands.describe(tagline="New tagline; leave blank to view current tagline")
    @app_commands.checks.has_permissions(manage_guild=True)
    async def premium_vanity(interaction: discord.Interaction, tagline: str = ""):
        if not await premium_gate(interaction): return
        vanity = data.setdefault("vanity", {})
        gid = str(interaction.guild_id)
        if tagline.strip():
            vanity[gid] = tagline.strip()[:160]; save()
        current = vanity.get(gid, "")
        await interaction.response.send_message(f"{interaction.guild.name} tagline: {current or 'No tagline set.'}", ephemeral=not bool(tagline.strip()))

    @bot.tree.command(name="premium-status", description="Check your Mallow Premium status.")
    async def premium_status(interaction: discord.Interaction):
        if interaction.user.id == OWNER_USER_ID:
            await interaction.response.send_message("Mallow Premium: developer access is enabled.", ephemeral=True); return
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=5)) as session:
                async with session.get("https://mallowbot.kz.ci/api/premium-check", params={"discord_id": str(interaction.user.id)}) as response:
                    result = await response.json()
            if response.status == 200 and result.get("active"):
                await interaction.response.send_message(f"Mallow Premium is active. Expires: {result.get('expires_at') or 'unknown'}", ephemeral=True)
            else:
                await interaction.response.send_message("Mallow Premium is not active for your Discord account. Get Premium at https://mallowbot.kz.ci/subscriptions.html", ephemeral=True)
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError):
            await interaction.response.send_message("I couldn't check Premium right now. Please try again shortly.", ephemeral=True)

    async def deleted_message_listener(message: discord.Message):
        if message.guild and not message.author.bot:
            data.setdefault("snipes", {})[str(message.channel.id)] = {"content": message.content, "author": str(message.author), "deleted_at": discord.utils.utcnow().isoformat()}
    async def sticky_message_listener(message: discord.Message):
        if message.guild is None or message.author.bot: return
        item = data.setdefault("sticky_messages", {}).get(str(message.channel.id))
        if not item or item.get("message_id") == message.id: return
        try:
            old_id = item.get("message_id")
            if old_id:
                old = await message.channel.fetch_message(int(old_id))
                await old.delete()
            posted = await message.channel.send("📌 " + str(item.get("content", ""))[:1800], allowed_mentions=discord.AllowedMentions.none())
            item["message_id"] = posted.id; save()
        except (discord.NotFound, discord.Forbidden, discord.HTTPException, ValueError):
            pass

    async def message_listener(message):
        await process_message(bot, message)
    async def prefix_listener(message):
        if message.author.bot or message.guild is None or await is_bot_access_banned(message.author.id):
            return
        cfg = await get_guild_config(message.guild.id)
        command_prefix = cfg.get("prefix", "-mb")
        if not command_prefix or not message.content.casefold().startswith(command_prefix.casefold()):
            return
        raw=message.content[len(command_prefix):].strip()
        if not raw: return
        parts=raw.split(maxsplit=1)
        command_name=parts[0].lower()
        if command_name in set(cfg.get("disabled_commands", [])):
            await message.reply("The " + command_prefix + command_name + " command is disabled in this server.", mention_author=False)
            return

        argument = parts[1] if len(parts) > 1 else ""
        prefix_feature = (
            "moderation" if command_name in {"purge", "dm"}
            else "utility" if command_name in {"ticket", "ticket-panel", "ticket-close"}
            else None
        )
        if prefix_feature and not cfg.get("features", {}).get(prefix_feature, True):
            await message.reply(f"The {command_name} feature is disabled in this server.", mention_author=False)
            return
        if command_name == "swears":
            raw_count = int(data.setdefault("swears", {}).get(key(message.guild.id, message.author.id), 0))
            adjustment = int(cfg.get("swear_count_adjustments", {}).get(str(message.author.id), 0))
            count = max(0, raw_count + adjustment)
            await message.reply(f"You have {count} swear count(s) in this server.", mention_author=False)
            return

        if command_name == "swear-leaderboard":
            entries = []
            adjustments = cfg.get("swear_count_adjustments", {})
            seen_users = set()
            for entry_key, raw_count in data.setdefault("swears", {}).items():
                try:
                    guild_id, user_id = entry_key.split(":", 1)
                    if guild_id == str(message.guild.id):
                        count = max(0, int(raw_count) + int(adjustments.get(user_id, 0)))
                        seen_users.add(user_id)
                        if count > 0:
                            member = message.guild.get_member(int(user_id))
                            entries.append((count, member.display_name if member else f"User {user_id}", user_id))
                except (ValueError, TypeError):
                    continue
            for user_id, adjustment in adjustments.items():
                if user_id not in seen_users:
                    try:
                        count = max(0, int(adjustment))
                        if count > 0:
                            member = message.guild.get_member(int(user_id))
                            entries.append((count, member.display_name if member else f"User {user_id}", user_id))
                    except (ValueError, TypeError):
                        continue
            entries.sort(key=lambda item: (-item[0], item[1].casefold()))
            if not entries:
                await message.reply("No swears have been counted in this server yet.", mention_author=False)
                return
            lines = [f"**{i}.** {name} — **{count}**" for i, (count, name, _uid) in enumerate(entries[:10], 1)]
            await message.reply("**Swear leaderboard**\n" + "\n".join(lines), mention_author=False, allowed_mentions=discord.AllowedMentions.none())
            return

        # Prefix equivalents for the newer moderation and ticket slash commands.
        if command_name == "purge":
            if not message.author.guild_permissions.manage_messages:
                await message.reply("You need Manage Messages permission to use this command.", mention_author=False)
                return
            if not message.guild.me or not message.guild.me.guild_permissions.manage_messages:
                await message.reply("Mallow needs Manage Messages permission in this channel.", mention_author=False)
                return
            try:
                amount = int(argument.strip())
                if not 1 <= amount <= 100:
                    raise ValueError
            except ValueError:
                await message.reply("Usage: -mb purge <1-100>", mention_author=False)
                return
            try:
                deleted = await message.channel.purge(
                    limit=amount + 1,
                    reason=f"-mb purge used by {message.author} ({message.author.id})",
                )
                # Exclude the command invocation from the reported deletion count.
                deleted_count = sum(1 for item in deleted if item.id != message.id)
                await message.channel.send(f"Deleted {deleted_count} message(s).")
            except discord.Forbidden:
                await message.reply("I couldn't delete messages. Check my Manage Messages permission.", mention_author=False)
            except discord.HTTPException:
                await message.reply("Discord couldn't delete those messages. Try again in a moment.", mention_author=False)
            return

        if command_name == "ticket":
            guild = message.guild
            ticket_cfg = cfg.get("tickets_config", {})
            if not ticket_cfg.get("enabled", False):
                await message.reply("Tickets are disabled in this server. Ask a server manager to enable them in the Mallow dashboard.", mention_author=False)
                return
            existing = discord.utils.find(
                lambda ch: isinstance(ch, discord.TextChannel) and ch.topic == f"mallow-ticket:{message.author.id}:{guild.id}",
                guild.text_channels,
            )
            if existing:
                await message.reply(f"You already have a ticket open: {existing.mention}", mention_author=False)
                return
            category = guild.get_channel(int(ticket_cfg.get("category_id") or 0))
            if not isinstance(category, discord.CategoryChannel):
                category = None
            role_ids = {int(x) for x in ticket_cfg.get("support_role_ids", []) if str(x).isdigit()}
            overwrites = {
                guild.default_role: discord.PermissionOverwrite(view_channel=False),
                message.author: discord.PermissionOverwrite(view_channel=True, send_messages=True, read_message_history=True, attach_files=True, embed_links=True),
            }
            for role_id in role_ids:
                role = guild.get_role(role_id)
                if role:
                    overwrites[role] = discord.PermissionOverwrite(view_channel=True, send_messages=True, read_message_history=True)
            try:
                channel = await guild.create_text_channel(
                    f"ticket-{message.author.name}".lower().replace(" ", "-")[:90],
                    category=category,
                    overwrites=overwrites,
                    topic=f"mallow-ticket:{message.author.id}:{guild.id}",
                    reason=f"Support ticket opened by {message.author}",
                )
                welcome = str(ticket_cfg.get("welcome_message") or "Thanks for opening a ticket. A staff member will be with you soon.")[:1800]
                mentions = " ".join(f"<@&{rid}>" for rid in role_ids if guild.get_role(rid))
                await channel.send(
                    content=(mentions + "\n" if mentions else "") + f"{message.author.mention}\n{welcome}",
                    allowed_mentions=discord.AllowedMentions(users=True, roles=True, everyone=False),
                )
                await message.reply(f"Your ticket is ready: {channel.mention}", mention_author=False)
            except (discord.Forbidden, discord.HTTPException):
                await message.reply("I couldn't create the ticket. Check that Mallow has Manage Channels and View Channel permissions.", mention_author=False)
            return

        if command_name == "ticket-panel":
            if not message.author.guild_permissions.manage_guild:
                await message.reply("You need Manage Server permission to post a ticket panel.", mention_author=False)
                return
            if not cfg.get("tickets_config", {}).get("enabled", False):
                await message.reply("Enable tickets in the Mallow dashboard first.", mention_author=False)
                return
            embed = discord.Embed(title="Support tickets", description="Need help? Click the button below to open a private ticket.", color=discord.Color.blurple())
            await message.channel.send(embed=embed, view=TicketPanelView())
            await message.reply("Ticket panel posted.", mention_author=False)
            return

        if command_name == "ticket-close":
            channel = message.channel
            if not isinstance(channel, discord.TextChannel) or not channel.topic or not channel.topic.startswith("mallow-ticket:"):
                await message.reply("This command only works inside a Mallow ticket channel.", mention_author=False)
                return
            ticket_cfg = cfg.get("tickets_config", {})
            allowed = {int(x) for x in ticket_cfg.get("support_role_ids", []) if str(x).isdigit()}
            is_support = message.author.guild_permissions.manage_channels or any(role.id in allowed for role in message.author.roles)
            owner_text = channel.topic.split(":", 2)[1] if len(channel.topic.split(":")) > 1 else ""
            if not is_support and str(message.author.id) != owner_text:
                await message.reply("Only the ticket creator or support staff can close this ticket.", mention_author=False)
                return
            await message.reply("Closing this ticket in 5 seconds…", mention_author=False)
            await asyncio.sleep(5)
            try:
                await channel.delete(reason=f"Ticket closed by {message.author}")
            except (discord.Forbidden, discord.HTTPException):
                pass
            return

        await prefix(bot,message,command_name,argument)
    @bot.tree.command(name="dm", description="Send a direct message to a server member.")
    @app_commands.checks.has_permissions(manage_messages=True)
    @app_commands.describe(member="Member to message", message="Message to send")
    async def dm_slash(interaction: discord.Interaction, member: discord.Member, message: str):
        if len(message) > 1900:
            await interaction.response.send_message("Messages must be 1,900 characters or fewer.", ephemeral=True)
            return
        if interaction.guild is None or interaction.guild_id is None:
            await interaction.response.send_message("Use this command inside a server.", ephemeral=True)
            return
        # Require this server's configured DM/mod log channel so private
        # replies are never accidentally posted into a public command channel.
        configured_channel_id = data.get("log_channels", {}).get(str(interaction.guild_id))
        destination = interaction.guild.get_channel(int(configured_channel_id)) if configured_channel_id else None
        if not isinstance(destination, discord.TextChannel):
            await interaction.response.send_message(
                "This server doesn't have an accessible DM log/mod channel configured. Ask the bot owner to set one with /set-log first.",
                ephemeral=True,
            )
            return
        try:
            await member.send(message, allowed_mentions=discord.AllowedMentions.none())
        except (discord.Forbidden, discord.HTTPException):
            await interaction.response.send_message("I couldn't DM that member. They may have DMs disabled.", ephemeral=True)
            return
        data.setdefault("dm_routes", {})[str(member.id)] = {
            "guild_id": interaction.guild_id,
            "channel_id": destination.id,
            "set_by": interaction.user.id,
            "updated_at": int(time.time()),
        }
        save()
        await interaction.response.send_message(
            f"Sent a DM to {member.mention}. Their replies will be forwarded only to {destination.mention}.",
            ephemeral=True,
        )

    @bot.tree.command(name="youtube", description="Look up a YouTube video link.")
    @app_commands.describe(url="YouTube video URL")
    async def youtube_slash(interaction: discord.Interaction, url: str):
        match = YT_RE.search(url)
        if not match:
            await interaction.response.send_message("Please provide a supported YouTube video URL.", ephemeral=True)
            return
        video_url = "https://www.youtube.com/watch?v=" + match.group(1)
        try:
            async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=8)) as session:
                async with session.get("https://www.youtube.com/oembed", params={"url": video_url, "format": "json"}) as response:
                    if response.status != 200:
                        await interaction.response.send_message("YouTube didn't return video information for that link.", ephemeral=True)
                        return
                    info = await response.json()
            embed = discord.Embed(title=str(info.get("title", "YouTube video"))[:256], url=video_url, color=discord.Color.red())
            embed.add_field(name="Channel", value=str(info.get("author_name", "Unknown"))[:256])
            await interaction.response.send_message(embed=embed)
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError, discord.HTTPException):
            await interaction.response.send_message("I couldn't look up that video right now.", ephemeral=True)

    def render_member_message(template: str, member: discord.Member, *, leaving: bool = False) -> str:
        display_name = discord.utils.escape_markdown(member.display_name)
        user_value = display_name if leaving else member.mention
        member_count = max(0, (member.guild.member_count or 1) - (1 if leaving else 0))
        return (
            str(template)
            .replace("{user}", user_value)
            .replace("{server}", discord.utils.escape_markdown(member.guild.name))
            .replace("{member_count}", str(member_count))
        )[:1900]

    async def fetch_member_image(session: aiohttp.ClientSession, image_url: str, *, max_bytes: int = 5 * 1024 * 1024) -> bytes | None:
        import logging
        logger = logging.getLogger("mallow-member-messages")
        image_url = str(image_url or "").strip()
        if not image_url.startswith(("https://", "http://")):
            logger.warning("Member-card image URL is missing or is not HTTP(S).")
            return None
        try:
            async with session.get(image_url, allow_redirects=True) as response:
                if response.status != 200:
                    logger.warning("Member-card image request returned HTTP %s for host %s.", response.status, response.url.host)
                    return None
                length = int(response.headers.get("Content-Length", "0") or 0)
                if length > max_bytes:
                    logger.warning("Member-card image from host %s is too large (%s bytes).", response.url.host, length)
                    return None
                # Stream the entire response in bounded chunks. StreamReader.read(n) may
                # return fewer than n bytes even when the response has not finished;
                # validating that partial first chunk makes valid PNGs look corrupt.
                chunks = []
                total_bytes = 0
                async for chunk in response.content.iter_chunked(64 * 1024):
                    total_bytes += len(chunk)
                    if total_bytes > max_bytes:
                        logger.warning("Member-card image from host %s exceeded the %s-byte limit while downloading.", response.url.host, max_bytes)
                        return None
                    chunks.append(chunk)
                payload = b"".join(chunks)
                if not payload:
                    logger.warning("Member-card image from host %s was empty.", response.url.host)
                    return None
                # Some image hosts return application/octet-stream despite serving a valid image.
                # Pillow validates the actual image format before it is used for rendering.
                try:
                    with Image.open(io.BytesIO(payload)) as image:
                        image.verify()
                except (OSError, ValueError, TypeError):
                    content_type = response.headers.get("Content-Type", "").split(";", 1)[0].lower()
                    logger.warning("Member-card response from host %s was not a readable image (Content-Type: %s).", response.url.host, content_type or "missing")
                    return None
                return payload
        except asyncio.TimeoutError:
            logger.warning("Timed out downloading a member-card image from host %s.", __import__("urllib.parse", fromlist=["urlparse"]).urlparse(image_url).hostname or "unknown")
            return None
        except (aiohttp.ClientError, ValueError) as exc:
            logger.warning("Could not download a member-card image: %s.", type(exc).__name__)
            return None

    def draw_member_card(background_bytes: bytes, avatar_bytes: bytes, username: str, member_number: int, *, leaving: bool) -> bytes:
        with Image.open(io.BytesIO(background_bytes)) as source:
            background = ImageOps.fit(source.convert("RGB"), (700, 400), method=Image.Resampling.LANCZOS)
        avatar = Image.open(io.BytesIO(avatar_bytes)).convert("RGBA")
        avatar = ImageOps.fit(avatar, (112, 112), method=Image.Resampling.LANCZOS)
        mask = Image.new("L", (112, 112), 0)
        ImageDraw.Draw(mask).ellipse((0, 0, 111, 111), fill=255)
        avatar.putalpha(mask)

        # A dark translucent footer keeps the username and member count legible
        # without hiding the selected background image.
        overlay = Image.new("RGBA", background.size, (0, 0, 0, 0))
        draw = ImageDraw.Draw(overlay)
        draw.rounded_rectangle((22, 248, 678, 378), radius=24, fill=(13, 12, 20, 220), outline=(255, 255, 255, 70), width=2)
        background = Image.alpha_composite(background.convert("RGBA"), overlay)
        background.alpha_composite(avatar, (42, 257))
        draw = ImageDraw.Draw(background)
        font_path_candidates = (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        )
        bold_path_candidates = (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
        )
        def load_font(paths, size):
            for path in paths:
                try:
                    return ImageFont.truetype(path, size=size)
                except OSError:
                    continue
            return ImageFont.load_default()
        name_font = load_font(bold_path_candidates, 28)
        detail_font = load_font(font_path_candidates, 18)
        safe_name = str(username or "Member").replace("\n", " ").strip()[:80] or "Member"
        max_name_width = 490
        while safe_name and draw.textbbox((0, 0), safe_name, font=name_font)[2] > max_name_width:
            safe_name = safe_name[:-2].rstrip() + "…" if len(safe_name) > 2 else safe_name[:-1]
        draw.text((174, 272), safe_name, font=name_font, fill=(255, 255, 255, 255), stroke_width=1, stroke_fill=(0, 0, 0, 100))
        detail = f"This user {'was' if leaving else 'is'} #{max(1, int(member_number))} on the server"
        draw.text((174, 319), detail, font=detail_font, fill=(224, 218, 242, 255))
        output = io.BytesIO()
        background.convert("RGB").save(output, format="PNG", optimize=True)
        return output.getvalue()

    async def build_member_card(member: discord.Member, image_url: str, *, leaving: bool) -> discord.File | None:
        avatar_url = member.display_avatar.replace(size=256, format="png").url
        timeout = aiohttp.ClientTimeout(total=8)
        async with aiohttp.ClientSession(timeout=timeout) as session:
            avatar_bytes = await fetch_member_image(session, avatar_url, max_bytes=2 * 1024 * 1024)
            background_bytes = await fetch_member_image(session, image_url)
        if not avatar_bytes:
            import logging
            logging.getLogger("mallow-member-messages").warning(
                "Could not download/read avatar image for guild %s member %s.",
                member.guild.id, member.id,
            )
            return None
        if not background_bytes:
            fallback = "https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png"
            async with aiohttp.ClientSession(timeout=timeout) as session:
                background_bytes = await fetch_member_image(session, fallback)
        if not background_bytes:
            import logging
            logging.getLogger("mallow-member-messages").warning(
                "No readable welcome-card background was available for guild %s member %s.",
                member.guild.id, member.id,
            )
            return None
        member_number = (member.guild.member_count or 1) + (1 if leaving else 0)
        try:
            card_bytes = await asyncio.to_thread(
                draw_member_card, background_bytes, avatar_bytes, member.display_name, member_number, leaving=leaving
            )
            return discord.File(io.BytesIO(card_bytes), filename="mallow-member-card.png")
        except (OSError, ValueError, TypeError):
            import logging
            logging.getLogger("mallow-member-messages").exception(
                "Image composition failed for guild %s member %s.",
                member.guild.id, member.id,
            )
            return None

    def member_message_embed(title: str, image_url: str, color: discord.Color) -> discord.Embed | None:
        title = str(title or "").strip()
        image_url = str(image_url or "").strip()
        if not title and not image_url:
            return None
        embed = discord.Embed(title=title[:256] or None, color=color)
        if image_url.startswith(("https://", "http://")):
            embed.set_image(url=image_url[:1000])
        return embed

    async def member_join_listener(member: discord.Member):
        import logging
        logger = logging.getLogger("mallow-member-messages")
        try:
            if await is_bot_access_banned(member.id):
                return
            cfg = await get_guild_config(member.guild.id)
            mc = cfg.get("messages_config", {})
            if not mc.get("welcome_enabled") or not mc.get("welcome_channel_id"):
                return
            channel_id = int(mc["welcome_channel_id"])
            channel = member.guild.get_channel(channel_id)
            if channel is None:
                channel = await member.guild.fetch_channel(channel_id)
            if not isinstance(channel, discord.TextChannel):
                logger.warning("Welcome channel %s for guild %s is not a text channel.", channel_id, member.guild.id)
                return
            image_url = mc.get("welcome_image_url") or "https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png"
            welcomer_section = (
                "╭─〔 👤 **Welcomer** 〕─╮\n"
                f"│ WELCOME {member.mention} TO {discord.utils.escape_markdown(member.guild.name)}!!\n"
                f"│ we now have {member.guild.member_count or 1} members!!!\n"
                "╰──────────────────────────╯"
            )
            embed = discord.Embed(
                title=str(mc.get("welcome_title") or "").strip()[:256] or None,
                description=welcomer_section[:4096],
                color=discord.Color.green(),
            )
            card = await build_member_card(member, image_url, leaving=False)
            if card:
                embed.set_image(url="attachment://mallow-member-card.png")
                await channel.send(embed=embed, file=card, allowed_mentions=discord.AllowedMentions(users=True, roles=False, everyone=False))
            else:
                logger.warning("Could not render welcome card for guild %s member %s; sending a non-rendered fallback.", member.guild.id, member.id)
                embed.set_thumbnail(url=member.display_avatar.url)
                embed.set_image(url=image_url)
                embed.set_footer(text=f"{member.display_name} · This user is #{member.guild.member_count or 1} on the server")
                await channel.send(embed=embed, allowed_mentions=discord.AllowedMentions(users=True, roles=False, everyone=False))
        except Exception:
            logger.exception("Welcome message failed for guild %s member %s.", member.guild.id, member.id)

    async def member_remove_listener(member: discord.Member):
        import logging
        logger = logging.getLogger("mallow-member-messages")
        try:
            cfg = await get_guild_config(member.guild.id)
            mc = cfg.get("messages_config", {})
            if not mc.get("goodbye_enabled") or not mc.get("goodbye_channel_id"):
                return
            channel_id = int(mc["goodbye_channel_id"])
            channel = member.guild.get_channel(channel_id)
            if channel is None:
                channel = await member.guild.fetch_channel(channel_id)
            if not isinstance(channel, discord.TextChannel):
                logger.warning("Goodbye channel %s for guild %s is not a text channel.", channel_id, member.guild.id)
                return
            content = render_member_message(mc.get("goodbye_message") or "{user} left {server}.", member, leaving=True)
            image_url = mc.get("goodbye_image_url") or mc.get("welcome_image_url") or "https://raw.githubusercontent.com/WebsiteDevelopmentStudios/Mallow-Bot/main/dashboard/Welcome.png"
            embed = discord.Embed(
                title=str(mc.get("goodbye_title") or "").strip()[:256] or None,
                description=content[:4096] or f"{discord.utils.escape_markdown(member.display_name)} left {discord.utils.escape_markdown(member.guild.name)}.",
                color=discord.Color.orange(),
            )
            card = await build_member_card(member, image_url, leaving=True)
            if card:
                embed.set_image(url="attachment://mallow-member-card.png")
                await channel.send(embed=embed, file=card, allowed_mentions=discord.AllowedMentions.none())
            else:
                logger.warning("Could not render goodbye card for guild %s member %s; sending a non-rendered fallback.", member.guild.id, member.id)
                embed.set_thumbnail(url=member.display_avatar.url)
                embed.set_image(url=image_url)
                embed.set_footer(text=f"{member.display_name} · This user was #{(member.guild.member_count or 1) + 1} on the server")
                await channel.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())
        except Exception:
            logger.exception("Goodbye message failed for guild %s member %s.", member.guild.id, member.id)

    bot.add_listener(deleted_message_listener, "on_message_delete")
    bot.add_listener(sticky_message_listener, "on_message")
    bot.add_listener(message_listener, "on_message")
    bot.add_listener(prefix_listener, "on_message")
    bot.add_listener(member_join_listener, "on_member_join")
    bot.add_listener(auto_role_member_join_listener, "on_member_join")
    bot.add_listener(reaction_role_added, "on_raw_reaction_add")
    bot.add_listener(reaction_role_removed, "on_raw_reaction_remove")
    bot.add_listener(member_remove_listener, "on_member_remove")

    class TicketPanelView(discord.ui.View):
        def __init__(self):
            super().__init__(timeout=None)

        @discord.ui.button(label="Open a ticket", style=discord.ButtonStyle.primary, emoji="🎫", custom_id="mallow:ticket:open")
        async def open_ticket(self, interaction: discord.Interaction, button: discord.ui.Button):
            await create_ticket(interaction)

    class TicketControlsView(discord.ui.View):
        def __init__(self):
            super().__init__(timeout=None)

        @discord.ui.button(label="Close ticket", style=discord.ButtonStyle.danger, emoji="🔒", custom_id="mallow:ticket:close")
        async def close_ticket(self, interaction: discord.Interaction, button: discord.ui.Button):
            channel = interaction.channel
            if not isinstance(channel, discord.TextChannel) or not channel.topic or not channel.topic.startswith("mallow-ticket:"):
                await interaction.response.send_message("This is not a Mallow ticket channel.", ephemeral=True)
                return
            cfg = await get_guild_config(interaction.guild_id) if interaction.guild_id else {}
            allowed = {int(x) for x in cfg.get("tickets_config", {}).get("support_role_ids", []) if str(x).isdigit()}
            is_support = isinstance(interaction.user, discord.Member) and (interaction.user.guild_permissions.manage_channels or any(r.id in allowed for r in interaction.user.roles))
            owner_text = channel.topic.split(":", 2)[1] if len(channel.topic.split(":")) > 1 else ""
            if not is_support and str(interaction.user.id) != owner_text:
                await interaction.response.send_message("Only the ticket creator or support staff can close this ticket.", ephemeral=True)
                return
            await interaction.response.send_message("Closing this ticket in 5 seconds…", ephemeral=True)
            await asyncio.sleep(5)
            try:
                await channel.delete(reason=f"Ticket closed by {interaction.user}")
            except (discord.Forbidden, discord.HTTPException):
                pass

    async def create_ticket(interaction: discord.Interaction):
        if interaction.guild is None:
            await interaction.response.send_message("Tickets can only be opened in a server.", ephemeral=True)
            return
        cfg = await get_guild_config(interaction.guild.id)
        ticket_cfg = cfg.get("tickets_config", {})
        if not ticket_cfg.get("enabled", False):
            await interaction.response.send_message("Tickets are disabled in this server. Ask a server manager to enable them in the Mallow dashboard.", ephemeral=True)
            return
        existing = discord.utils.find(
            lambda ch: isinstance(ch, discord.TextChannel) and ch.topic == f"mallow-ticket:{interaction.user.id}:{interaction.guild.id}",
            interaction.guild.text_channels,
        )
        if existing:
            await interaction.response.send_message(f"You already have a ticket open: {existing.mention}", ephemeral=True)
            return
        category = interaction.guild.get_channel(int(ticket_cfg.get("category_id") or 0))
        if not isinstance(category, discord.CategoryChannel):
            category = None
        role_ids = {int(x) for x in ticket_cfg.get("support_role_ids", []) if str(x).isdigit()}
        overwrites = {
            interaction.guild.default_role: discord.PermissionOverwrite(view_channel=False),
            interaction.user: discord.PermissionOverwrite(view_channel=True, send_messages=True, read_message_history=True, attach_files=True, embed_links=True),
        }
        for role_id in role_ids:
            role = interaction.guild.get_role(role_id)
            if role:
                overwrites[role] = discord.PermissionOverwrite(view_channel=True, send_messages=True, read_message_history=True)
        try:
            channel = await interaction.guild.create_text_channel(
                f"ticket-{interaction.user.name}".lower().replace(" ", "-")[:90],
                category=category,
                overwrites=overwrites,
                topic=f"mallow-ticket:{interaction.user.id}:{interaction.guild.id}",
                reason=f"Support ticket opened by {interaction.user}",
            )
            message = str(ticket_cfg.get("welcome_message") or "Thanks for opening a ticket. A staff member will be with you soon.")[:1800]
            mentions = " ".join(f"<@&{rid}>" for rid in role_ids if interaction.guild.get_role(rid))
            await channel.send(
                content=(mentions + "\n" if mentions else "") + f"{interaction.user.mention}\n{message}",
                view=TicketControlsView(),
                allowed_mentions=discord.AllowedMentions(users=True, roles=True, everyone=False),
            )
            await interaction.response.send_message(f"Your ticket is ready: {channel.mention}", ephemeral=True)
        except (discord.Forbidden, discord.HTTPException):
            if not interaction.response.is_done():
                await interaction.response.send_message("I couldn't create the ticket. Check that Mallow has Manage Channels and View Channel permissions.", ephemeral=True)

    @bot.tree.command(name="ticket", description="Open a private support ticket.")
    async def ticket_slash(interaction: discord.Interaction):
        await create_ticket(interaction)

    @bot.tree.command(name="ticket-panel", description="Post a button members can use to open support tickets.")
    @app_commands.checks.has_permissions(manage_guild=True)
    async def ticket_panel_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True)
            return
        cfg = await get_guild_config(interaction.guild_id)
        if not cfg.get("tickets_config", {}).get("enabled", False):
            await interaction.response.send_message("Enable tickets in the Mallow dashboard first.", ephemeral=True)
            return
        embed = discord.Embed(title="Support tickets", description="Need help? Click the button below to open a private ticket.", color=discord.Color.blurple())
        await interaction.channel.send(embed=embed, view=TicketPanelView())
        await interaction.response.send_message("Ticket panel posted.", ephemeral=True)

    @bot.tree.command(name="ticket-close", description="Close the ticket channel you are in.")
    async def ticket_close_slash(interaction: discord.Interaction):
        channel = interaction.channel
        if not isinstance(channel, discord.TextChannel) or not channel.topic or not channel.topic.startswith("mallow-ticket:"):
            await interaction.response.send_message("This command only works inside a Mallow ticket channel.", ephemeral=True)
            return
        await interaction.response.defer(ephemeral=True)
        await channel.delete(reason=f"Ticket closed by {interaction.user}")
        try:
            await interaction.followup.send("Ticket closed.", ephemeral=True)
        except discord.HTTPException:
            pass

    @bot.tree.command(name="purge", description="Delete 1–100 recent messages from this channel.")
    @app_commands.guild_only()
    @app_commands.checks.has_permissions(manage_messages=True)
    @app_commands.describe(amount="Number of recent messages to delete (1–100)")
    async def purge_slash(interaction: discord.Interaction, amount: app_commands.Range[int, 1, 100]):
        channel = interaction.channel
        if not isinstance(channel, discord.TextChannel):
            await interaction.response.send_message("Use this command in a text channel.", ephemeral=True)
            return
        bot_member = interaction.guild.me if interaction.guild else None
        if bot_member is None or not bot_member.guild_permissions.manage_messages:
            await interaction.response.send_message("Mallow needs the Manage Messages permission in this channel.", ephemeral=True)
            return
        await interaction.response.defer(ephemeral=True)
        try:
            deleted = await channel.purge(
                limit=amount,
                reason=f"/purge used by {interaction.user} ({interaction.user.id})",
            )
            await interaction.followup.send(f"Deleted {len(deleted)} message(s).", ephemeral=True)
        except discord.Forbidden:
            await interaction.followup.send("I couldn't delete messages. Check my Manage Messages permission.", ephemeral=True)
        except discord.HTTPException:
            await interaction.followup.send("Discord couldn't delete those messages. Try again in a moment.", ephemeral=True)

    bot.add_view(TicketPanelView())
    bot.add_view(TicketControlsView())

