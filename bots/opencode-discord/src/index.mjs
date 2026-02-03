import process from "node:process"

import { createOpencode } from "@opencode-ai/sdk"
import { Client, GatewayIntentBits, Partials } from "discord.js"

function assertEnv(name) {
  const value = process.env[name]
  if (value === undefined || String(value).trim() === "") {
    throw new Error(`missing required env: ${name}`)
  }
  return value
}

function truncateForDiscord(text, max = 1900) {
  const s = String(text || "")
  if (s.length <= max) return s
  return `${s.slice(0, max - 20)}\n\n[truncated]\n`
}

const discordToken = assertEnv("DISCORD_BOT_TOKEN")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

/**
 * channelId -> sessionId
 */
const channelToSession = new Map()
/**
 * sessionId -> channelId
 */
const sessionToChannel = new Map()

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
  const channel = await client.channels.fetch(channelId).catch(() => null)
  if (!channel || !channel.isTextBased()) return
  await channel.send(truncateForDiscord(text)).catch(() => {})
}

async function ensureSession(channelId) {
  const existing = channelToSession.get(channelId)
  if (existing) return { channelId, sessionId: existing }

  const created = await opencode.client.session.create({
    body: { title: `Discord channel ${channelId}` },
  })
  if (created.error) {
    throw new Error(created.error.message || "failed to create session")
  }

  const sessionId = created.data.id
  channelToSession.set(channelId, sessionId)
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
  const responseText =
    response?.info?.content ||
    response?.parts
      ?.filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("\n") ||
    "I received your message but didn't have a response."

  await postChannelMessage(channelId, responseText)
}

async function handleToolUpdate(part) {
  if (!part || part.type !== "tool") return
  if (!part.state || part.state.status !== "completed") return

  const sessionId = part.sessionID
  const channelId = sessionToChannel.get(sessionId)
  if (!channelId) return

  const title = part.state.title || "completed"
  const tool = part.tool || "tool"
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

