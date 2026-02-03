import crypto from "node:crypto"
import http from "node:http"
import { URL } from "node:url"

import { XMLParser } from "fast-xml-parser"
import { createOpencode } from "@opencode-ai/sdk"

function assertEnv(name, { optional = false } = {}) {
  const value = process.env[name]
  if ((value === undefined || String(value).trim() === "") && !optional) {
    throw new Error(`missing required env: ${name}`)
  }
  return value
}

assertEnv("WECOM_CORP_ID")
assertEnv("WECOM_CORP_SECRET")
assertEnv("WECOM_AGENT_ID")
assertEnv("WECOM_TOKEN")
assertEnv("WECOM_ENCODING_AES_KEY")

const port = Number.parseInt(process.env.PORT || "3000", 10)
const sessionScope = (process.env.WECOM_SESSION_SCOPE || "user").toLowerCase()
const replyTo = (process.env.WECOM_REPLY_TO || "user").toLowerCase()

const xml = new XMLParser({ ignoreAttributes: true })

function sha1Hex(value) {
  return crypto.createHash("sha1").update(String(value)).digest("hex")
}

function computeSignature(token, timestamp, nonce, encrypted) {
  const items = [token, timestamp, nonce, encrypted].map((v) => String(v || ""))
  items.sort()
  return sha1Hex(items.join(""))
}

function decodeAesKey(encodingAesKey) {
  // WeCom provides 43 chars base64; add '=' padding to make it valid base64.
  const key = Buffer.from(`${encodingAesKey}=`, "base64")
  if (key.length !== 32) throw new Error("invalid WECOM_ENCODING_AES_KEY (expected 32 bytes after base64 decode)")
  return key
}

function pkcs7Unpad(buf) {
  if (!buf || buf.length === 0) throw new Error("invalid pkcs7 padding")
  const pad = buf[buf.length - 1]
  if (pad < 1 || pad > 32) throw new Error("invalid pkcs7 padding length")
  for (let i = 1; i <= pad; i += 1) {
    if (buf[buf.length - i] !== pad) throw new Error("invalid pkcs7 padding")
  }
  return buf.subarray(0, buf.length - pad)
}

function decryptWeCom(encryptedBase64, encodingAesKey) {
  const aesKey = decodeAesKey(encodingAesKey)
  const iv = aesKey.subarray(0, 16)
  const cipherText = Buffer.from(String(encryptedBase64 || ""), "base64")

  const decipher = crypto.createDecipheriv("aes-256-cbc", aesKey, iv)
  decipher.setAutoPadding(false)
  let plain = Buffer.concat([decipher.update(cipherText), decipher.final()])
  plain = pkcs7Unpad(plain)

  if (plain.length < 20) throw new Error("invalid decrypted message")
  const msgLen = plain.readUInt32BE(16)
  const msgStart = 20
  const msgEnd = msgStart + msgLen
  const xmlText = plain.subarray(msgStart, msgEnd).toString("utf-8")
  const receiver = plain.subarray(msgEnd).toString("utf-8").replace(/\0+$/u, "")

  return { xmlText, receiver }
}

function assertReceiverOrThrow(receiver) {
  const expected = String(process.env.WECOM_CORP_ID || "").trim()
  const actual = String(receiver || "").trim()
  if (!expected || !actual || expected !== actual) {
    throw new Error("invalid receiver corp id")
  }
}

async function readRequestBody(req, { limitBytes = 1024 * 1024 } = {}) {
  const chunks = []
  let size = 0
  for await (const chunk of req) {
    size += chunk.length
    if (size > limitBytes) throw new Error("request body too large")
    chunks.push(chunk)
  }
  return Buffer.concat(chunks).toString("utf-8")
}

let accessTokenCache = null
let accessTokenExpiresAtMs = 0

async function getWeComAccessToken() {
  const now = Date.now()
  if (accessTokenCache && now < accessTokenExpiresAtMs) return accessTokenCache

  const corpId = process.env.WECOM_CORP_ID
  const corpSecret = process.env.WECOM_CORP_SECRET
  const url = new URL("https://qyapi.weixin.qq.com/cgi-bin/gettoken")
  url.searchParams.set("corpid", corpId)
  url.searchParams.set("corpsecret", corpSecret)

  const resp = await fetch(url, { method: "GET" })
  const data = await resp.json().catch(() => null)
  if (!resp.ok || !data || data.errcode) {
    throw new Error(`wecom gettoken failed: ${data?.errmsg || resp.status}`)
  }

  accessTokenCache = data.access_token
  const expiresInSec = Number.parseInt(String(data.expires_in || "7200"), 10)
  accessTokenExpiresAtMs = now + Math.max(60, expiresInSec - 120) * 1000
  return accessTokenCache
}

