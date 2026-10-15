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

OWNER_USER_ID = 1021068313071210498
DATA_FILE = Path(__file__).resolve().parent / "community_data.json"
INVITE_RE = re.compile(r"(?:https?://)?(?:www\.)?(?:discord\.gg|discord(?:app)?\.com/invite)/[A-Za-z0-9-]+", re.I)
YT_RE = re.compile(r"https?://(?:www\.)?(?:youtube\.com/watch\?[^\s]*v=|youtu\.be/|youtube\.com/shorts/)([A-Za-z0-9_-]{11})", re.I)

def load():
    try:
        value = json.loads(DATA_FILE.read_text(encoding="utf-8"))
        if isinstance(value, dict):
            for key in ("xp", "currency", "log_channels", "timeout_minutes", "last_xp"):
                value.setdefault(key, {})
            return value
    except (OSError, json.JSONDecodeError):
        pass
    return {key: {} for key in ("xp", "currency", "log_channels", "timeout_minutes", "last_xp")}

data = load()
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
                    embed = discord.Embed(title="Movie-Bot DM received", color=discord.Color.orange())
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
    if INVITE_RE.search(content) and not message.author.guild_permissions.manage_messages:
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

    if content.strip() and not content.lower().startswith("-mb"):
        k = key(guild.id, message.author.id)
        now = time.time()
        if now - float(data["last_xp"].get(k, 0)) >= 60:
            old = int(data["xp"].get(k, 0))
            new = old + random.randint(10, 20)
            data["xp"][k] = new
            data["last_xp"][k] = now
            save()
            if level(new) > level(old):
                await message.channel.send(f"{message.author.mention} reached level {level(new)}. Try not to let it go to your head.", delete_after=12)

    yt = YT_RE.search(content)
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

    if re.match(r"^\s*movie\s+bot\b", content, re.I):
        reply = None
        system_prompt = (
            "You are Movie Bot, a sarcastic, perpetually irritated movie and music Discord bot. "
            "Answer the user's actual message directly and sensibly. For yes/no questions, lead "
            "with yes or no when you can reasonably tell; if you cannot know, say what information "
            "is missing. Never invent facts. Be comically grumpy, never hateful, threatening, or "
            "discriminatory. Keep replies under 100 words."
        )

        # Groq is the primary AI provider; it uses an OpenAI-compatible chat endpoint.
        groq_key = os.getenv("GROQ_API_KEY", "").strip()
        if groq_key:
            try:
                async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=20)) as session:
                    async with session.post(
                        "https://api.groq.com/openai/v1/chat/completions",
                        headers={"Authorization": f"Bearer {groq_key}", "Content-Type": "application/json"},
                        json={
                            "model": os.getenv("GROQ_MODEL", "openai/gpt-oss-120b"),
                            "messages": [
                                {"role": "system", "content": system_prompt},
                                {"role": "user", "content": content},
                            ],
                            "max_tokens": 1024,
                            "temperature": 0.85,
                        },
                    ) as response:
                        if response.status == 200:
                            result = await response.json()
                            choices = result.get("choices") or []
                            if choices:
                                reply = str(choices[0].get("message", {}).get("content") or "").strip()
                            if not reply:
                                print("[MovieBot AI] Groq returned no text.", flush=True)
                        else:
                            detail = (await response.text())[:1200]
                            print("[MovieBot AI] Groq HTTP " + str(response.status) + " for model " + os.getenv("GROQ_MODEL", "openai/gpt-oss-120b") + ": " + detail, flush=True)
            except (aiohttp.ClientError, asyncio.TimeoutError, KeyError, IndexError, TypeError, ValueError) as exc:
                print("[MovieBot AI] Groq request failed: " + type(exc).__name__ + ": " + str(exc), flush=True)

        # Gemini's free API tier provides actual model-generated replies.
        gemini_key = os.getenv("GEMINI_API_KEY", "").strip()
        if gemini_key:
            try:
                async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=20)) as session:
                    endpoint = (
                        "https://generativelanguage.googleapis.com/v1beta/models/"
                        + os.getenv("GEMINI_MODEL", "gemini-3.8-flash")
                        + ":generateContent"
                    )
                    async with session.post(
                        endpoint,
                        params={"key": gemini_key},
                        json={
                            "system_instruction": {"parts": [{"text": system_prompt}]},
                            "contents": [{"role": "user", "parts": [{"text": content}]}],
                            "generationConfig": {"maxOutputTokens": 180, "temperature": 0.85},
                        },
                    ) as response:
                        if response.status == 200:
                            result = await response.json()
                            candidates = result.get("candidates") or []
                            if candidates:
                                parts = candidates[0].get("content", {}).get("parts", [])
                                reply = "".join(part.get("text", "") for part in parts if isinstance(part, dict)).strip()
                            if not reply:
                                print("[MovieBot AI] Gemini returned no text for model " + os.getenv("GEMINI_MODEL", "gemini-3.8-flash") + ": " + str(result.get("promptFeedback", {})), flush=True)
                        else:
                            detail = (await response.text())[:1200]
                            print("[MovieBot AI] Gemini HTTP " + str(response.status) + " for model " + os.getenv("GEMINI_MODEL", "gemini-3.8-flash") + ": " + detail, flush=True)
            except (aiohttp.ClientError, asyncio.TimeoutError, KeyError, IndexError, TypeError, ValueError) as exc:
                print("[MovieBot AI] Gemini request failed: " + type(exc).__name__ + ": " + str(exc), flush=True)

        # Optional OpenAI-compatible provider fallback, if configured.
        if not reply:
            api_key = os.getenv("OPENAI_API_KEY", "").strip()
            if api_key:
                try:
                    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=15)) as session:
                        async with session.post(
                            os.getenv("OPENAI_BASE_URL", "https://api.openai.com/v1/chat/completions"),
                            headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
                            json={
                                "model": os.getenv("MOVIE_BOT_AI_MODEL", "gpt-4o-mini"),
                                "messages": [
                                    {"role": "system", "content": system_prompt},
                                    {"role": "user", "content": content},
                                ],
                                "max_tokens": 180,
                            },
                        ) as response:
                            if response.status == 200:
                                result = await response.json()
                                reply = result["choices"][0]["message"]["content"].strip()
                except (aiohttp.ClientError, asyncio.TimeoutError, KeyError, IndexError, TypeError, ValueError):
                    pass

        if not reply:
            if not groq_key and not gemini_key and not os.getenv("OPENAI_API_KEY", "").strip():
                reply = "I need an AI API key before I can think for myself. Set GROQ_API_KEY in the bot's environment to use Groq."
            else:
                reply = "My AI connection failed. Check the bot hosting logs for the exact API error, then verify the API key, model, and quota."
        await message.reply(reply[:1900], mention_author=False, allowed_mentions=discord.AllowedMentions.none())

