"""Signed, retryable sender for MallowCord's Rust event inbox."""
from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import os
import re
import time
import uuid
from typing import Any

import aiohttp


class MallowCordRustEvents:
    def __init__(self) -> None:
        self.base_url = os.getenv("MALLOWCORD_RUST_API_URL", "").strip().rstrip("/")
        if self.base_url.endswith("/api"):
            self.base_url = self.base_url[:-4]
        self.secret = os.getenv("MALLOWCORD_INTERNAL_EVENT_SECRET", "").strip()
        self.session: aiohttp.ClientSession | None = None

    @property
    def enabled(self) -> bool:
        return bool(self.base_url and len(self.secret.encode("utf-8")) >= 32)

    async def send_event(
        self,
        event_type: str,
        payload: dict[str, Any],
        *,
        event_id: str | None = None,
        source: str = "mallowbot",
    ) -> dict[str, Any]:
        if not self.base_url:
            raise RuntimeError("MALLOWCORD_RUST_API_URL is not configured.")
        if len(self.secret.encode("utf-8")) < 32:
            raise RuntimeError("MALLOWCORD_INTERNAL_EVENT_SECRET must contain at least 32 bytes.")
        stable_id = event_id or uuid.uuid4().hex
        if not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", stable_id):
            raise ValueError("Invalid MallowCord event ID.")

        body = json.dumps(
            {"source": source, "event_type": event_type, "payload": payload},
            separators=(",", ":"),
            sort_keys=True,
            ensure_ascii=False,
        ).encode("utf-8")
        timeout = aiohttp.ClientTimeout(total=12, connect=5)
        if self.session is None or self.session.closed:
            self.session = aiohttp.ClientSession(timeout=timeout)
        for attempt in range(3):
            timestamp = str(int(time.time()))
            signed = timestamp.encode("ascii") + b"." + stable_id.encode("ascii") + b"." + body
            signature = hmac.new(
                self.secret.encode("utf-8"), signed, hashlib.sha256
            ).hexdigest()
            headers = {
                "Content-Type": "application/json",
                "X-MallowCord-Timestamp": timestamp,
                "X-MallowCord-Event-Id": stable_id,
                "X-MallowCord-Signature": f"sha256={signature}",
            }
            try:
                async with self.session.post(
                    f"{self.base_url}/internal/v1/events",
                    data=body,
                    headers=headers,
                ) as response:
                    data = await response.json(content_type=None)
                    if response.status < 500 and response.status != 429:
                        if response.status >= 400:
                            detail = data.get("error") if isinstance(data, dict) else None
                            raise RuntimeError(detail or f"MallowCord returned HTTP {response.status}.")
                        return data if isinstance(data, dict) else {"accepted": True}
                    if attempt == 2:
                        raise RuntimeError(f"MallowCord event API returned HTTP {response.status}.")
            except (aiohttp.ClientError, TimeoutError):
                if attempt == 2:
                    raise
            await asyncio.sleep(0.4 * (attempt + 1))
        raise RuntimeError("MallowCord event delivery failed after retries.")

    async def close(self) -> None:
        if self.session and not self.session.closed:
            await self.session.close()
        self.session = None
