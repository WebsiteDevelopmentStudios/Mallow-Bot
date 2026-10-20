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
    "https://mallowbot.kz.ci/api/runtime-config",
).strip()
DASHBOARD_CONFIG_SECRET = os.getenv("MALLOW_DASHBOARD_CONFIG_SECRET", "").strip()
_CACHE: dict[str, tuple[float, dict[str, Any]]] = {}
_CACHE_TTL = 3.0
_LOCKS: dict[str, asyncio.Lock] = {}


def default_config() -> dict[str, Any]:
    return {
        "prefix": "-mb",
        "greeting": "hey! i'm Mallow Bot ☁️",
        "greetings": True,
        "mention_only": True,
        "disabled_commands": [],
        "features": {"economy": True, "xp": True, "moderation": True, "music": True, "fun": True, "movies": True, "utility": True, "social": True, "ai": False},
        "economy_config": {"starting_money": 100, "daily_reward": 100, "work_min": 20, "work_max": 80},
        "xp_config": {"min_xp": 10, "max_xp": 20, "cooldown_seconds": 60, "announce_level_up": True},
        "messages_config": {"welcome_enabled": False, "welcome_channel_id": "", "welcome_message": "Welcome {user} to {server}!", "goodbye_enabled": False, "goodbye_channel_id": "", "goodbye_message": "{user} left {server}.", "auto_reply_enabled": False, "auto_replies": []},
        "tickets_config": {"enabled": False, "category_id": "", "support_role_ids": [], "transcript_channel_id": "", "welcome_message": "Thanks for opening a ticket. A staff member will be with you soon."},
    }


async def get_guild_config(guild_id: int | str) -> dict[str, Any]:
    """Fetch settings for one server, caching briefly to avoid per-message API traffic."""
    key = str(guild_id)
    now = time.monotonic()
    cached = _CACHE.get(key)
    if cached and now - cached[0] < _CACHE_TTL:
        return cached[1]
    if not DASHBOARD_CONFIG_SECRET or not DASHBOARD_CONFIG_URL:
        import logging
        logging.getLogger("mallow-config").error(
            "Dashboard config bridge is not configured (URL set=%s, secret set=%s).",
            bool(DASHBOARD_CONFIG_URL), bool(DASHBOARD_CONFIG_SECRET),
        )
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
                        import logging
                        logging.getLogger("mallow-config").error(
                            "Dashboard config request failed for guild %s with HTTP %s.",
                            key, response.status,
                        )
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
            features = settings.get("features", {})
            if isinstance(features, dict):
                config["features"] = {**config["features"], **{k: bool(v) for k, v in features.items() if k in config["features"]}}
            for field, limits in (("economy_config", {"starting_money": (0,100000000), "daily_reward": (0,1000000), "work_min": (0,1000000), "work_max": (0,1000000)}), ("xp_config", {"min_xp": (0,10000), "max_xp": (0,10000), "cooldown_seconds": (0,86400)})):
                values = settings.get(field, {})
                if isinstance(values, dict):
                    for name, (minimum, maximum) in limits.items():
                        try:
                            config[field][name] = max(minimum, min(maximum, int(values.get(name, config[field][name]))))
                        except (ValueError, TypeError):
                            pass
            if isinstance(settings.get("xp_config"), dict):
                config["xp_config"]["announce_level_up"] = settings["xp_config"].get("announce_level_up", True) is not False
            for field in ("messages_config", "tickets_config"):
                values = settings.get(field, {})
                if isinstance(values, dict):
                    config[field].update(values)
            _CACHE[key] = (time.monotonic(), config)
            return config
        except (aiohttp.ClientError, asyncio.TimeoutError, ValueError, TypeError) as exc:
            import logging
            logging.getLogger("mallow-config").error(
                "Dashboard config request failed for guild %s (%s).",
                key, type(exc).__name__,
            )
            return cached[1] if cached else default_config()


def invalidate_guild_config(guild_id: int | str) -> None:
    _CACHE.pop(str(guild_id), None)
