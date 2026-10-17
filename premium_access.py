"""Shared check for owner-imposed Mallow Bot access bans."""
import asyncio
import logging
import os
import time

import aiohttp

logger = logging.getLogger("mallow-bot.access")
CHECK_URL = os.getenv("PREMIUM_CHECK_URL", "https://mallowbot.kz.ci/api/premium-check").strip()
CACHE_SECONDS = 15
_cache: dict[str, tuple[float, bool]] = {}
_inflight: dict[str, asyncio.Task] = {}


async def is_bot_access_banned(discord_id: int | str) -> bool:
    """Return whether a Discord ID is globally banned; cache checks briefly."""
    user_id = str(discord_id).strip()
    now = time.monotonic()
    cached = _cache.get(user_id)
    if cached and cached[0] > now:
        return cached[1]

    async def fetch_status() -> bool:
        timeout = aiohttp.ClientTimeout(total=4)
        try:
            async with aiohttp.ClientSession(timeout=timeout) as session:
                async with session.get(CHECK_URL, params={"discord_id": user_id}) as response:
                    if response.status != 200:
                        logger.warning("Premium-ban check returned HTTP %s; allowing user %s temporarily.", response.status, user_id)
                        return False
                    data = await response.json(content_type=None)
                    return bool(data.get("banned", False))
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError) as exc:
            logger.warning("Premium-ban check unavailable; allowing user %s temporarily: %s", user_id, exc)
            return False

    task = _inflight.get(user_id)
    if task is None:
        task = asyncio.create_task(fetch_status())
        _inflight[user_id] = task
    try:
        banned = await task
    finally:
        if _inflight.get(user_id) is task:
            _inflight.pop(user_id, None)
    _cache[user_id] = (time.monotonic() + CACHE_SECONDS, banned)
    return banned
