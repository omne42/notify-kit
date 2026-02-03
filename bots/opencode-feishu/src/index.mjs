import http from "http"
import * as lark from "@larksuiteoapi/node-sdk"
import { createOpencode } from "@opencode-ai/sdk"

import { createLimiter } from "../../_shared/limiter.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import { assertEnv, buildResponseText, getCompletedToolUpdate } from "../../_shared/opencode.mjs"
import { createSessionStore } from "../../_shared/session_store.mjs"

assertEnv("FEISHU_APP_ID")
assertEnv("FEISHU_APP_SECRET")
assertEnv("FEISHU_VERIFICATION_TOKEN")
assertEnv("FEISHU_ENCRYPT_KEY", { optional: true })

const port = Number.parseInt(process.env.PORT || "3000", 10)

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createLimiter({ maxInflight: process.env.OPENCODE_BOT_MAX_INFLIGHT || "4" })
const store = createSessionStore(process.env.OPENCODE_SESSION_STORE_PATH, {
  rootDir: process.env.OPENCODE_SESSION_STORE_ROOT || process.cwd(),
})
await store.load()
store.installExitHooks()
if (store.enabled) {
  console.log(`🗄️ Session store enabled: ${store.path}`)
}

const client = new lark.Client({
  appId: process.env.FEISHU_APP_ID,
  appSecret: process.env.FEISHU_APP_SECRET,
})

/**
 * sessionKey = `${tenantKey ?? "default"}-${chatId}`
 * value = { sessionId, tenantKey, chatId }
 */
const sessions = store.map

async function sendTextToChat(tenantKey, chatId, text) {
  if (!chatId || !text) return
  const req = {
    params: { receive_id_type: "chat_id" },
    data: {
      receive_id: chatId,
      msg_type: "text",
      content: JSON.stringify({ text }),
    },
  }

  const tenantOpt =
    tenantKey && String(tenantKey).trim() !== "" ? lark.withTenantKey(tenantKey) : undefined

  await ignoreError(client.im.message.create(req, tenantOpt), "feishu send message failed")
}

async function ensureSession(tenantKey, chatId) {
  const sessionKey = `${tenantKey || "default"}-${chatId}`
  let session = sessions.get(sessionKey)
  if (session) return session

  const created = await opencode.client.session.create({
    body: { title: `Feishu chat ${chatId}` },
  })

  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  session = { sessionId: created.data.id, tenantKey, chatId }
  store.set(sessionKey, session)

  const share = await opencode.client.session.share({ path: { id: session.sessionId } })
  const url = share?.data?.share?.url
  if (url) {
    await sendTextToChat(tenantKey, chatId, url)
  }

  return session
}

async function handleUserText(tenantKey, chatId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await sendTextToChat(tenantKey, chatId, "Bot is working.")
    return
  }

  await limiter.run(async () => {
    let session
    try {
      session = await ensureSession(tenantKey, chatId)
    } catch {
      await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble creating a session.")
      return
    }

    const result = await opencode.client.session.prompt({
      path: { id: session.sessionId },
      body: { parts: [{ type: "text", text: trimmed }] },
    })

    if (result.error) {
      await sendTextToChat(tenantKey, chatId, "Sorry, I had trouble processing your message.")
      return
    }

    const response = result.data
    const responseText = buildResponseText(response)

    await sendTextToChat(tenantKey, chatId, responseText)
  })
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const sessionId = update.sessionId
  for (const session of sessions.values()) {
    if (session.sessionId !== sessionId) continue
    await sendTextToChat(
      session.tenantKey,
      session.chatId,
      `${update.tool} - ${update.title}`,
    )
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

const dispatcher = new lark.EventDispatcher({
  encryptKey: process.env.FEISHU_ENCRYPT_KEY,
  verificationToken: process.env.FEISHU_VERIFICATION_TOKEN,
}).register({
  "im.message.receive_v1": async (data) => {
    if (!data || !data.message || !data.sender) return
    if (data.sender.sender_type !== "user") return

    if (data.message.message_type !== "text") return

    const tenantKey = data.tenant_key || data.sender.tenant_key || null
    const chatId = data.message.chat_id
    const content = data.message.content
    if (!chatId || !content) return

    let text
    try {
      text = JSON.parse(content).text
    } catch {
      return
    }

    queueMicrotask(() => {
      handleUserText(tenantKey, chatId, text).catch((err) => {
        console.error("handle message failed:", err)
      })
    })
  },
})

const server = http.createServer()
server.on(
  "request",
  lark.adaptDefault("/webhook/event", dispatcher, {
    autoChallenge: true,
  }),
)
server.listen(port, () => {
  console.log(`⚡️ Feishu bot is listening on :${port}`)
})
