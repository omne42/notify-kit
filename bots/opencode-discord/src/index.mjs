import process from "node:process"

import { createOpencode } from "@opencode-ai/sdk"
import { Client, GatewayIntentBits, Partials } from "discord.js"

import { createLimiter } from "../../_shared/limiter.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import { assertEnv, buildResponseText, getCompletedToolUpdate } from "../../_shared/opencode.mjs"
import { createSessionStore } from "../../_shared/session_store.mjs"

function truncateForDiscord(text, max = 1900) {
  const s = String(text || "")
  if (s.length <= max) return s
  return `${s.slice(0, max - 20)}\n\n[truncated]\n`
}

const discordToken = assertEnv("DISCORD_BOT_TOKEN")

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

/**
 * channelId -> sessionId
 */
const channelToSession = store.map
/**
 * sessionId -> channelId
 */
const sessionToChannel = new Map()

for (const [channelId, value] of channelToSession.entries()) {
  const sessionId = typeof value === "string" ? value : value?.sessionId
  if (sessionId) {
    sessionToChannel.set(sessionId, channelId)
  }
}

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMessages,
    GatewayIntentBits.DirectMessages,
    GatewayIntentBits.MessageContent,
  ],
  partials: [Partials.Channel],
})

async function postChannelMessage(channelId, text) {
  const channel = await ignoreError(client.channels.fetch(channelId), "discord fetch channel failed")
  if (!channel || !channel.isTextBased()) return
  await ignoreError(channel.send(truncateForDiscord(text)), "discord channel send failed")
}

async function ensureSession(channelId) {
  const existing = channelToSession.get(channelId)
  const existingSessionId = typeof existing === "string" ? existing : existing?.sessionId
  if (existingSessionId) return { channelId, sessionId: existingSessionId }

  const created = await opencode.client.session.create({
    body: { title: `Discord channel ${channelId}` },
  })
  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  const sessionId = created.data.id
  store.set(channelId, sessionId)
  sessionToChannel.set(sessionId, channelId)

  const share = await opencode.client.session.share({ path: { id: sessionId } })
  const url = share?.data?.share?.url
  if (url) {
    await postChannelMessage(channelId, url)
  }

  return { channelId, sessionId }
}

async function handleUserText(channelId, text) {
  const trimmed = String(text || "").trim()
  if (!trimmed) return

  if (trimmed === "/test") {
    await postChannelMessage(channelId, "Bot is working.")
    return
  }

  await limiter.run(async () => {
    let session
    try {
      session = await ensureSession(channelId)
    } catch {
      await postChannelMessage(channelId, "Sorry, I had trouble creating a session.")
      return
    }

    const result = await opencode.client.session.prompt({
      path: { id: session.sessionId },
      body: { parts: [{ type: "text", text: trimmed }] },
    })

    if (result.error) {
      await postChannelMessage(channelId, "Sorry, I had trouble processing your message.")
      return
    }

    const response = result.data
    const responseText = buildResponseText(response)

    await postChannelMessage(channelId, responseText)
  })
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const sessionId = update.sessionId
  const channelId = sessionToChannel.get(sessionId)
  if (!channelId) return

  const title = update.title
  const tool = update.tool
  await postChannelMessage(channelId, `*${tool}* - ${title}`)
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

client.on("messageCreate", (message) => {
  if (!message) return
  if (message.author?.bot) return
  if (!message.content) return

  const channelId = message.channelId
  queueMicrotask(() => {
    handleUserText(channelId, message.content).catch((err) => {
      console.error("handle message failed:", err)
    })
  })
})

client.once("ready", () => {
  console.log(`⚡️ Discord bot is running as ${client.user?.tag || "unknown"}`)
})

await client.login(discordToken)
