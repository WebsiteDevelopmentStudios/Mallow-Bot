"""Per-guild settings bridge to the Mallow Bot dashboard.

Set MALLOW_DASHBOARD_CONFIG_SECRET on the bot host and the matching
RUNTIME_CONFIG_SECRET secret in Cloudflare Pages. Without both, defaults apply.
"""
import asyncio
import os
import time
from typing import Any

import aiohttp

DASHBOARD_CONFIG_URL = os.getenv(
    "MALLOW_DASHBOARD_CONFIG_URL",
    "https://mallowbot.devs.surf/api/runtime-config",
).strip()
DASHBOARD_CONFIG_SECRET = os.getenv("MALLOW_DASHBOARD_CONFIG_SECRET", "").strip()
_CACHE: dict[str, tuple[float, dict[str, Any]]] = {}
_CACHE_TTL = 20.0
_LOCKS: dict[str, asyncio.Lock] = {}


def default_config() -> dict[str, Any]:
    return {
        "prefix": "-mb",
        "greeting": "hey! i'm Mallow Bot ☁️",
        "greetings": True,
        "mention_only": True,
        "disabled_commands": [],
    }


async def get_guild_config(guild_id: int | str) -> dict[str, Any]:
    """Fetch settings for one server, caching briefly to avoid per-message API traffic."""
    key = str(guild_id)
    now = time.monotonic()
    cached = _CACHE.get(key)
    if cached and now - cached[0] < _CACHE_TTL:
        return cached[1]
    if not DASHBOARD_CONFIG_SECRET or not DASHBOARD_CONFIG_URL:
        return default_config()
    lock = _LOCKS.setdefault(key, asyncio.Lock())
    async with lock:
        now = time.monotonic()
        cached = _CACHE.get(key)
        if cached and now - cached[0] < _CACHE_TTL:
            return cached[1]
        try:
            timeout = aiohttp.ClientTimeout(total=4)
            async with aiohttp.ClientSession(timeout=timeout) as session:
                async with session.get(
                    DASHBOARD_CONFIG_URL,
                    params={"guild_id": key},
                    headers={"Authorization": f"Bearer {DASHBOARD_CONFIG_SECRET}"},
                ) as response:
                    if response.status != 200:
                        return cached[1] if cached else default_config()
                    payload = await response.json()
            settings = payload.get("settings", {})
            config = default_config()
            config["prefix"] = str(settings.get("prefix") or "-mb")[:8]
            config["greeting"] = str(settings.get("greeting") or config["greeting"])[:500]
            config["greetings"] = settings.get("greetings") is not False
            config["mention_only"] = settings.get("mention_only") is not False
            disabled = settings.get("disabled_commands", [])
            config["disabled_commands"] = sorted({
                str(item).lower() for item in disabled
                if isinstance(item, str) and item.replace("-", "").replace("_", "").isalnum()
            }) if isinstance(disabled, list) else []
            _CACHE[key] = (time.monotonic(), config)
            return config
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError, TypeError):
            return cached[1] if cached else default_config()


def invalidate_guild_config(guild_id: int | str) -> None:
    _CACHE.pop(str(guild_id), None)
