import asyncio
import json
import os
import random
import re
import time
from pathlib import Path
from datetime import timedelta

import aiohttp
import discord
from discord import app_commands

from dashboard_config import get_guild_config

OWNER_USER_ID = 1021068313071210498
DATA_FILE = Path(__file__).resolve().parent / "community_data.json"
INVITE_RE = re.compile(r"(?:https?://)?(?:www\.)?(?:discord\.gg|discord(?:app)?\.com/invite)/[A-Za-z0-9-]+", re.I)
YT_RE = re.compile(r"https?://(?:www\.)?(?:youtube\.com/watch\?[^\s]*v=|youtu\.be/|youtube\.com/shorts/)([A-Za-z0-9_-]{11})", re.I)

def load():
    try:
        value = json.loads(DATA_FILE.read_text(encoding="utf-8"))
        if isinstance(value, dict):
            for key in ("xp", "currency", "log_channels", "timeout_minutes", "last_xp", "daily_claims"):
                value.setdefault(key, {})
            return value
    except (OSError, json.JSONDecodeError):
        pass
    return {key: {} for key in ("xp", "currency", "log_channels", "timeout_minutes", "last_xp", "daily_claims")}

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
    if message.author.bot:
        return
    if message.guild is None:
        # DM inbox: send incoming messages to a configured log channel.
        if bot.user and message.author.id == bot.user.id:
            return
        for channel_id in data["log_channels"].values():
            try:
                channel = bot.get_channel(int(channel_id)) or await bot.fetch_channel(int(channel_id))
                if isinstance(channel, discord.TextChannel):
                    embed = discord.Embed(title="Mallow-Bot DM received", color=discord.Color.orange())
                    embed.add_field(name="From", value=f"{message.author} (ID {message.author.id})", inline=False)
                    embed.add_field(name="Message", value=(message.content or "[No text content]")[:4000], inline=False)
                    embed.timestamp = message.created_at
                    await channel.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())
                    break
            except (discord.Forbidden, discord.NotFound, discord.HTTPException, ValueError):
                continue
        try:
            await message.channel.send("Your message was delivered to the bot owner.")
        except discord.HTTPException:
            pass
        return

    guild = message.guild
    content = message.content or ""
    dashboard_config = await get_guild_config(guild.id)
    command_prefix = dashboard_config.get("prefix", "-mb")
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
    yt = None if content.casefold().startswith((command_prefix + " youtube").casefold()) else YT_RE.search(content)
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
        "utility": {"youtube"},
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

    async def message_listener(message):
        await process_message(bot, message)
    async def prefix_listener(message):
        if message.author.bot or message.guild is None:
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
        await prefix(bot,message,command_name,parts[1] if len(parts)>1 else "")
    @bot.tree.command(name="dm", description="Send a direct message to a server member.")
    @app_commands.checks.has_permissions(manage_messages=True)
    @app_commands.describe(member="Member to message", message="Message to send")
    async def dm_slash(interaction: discord.Interaction, member: discord.Member, message: str):
        if len(message) > 1900:
            await interaction.response.send_message("Messages must be 1,900 characters or fewer.", ephemeral=True)
            return
        try:
            await member.send(message, allowed_mentions=discord.AllowedMentions.none())
        except (discord.Forbidden, discord.HTTPException):
            await interaction.response.send_message("I couldn't DM that member. They may have DMs disabled.", ephemeral=True)
            return
        await interaction.response.send_message(f"Sent a DM to {member.mention}. Replies will be forwarded to the configured DM log channel.", ephemeral=True)

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

    bot.add_listener(message_listener, "on_message")
    bot.add_listener(prefix_listener, "on_message")