async def prefix(bot, message, command, argument):
    if message.guild is None:
        return False
    gid, uid = message.guild.id, message.author.id
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
        entries = [(int(k.split(":")[1]), int(v)) for k,v in data["xp"].items() if k.startswith(f"{gid}:")]
        entries.sort(key=lambda item: item[1], reverse=True)
        lines = []
        for i, (user_id, xp) in enumerate(entries[:10], 1):
            member = message.guild.get_member(user_id)
            lines.append(f"**{i}.** {member.display_name if member else 'User '+str(user_id)} — {xp} XP (Level {level(xp)})")
        await message.reply("\n".join(lines) if lines else "No XP has been earned yet.", mention_author=False)
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
        balance = int(data["currency"].get(k, 100))
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
            result = f"You won {bet} Movie Coins."
        else:
            balance -= bet
            result = f"You lost {bet} Movie Coins."
        data["currency"][k] = balance
        save()
        await message.reply(f"{result} Balance: {balance} Movie Coins.", mention_author=False)
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
        outcome = f"Won {payout} Movie Coins." if payout else f"Lost {bet} Movie Coins."
        await message.reply(f"{detail} {outcome} Balance: {balance} Movie Coins.", mention_author=False)
        return True
    if command == "work":
        k = key(gid, uid)
        earned = random.randint(20, 80)
        data["currency"][k] = int(data["currency"].get(k, 100)) + earned
        save()
        await message.reply(f"You worked a shift and earned {earned} Movie Coins.", mention_author=False)
        return True
    if command == "rob":
        target = message.mentions[0] if message.mentions else None
        if target is None or target.bot or target.id == uid:
            await message.reply("Usage: -mb rob @member", mention_author=False)
            return True
        target_key = key(gid, target.id)
        target_balance = int(data["currency"].get(target_key, 100))
        if target_balance < 1:
            await message.reply("That member has no Movie Coins to steal.", mention_author=False)
            return True
        if random.random() < 0.35:
            stolen = min(target_balance, random.randint(1, max(1, target_balance // 4)))
            data["currency"][target_key] = target_balance - stolen
            data["currency"][key(gid, uid)] = int(data["currency"].get(key(gid, uid), 100)) + stolen
            result = f"You stole {stolen} Movie Coins from {target.display_name}."
        else:
            fine = min(int(data["currency"].get(key(gid, uid), 100)), 25)
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
            await message.reply(f"You only have {balance} Movie Coins.", mention_author=False)
            return True
        tk = key(gid, target.id)
        data["currency"][k] = balance - amount
        data["currency"][tk] = int(data["currency"].get(tk, 100)) + amount
        save()
        await message.reply(f"Paid {amount} Movie Coins to {target.display_name}.", mention_author=False)
        return True
    if command == "balance":
        await message.reply(f"You have {int(data['currency'].get(key(gid,uid),100))} Movie Coins.", mention_author=False)
        return True
    if command == "daily":
        k = key(gid,uid)
        data["currency"][k] = int(data["currency"].get(k,100)) + 100
        save()
        await message.reply("Claimed 100 Movie Coins. Try not to lose them immediately.", mention_author=False)
        return True
    if command in ("coinflip", "slots"):
        k = key(gid,uid)
        balance = int(data["currency"].get(k,100))
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
            result = f"You won {bet if command == 'coinflip' else bet*2} Movie Coins."
        else:
            balance -= bet
            result = f"You lost {bet} Movie Coins. Excellent financial strategy."
        data["currency"][k] = balance
        save()
        await message.reply(f"{result} Balance: {balance} Movie Coins.", mention_author=False)
        return True
    return False

async def play_game(interaction: discord.Interaction, game: str, bet: int):
    if interaction.guild_id is None:
        await interaction.response.send_message("Use this in a server.", ephemeral=True); return
    k=key(interaction.guild_id,interaction.user.id)
    balance=int(data["currency"].get(k,100))
    if bet>balance:
        await interaction.response.send_message(f"You only have {balance} Movie Coins.",ephemeral=True); return
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
    outcome=f"Won {payout} Movie Coins." if payout else f"Lost {bet} Movie Coins."
    await interaction.response.send_message(f"{detail} {outcome} Balance: {balance} Movie Coins.",ephemeral=True)


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
        entries = [(int(k.split(":")[1]), int(v)) for k,v in data["xp"].items() if k.startswith(f"{gid}:")]
        entries.sort(key=lambda item:item[1], reverse=True)
        lines=[]
        for i,(uid,xp) in enumerate(entries[:10],1):
            member=interaction.guild.get_member(uid)
            lines.append(f"**{i}.** {member.display_name if member else 'User '+str(uid)} — {xp} XP (Level {level(xp)})")
        await interaction.response.send_message("\n".join(lines) if lines else "No XP has been earned yet.")

    @bot.tree.command(name="coinflip", description="Bet virtual Movie Coins on a coin flip.")
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

    @bot.tree.command(name="work", description="Work to earn virtual Movie Coins.")
    async def work_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        k=key(interaction.guild_id, interaction.user.id)
        earned=random.randint(20,80)
        data["currency"][k]=int(data["currency"].get(k,100))+earned
        save()
        await interaction.response.send_message(f"You earned {earned} Movie Coins. Balance: {data['currency'][k]}.",ephemeral=True)

    @bot.tree.command(name="pay", description="Give virtual Movie Coins to a member.")
    @app_commands.describe(member="Member to pay", amount="Amount of virtual coins")
    async def pay_slash(interaction: discord.Interaction, member: discord.Member, amount: app_commands.Range[int,1,1000000]):
        if interaction.guild_id is None or member.bot or member.id == interaction.user.id:
            await interaction.response.send_message("Choose another server member.", ephemeral=True); return
        k=key(interaction.guild_id,interaction.user.id); tk=key(interaction.guild_id,member.id)
        balance=int(data["currency"].get(k,100))
        if amount>balance:
            await interaction.response.send_message(f"You only have {balance} Movie Coins.",ephemeral=True); return
        data["currency"][k]=balance-amount
        data["currency"][tk]=int(data["currency"].get(tk,100))+amount
        save()
        await interaction.response.send_message(f"Paid {amount} Movie Coins to {member.display_name}.",ephemeral=True)

    @bot.tree.command(name="rob", description="Try to steal virtual Movie Coins from a member.")
    @app_commands.describe(member="Member to rob")
    async def rob_slash(interaction: discord.Interaction, member: discord.Member):
        if interaction.guild_id is None or member.bot or member.id == interaction.user.id:
            await interaction.response.send_message("Choose another server member.", ephemeral=True); return
        tk=key(interaction.guild_id,member.id); k=key(interaction.guild_id,interaction.user.id)
        target_balance=int(data["currency"].get(tk,100))
        if target_balance<1:
            await interaction.response.send_message("That member has no Movie Coins.",ephemeral=True); return
        if random.random()<0.35:
            stolen=min(target_balance,random.randint(1,max(1,target_balance//4)))
            data["currency"][tk]=target_balance-stolen
            data["currency"][k]=int(data["currency"].get(k,100))+stolen
            result=f"You stole {stolen} Movie Coins from {member.display_name}."
        else:
            fine=min(int(data["currency"].get(k,100)),25)
            data["currency"][k]=int(data["currency"].get(k,100))-fine
            result=f"You got caught and paid a {fine}-coin fine."
        save()
        await interaction.response.send_message(result,ephemeral=True)

    @bot.tree.command(name="balance", description="Show your virtual Movie Coins.")
    async def balance_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        await interaction.response.send_message(f"You have {int(data['currency'].get(key(interaction.guild_id,interaction.user.id),100))} Movie Coins.", ephemeral=True)

    @bot.tree.command(name="daily", description="Claim 100 virtual Movie Coins.")
    async def daily_slash(interaction: discord.Interaction):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        k=key(interaction.guild_id,interaction.user.id)
        data["currency"][k]=int(data["currency"].get(k,100))+100
        save()
        await interaction.response.send_message("Claimed 100 Movie Coins.", ephemeral=True)

    @bot.tree.command(name="gamble", description="Bet virtual Movie Coins on a coin flip.")
    @app_commands.describe(bet="Amount of virtual currency to wager")
    async def gamble_slash(interaction: discord.Interaction, bet: app_commands.Range[int,1,1000000]):
        if interaction.guild_id is None:
            await interaction.response.send_message("Use this in a server.", ephemeral=True); return
        k=key(interaction.guild_id,interaction.user.id)
        balance=int(data["currency"].get(k,100))
        if bet>balance:
            await interaction.response.send_message(f"You only have {balance} Movie Coins.",ephemeral=True); return
        if random.choice([True,False]):
            balance += bet; result=f"You won {bet} Movie Coins."
        else:
            balance -= bet; result=f"You lost {bet} Movie Coins."
        data["currency"][k]=balance; save()
        await interaction.response.send_message(f"{result} New balance: {balance} Movie Coins.",ephemeral=True)

    async def message_listener(message):
        await process_message(bot, message)
    async def prefix_listener(message):
        if message.author.bot or message.guild is None or not message.content.lower().startswith("-mb"):
            return
        raw=message.content[3:].strip()
        if not raw: return
        parts=raw.split(maxsplit=1)
        await prefix(bot,message,parts[0].lower(),parts[1] if len(parts)>1 else "")
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