async function wecomPost(path, body) {
  const token = await getWeComAccessToken()
  const url = new URL(`https://qyapi.weixin.qq.com/cgi-bin/${path}`)
  url.searchParams.set("access_token", token)

  const resp = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  })
  const data = await resp.json().catch(() => null)
  if (!resp.ok || !data || data.errcode) {
    throw new Error(`wecom api failed (${path}): ${data?.errmsg || resp.status}`)
  }
  return data
}

async function sendTextToUser(userId, text) {
  if (!userId || !text) return
  const agentId = Number.parseInt(String(process.env.WECOM_AGENT_ID), 10)
  await wecomPost("message/send", {
    touser: userId,
    msgtype: "text",
    agentid: agentId,
    text: { content: text },
    safe: 0,
  }).catch(() => {})
}

async function sendTextToChat(chatId, text) {
  if (!chatId || !text) return
  await wecomPost("appchat/send", {
    chatid: chatId,
    msgtype: "text",
    text: { content: text },
  }).catch(() => {})
}

async function sendText({ userId, chatId }, text) {
  if (!text) return
  if (replyTo === "chat" && chatId) {
    await sendTextToChat(chatId, text)
    return
  }
  await sendTextToUser(userId, text)
}

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

/**
 * sessionKey = `${scope}-${id}`
 * value = { sessionId, userId, chatId }
 */
const sessions = new Map()

function getSessionKey({ userId, chatId }) {
  if (sessionScope === "chat" && chatId) return `chat-${chatId}`
  return `user-${userId}`
}

async function ensureSession(ctx) {
  const key = getSessionKey(ctx)
  let session = sessions.get(key)
  if (session) return session

  const created = await opencode.client.session.create({
    body: { title: `WeCom ${key}` },
  })
  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  session = { sessionId: created.data.id, userId: ctx.userId, chatId: ctx.chatId || null }
  sessions.set(key, session)

  const share = await opencode.client.session.share({ path: { id: session.sessionId } })
  const url = share?.data?.share?.url
  if (url) {
    await sendText(session, url)
  }

  return session
}

async function handleUserText(ctx, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendText(ctx, "Bot is working.")
    return
  }

  let session
  try {
    session = await ensureSession(ctx)
  } catch {
    await sendText(ctx, "Sorry, I had trouble creating a session.")
    return
  }

  const result = await opencode.client.session.prompt({
    path: { id: session.sessionId },
    body: { parts: [{ type: "text", text: trimmed }] },
  })

  if (result.error) {
    await sendText(ctx, "Sorry, I had trouble processing your message.")
    return
  }

  const response = result.data
  const responseText =
    response?.info?.content ||
    response?.parts
      ?.filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("\n") ||
    "I received your message but didn't have a response."

  await sendText(ctx, responseText)
}

async function handleToolUpdate(part) {
  if (!part || part.type !== "tool") return
  if (!part.state || part.state.status !== "completed") return

  const sessionId = part.sessionID
  for (const session of sessions.values()) {
    if (session.sessionId !== sessionId) continue
    const title = part.state.title || "completed"
    const tool = part.tool || "tool"
    await sendText(session, `${tool} - ${title}`)
    break
  }
}

;(async () => {
  const events = await opencode.client.event.subscribe()
  for await (const event of events.stream) {
    if (event?.type !== "message.part.updated") continue
    const part = event?.properties?.part
    await handleToolUpdate(part)
  }
})().catch((err) => {
  console.error("event subscription failed:", err)
  process.exitCode = 1
})

function parseWeComEncryptedXml(encryptedXmlText) {
  const parsed = xml.parse(encryptedXmlText)
  const root = parsed?.xml || parsed
  const encrypt = root?.Encrypt
  return String(encrypt || "")
}

