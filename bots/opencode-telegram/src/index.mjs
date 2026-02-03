import process from "node:process"

import { createOpencode } from "@opencode-ai/sdk"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import { assertEnv, buildResponseText, getCompletedToolUpdate } from "../../_shared/opencode.mjs"

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function truncateForTelegram(text, max = 3800) {
  const s = String(text || "")
  if (s.length <= max) return s
  return `${s.slice(0, max - 20)}\n\n[truncated]\n`
}

const token = assertEnv("TELEGRAM_BOT_TOKEN")
const apiBase = `https://api.telegram.org/bot${token}`

async function tg(method, payload) {
  const resp = await fetch(`${apiBase}/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload ?? {}),
  })

  const data = await resp.json().catch(() => null)
  if (!resp.ok || !data?.ok) {
    const desc = data?.description || `http ${resp.status}`
    throw new Error(`telegram api error: ${method}: ${desc}`)
  }

  return data.result
}

async function sendMessage(chatId, text) {
  await ignoreError(
    tg("sendMessage", { chat_id: chatId, text: truncateForTelegram(text) }),
    "telegram sendMessage failed",
  )
}

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * chatId -> sessionId
 */
const chatToSession = store.map
/**
 * sessionId -> chatId
 */
const sessionToChat = new Map()
for (const [chatId, sessionId] of chatToSession.entries()) {
  if (!chatId) continue
  if (typeof sessionId !== "string" || !sessionId) continue
  sessionToChat.set(sessionId, chatId)
}

async function ensureSession(chatId) {
  const existing = chatToSession.get(chatId)
  if (typeof existing === "string" && existing) return { chatId, sessionId: existing }

  const created = await opencode.client.session.create({
    body: { title: `Telegram chat ${chatId}` },
  })
  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  const sessionId = created.data.id
  store.set(chatId, sessionId)
  sessionToChat.set(sessionId, chatId)

  const share = await opencode.client.session.share({ path: { id: sessionId } })
  const url = share?.data?.share?.url
  if (url) {
    await sendMessage(chatId, url)
  }

  return { chatId, sessionId }
}

async function handleUserText(chatId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendMessage(chatId, "Bot is working.")
    return
  }

  await limiter.run(async () => {
    let session
    try {
      session = await ensureSession(chatId)
    } catch {
      await sendMessage(chatId, "Sorry, I had trouble creating a session.")
      return
    }

    const result = await opencode.client.session.prompt({
      path: { id: session.sessionId },
      body: { parts: [{ type: "text", text: trimmed }] },
    })

    if (result.error) {
      await sendMessage(chatId, "Sorry, I had trouble processing your message.")
      return
    }

    const responseText = buildResponseText(result.data)
    await sendMessage(chatId, responseText)
  })
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const chatId = sessionToChat.get(update.sessionId)
  if (!chatId) return

  await sendMessage(chatId, `${update.tool} - ${update.title}`)
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

let offset = 0
for (;;) {
  try {
    const updates = await tg("getUpdates", {
      timeout: 30,
      offset,
      allowed_updates: ["message"],
    })

    if (Array.isArray(updates)) {
      for (const update of updates) {
        offset = Math.max(offset, Number(update.update_id || 0) + 1)
        const msg = update.message
        if (!msg || !msg.text) continue
        if (msg.from?.is_bot) continue

        const chatId = String(msg.chat?.id || "")
        if (!chatId) continue

        await handleUserText(chatId, msg.text)
      }
    }
  } catch (err) {
    console.error("telegram poll failed:", err?.message || err)
    await sleep(1000)
  }
}
