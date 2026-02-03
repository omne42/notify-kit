import { createOpencode } from "@opencode-ai/sdk"
import { DWClient, DWClientDownStream, EventAck, TOPIC_ROBOT } from "dingtalk-stream"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import { assertEnv, buildResponseText, getCompletedToolUpdate } from "../../_shared/opencode.mjs"

assertEnv("DINGTALK_CLIENT_ID")
assertEnv("DINGTALK_CLIENT_SECRET")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

const client = new DWClient({
  clientId: process.env.DINGTALK_CLIENT_ID,
  clientSecret: process.env.DINGTALK_CLIENT_SECRET,
})

/**
 * sessionKey = sessionWebhook
 * value = { sessionId, sessionWebhook }
 */
const sessions = store.map

function validateSessionWebhook(sessionWebhook) {
  let url
  try {
    url = new URL(String(sessionWebhook || ""))
  } catch {
    return null
  }

  if (url.protocol !== "https:") return null
  if (url.username || url.password) return null
  if (url.port && url.port !== "443") return null

  const host = url.hostname.toLowerCase()
  const isDingTalkHost =
    host === "dingtalk.com" ||
    host.endsWith(".dingtalk.com") ||
    host === "dingtalk.cn" ||
    host.endsWith(".dingtalk.cn")
  if (!isDingTalkHost) return null

  return url.toString()
}

async function postSessionMessage(sessionWebhook, text) {
  const accessToken = await client.getAccessToken()
  await ignoreError(
    fetch(sessionWebhook, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-acs-dingtalk-access-token": accessToken,
      },
      body: JSON.stringify({
        msgtype: "text",
        text: { content: text },
      }),
    }),
    "dingtalk send message failed",
  )
}

async function ensureSession(sessionWebhook) {
  const sessionKey = sessionWebhook
  let session = sessions.get(sessionKey)
  if (session) return session

  const created = await opencode.client.session.create({
    body: { title: "DingTalk session" },
  })
  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  session = { sessionId: created.data.id, sessionWebhook }
  store.set(sessionKey, session)

  const share = await opencode.client.session.share({ path: { id: session.sessionId } })
  const url = share?.data?.share?.url
  if (url) {
    await postSessionMessage(sessionWebhook, url)
  }

  return session
}

async function handleUserText(sessionWebhook, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await postSessionMessage(sessionWebhook, "Bot is working.")
    return
  }

  await limiter.run(async () => {
    let session
    try {
      session = await ensureSession(sessionWebhook)
    } catch {
      await postSessionMessage(sessionWebhook, "Sorry, I had trouble creating a session.")
      return
    }

    const result = await opencode.client.session.prompt({
      path: { id: session.sessionId },
      body: { parts: [{ type: "text", text: trimmed }] },
    })

    if (result.error) {
      await postSessionMessage(sessionWebhook, "Sorry, I had trouble processing your message.")
      return
    }

    const responseText = buildResponseText(result.data)

    await postSessionMessage(sessionWebhook, responseText)
  })
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  for (const session of sessions.values()) {
    if (session.sessionId !== update.sessionId) continue
    await postSessionMessage(session.sessionWebhook, `${update.tool} - ${update.title}`)
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

const downstream = new DWClientDownStream(client)
downstream.registerCallbackListener(TOPIC_ROBOT, (res) => {
  const sessionWebhook = validateSessionWebhook(res?.data?.sessionWebhook)
  const content = res?.data?.text?.content
  if (!sessionWebhook || !content) return EventAck.SUCCESS

  queueMicrotask(() => {
    handleUserText(sessionWebhook, content).catch((err) => {
      console.error("handle message failed:", err)
    })
  })

  return EventAck.SUCCESS
})

await downstream.connect()
console.log("⚡️ DingTalk Stream bot is running!")