function parseWeComPlainXml(plainXmlText) {
  const parsed = xml.parse(plainXmlText)
  const root = parsed?.xml || parsed
  return {
    toUserName: root?.ToUserName || null,
    fromUserName: root?.FromUserName || null,
    agentId: root?.AgentID || null,
    msgType: root?.MsgType || null,
    content: root?.Content || null,
    chatId: root?.ChatId || null,
  }
}

function verifySignatureOrThrow({ signature, timestamp, nonce, encrypted }) {
  const token = process.env.WECOM_TOKEN
  const expected = computeSignature(token, timestamp, nonce, encrypted)
  if (String(signature || "") !== expected) {
    throw new Error("invalid msg_signature")
  }
}

const REPLAY_WINDOW_SECONDS = 5 * 60
const REPLAY_CACHE_TTL_MS = 10 * 60 * 1000
const replayCache = new Map()

function isFreshTimestamp(timestamp) {
  const ts = Number.parseInt(String(timestamp || ""), 10)
  if (!Number.isFinite(ts) || ts <= 0) return false
  const now = Math.floor(Date.now() / 1000)
  return Math.abs(now - ts) <= REPLAY_WINDOW_SECONDS
}

function checkAndRememberReplay(signature, timestamp, nonce) {
  const key = `${timestamp}:${nonce}:${signature}`
  const now = Date.now()

  // Cleanup expired entries (best-effort).
  for (const [k, exp] of replayCache.entries()) {
    if (exp <= now) replayCache.delete(k)
  }

  if (replayCache.has(key)) {
    return false
  }

  replayCache.set(key, now + REPLAY_CACHE_TTL_MS)
  return true
}

function sendTextResponse(res, status, body) {
  res.statusCode = status
  res.setHeader("content-type", "text/plain; charset=utf-8")
  res.end(body)
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`)

  if (url.pathname !== "/webhook/wecom") {
    sendTextResponse(res, 404, "not found")
    return
  }

  if (req.method === "GET") {
    const signature = url.searchParams.get("msg_signature")
    const timestamp = url.searchParams.get("timestamp")
    const nonce = url.searchParams.get("nonce")
    const echostr = url.searchParams.get("echostr")

    if (!signature || !timestamp || !nonce || !echostr) {
      sendTextResponse(res, 400, "missing query params")
      return
    }

    try {
      verifySignatureOrThrow({ signature, timestamp, nonce, encrypted: echostr })
      const { xmlText, receiver } = decryptWeCom(echostr, process.env.WECOM_ENCODING_AES_KEY)
      assertReceiverOrThrow(receiver)
      sendTextResponse(res, 200, xmlText)
    } catch (err) {
      console.error("wecom verify failed:", err?.message || err)
      sendTextResponse(res, 403, "forbidden")
    }
    return
  }

  if (req.method === "POST") {
    const signature = url.searchParams.get("msg_signature")
    const timestamp = url.searchParams.get("timestamp")
    const nonce = url.searchParams.get("nonce")

    let rawBody
    try {
      rawBody = await readRequestBody(req)
    } catch (err) {
      console.error("wecom read body failed:", err?.message || err)
      sendTextResponse(res, 413, "payload too large")
      return
    }

    // Respond fast to WeCom; do heavy work async.
    sendTextResponse(res, 200, "success")

    queueMicrotask(() => {
      try {
        const encrypted = parseWeComEncryptedXml(rawBody)
        if (!encrypted) return

        if (!signature || !timestamp || !nonce) return
        if (!isFreshTimestamp(timestamp)) return
        if (!checkAndRememberReplay(signature, timestamp, nonce)) return
        verifySignatureOrThrow({ signature, timestamp, nonce, encrypted })

        const { xmlText, receiver } = decryptWeCom(encrypted, process.env.WECOM_ENCODING_AES_KEY)
        assertReceiverOrThrow(receiver)
        const msg = parseWeComPlainXml(xmlText)

        const userId = msg.fromUserName
        const chatId = msg.chatId
        const msgType = msg.msgType
        const content = msg.content

        if (!userId) return
        if (msgType !== "text") return

        const ctx = { userId, chatId }
        handleUserText(ctx, content).catch((err) => {
          console.error("handle message failed:", err)
        })
      } catch (err) {
        console.error("wecom handle webhook failed:", err?.message || err)
      }
    })

    return
  }

  sendTextResponse(res, 405, "method not allowed")
})

server.listen(port, () => {
  console.log(`⚡️ WeCom bot is listening on :${port} (/webhook/wecom)`)
})
