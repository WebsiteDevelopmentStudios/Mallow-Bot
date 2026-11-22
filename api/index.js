import { createPublicKey, verify as verifySignature } from "node:crypto";

export const config = {
  api: { bodyParser: false },
};

const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

async function readRawBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  return Buffer.concat(chunks);
}

function verifyDiscordRequest(body, signature, timestamp) {
  const hexKey = (process.env.DISCORD_PUBLIC_KEY || "").trim();
  if (!hexKey || !signature || !timestamp || !/^[0-9a-f]{64}$/i.test(hexKey)) return false;
  try {
    const rawKey = Buffer.from(hexKey, "hex");
    const rawSignature = Buffer.from(signature, "hex");
    if (rawKey.length !== 32 || rawSignature.length !== 64) return false;
    const publicKey = createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, rawKey]),
      format: "der",
      type: "spki",
    });
    return verifySignature(null, Buffer.concat([Buffer.from(timestamp, "utf8"), body]), publicKey, rawSignature);
  } catch {
    return false;
  }
}

function initialResponse(type) {
  switch (Number(type)) {
    case 1: return { type: 1 };
    case 2: return { type: 5, data: { flags: 64 } };
    case 3: return { type: 6 };
    case 4: return { type: 8, data: { choices: [] } };
    case 5: return { type: 5, data: { flags: 64 } };
    default: return { type: 5, data: { flags: 64 } };
  }
}

export default async function handler(req, res) {
  if (req.method === "GET") {
    return res.status(200).json({
      ok: true,
      service: "Mallow Bot",
      runtime: "Vercel",
      endpoint: "Discord interactions",
    });
  }
  if (req.method !== "POST") {
    res.setHeader("Allow", "GET, POST");
    return res.status(405).json({ error: "Method not allowed" });
  }

  const body = await readRawBody(req);
  const signature = String(req.headers["x-signature-ed25519"] || "").trim();
  const timestamp = String(req.headers["x-signature-timestamp"] || "").trim();
  if (!verifyDiscordRequest(body, signature, timestamp)) {
    return res.status(401).json({ error: "Invalid Discord signature" });
  }

  let payload;
  try {
    payload = JSON.parse(body.toString("utf8"));
  } catch {
    return res.status(400).json({ error: "Invalid JSON" });
  }

  if (Number(payload.type) !== 1) {
    const target = (process.env.BOT_INTERACTION_URL || "").trim();
    const secret = (process.env.WAKE_SECRET || "").trim();
    if (target && secret) {
      try {
        await fetch(target, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Mallow-Bot-Secret": secret,
          },
          body: JSON.stringify(payload),
          signal: AbortSignal.timeout(2500),
        });
      } catch {
        // Discord has already received the initial acknowledgement.
      }
    }
  }

  return res.status(200).json(initialResponse(payload.type));
}
