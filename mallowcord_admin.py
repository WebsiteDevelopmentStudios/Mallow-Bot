"""Server-side MallowCord admin API client for Mallow-Bot's owner dashboard."""
from __future__ import annotations

import os
from typing import Any

import aiohttp


class MallowCordAdminClient:
    def __init__(self) -> None:
        self.base_url = os.getenv("MALLOWCORD_API_URL", "").rstrip("/")
        self.service_key = os.getenv("MALLOWCORD_SERVICE_KEY", "")

    def _headers(self) -> dict[str, str]:
        if not self.base_url or not self.service_key:
            raise RuntimeError("MALLOWCORD_API_URL and MALLOWCORD_SERVICE_KEY must be configured.")
        return {"X-MallowBot-Key": self.service_key, "Content-Type": "application/json"}

    async def request(self, path: str, method: str = "GET", payload: dict[str, Any] | None = None) -> dict[str, Any]:
        timeout = aiohttp.ClientTimeout(total=12)
        async with aiohttp.ClientSession(timeout=timeout) as session:
            async with session.request(method, self.base_url + path, headers=self._headers(), json=payload) as response:
                data = await response.json(content_type=None)
                if response.status >= 400:
                    raise RuntimeError(data.get("error", "MallowCord request failed."))
                return data

    async def list_users(self) -> dict[str, Any]:
        return await self.request("/integrations/mallowbot/users")

    async def create_full_user(self, username: str, password: str) -> dict[str, Any]:
        return await self.request("/integrations/mallowbot/users", "POST", {"username": username, "password": password})

    async def moderate(self, user_id: str, action: str, seconds: int = 600) -> dict[str, Any]:
        if action not in {"timeout", "ban", "unban", "delete"}:
            raise ValueError("Unsupported moderation action.")
        payload = {"seconds": seconds} if action == "timeout" else {}
        return await self.request(f"/integrations/mallowbot/users/{user_id}/{action}", "POST", payload)
