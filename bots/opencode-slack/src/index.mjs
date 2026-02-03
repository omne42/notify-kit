import { App } from "@slack/bolt"
import { createOpencode } from "@opencode-ai/sdk"

import { createBotLimiter, createBotSessionStore } from "../../_shared/bootstrap.mjs"
import { ignoreError } from "../../_shared/log.mjs"
import { assertEnv, buildResponseText, getCompletedToolUpdate } from "../../_shared/opencode.mjs"

const app = new App({
  token: process.env.SLACK_BOT_TOKEN,
  signingSecret: process.env.SLACK_SIGNING_SECRET,
  socketMode: true,
  appToken: process.env.SLACK_APP_TOKEN,
})

assertEnv("SLACK_BOT_TOKEN")
assertEnv("SLACK_SIGNING_SECRET")
assertEnv("SLACK_APP_TOKEN")

console.log("🚀 Starting opencode server...")
const opencode = await createOpencode({ port: 0 })
console.log("✅ Opencode server ready")

const limiter = createBotLimiter()
const store = await createBotSessionStore()

/**
 * sessionKey = `${channel}-${threadTs}`
 * value = { sessionId, channel, threadTs }
 */
const sessions = store.map

async function postThreadMessage(channel, threadTs, text) {
  await ignoreError(
    app.client.chat.postMessage({
      channel,
      thread_ts: threadTs,
      text,
    }),
    "slack postMessage failed",
  )
}

async function handleToolUpdate(part) {
  const update = getCompletedToolUpdate(part)
  if (!update) return

  const sessionId = update.sessionId
  for (const session of sessions.values()) {
    if (session.sessionId !== sessionId) continue
    const title = update.title
    const tool = update.tool
    await postThreadMessage(session.channel, session.threadTs, `*${tool}* - ${title}`)
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

app.message(async ({ message, say }) => {
  if (!message || message.subtype || !("text" in message) || !message.text) return

  await limiter.run(async () => {
    const channel = message.channel
    const threadTs = message.thread_ts || message.ts
    const sessionKey = `${channel}-${threadTs}`

    let session = sessions.get(sessionKey)
    if (!session) {
      const createResult = await opencode.client.session.create({
        body: { title: `Slack thread ${threadTs}` },
      })
      if (createResult.error) {
        await say({
          text: "Sorry, I had trouble creating a session. Please try again.",
          thread_ts: threadTs,
        })
        return
      }

      session = { sessionId: createResult.data.id, channel, threadTs }
      store.set(sessionKey, session)

      const shareResult = await opencode.client.session.share({ path: { id: session.sessionId } })
      const url = shareResult?.data?.share?.url
      if (url) {
        await postThreadMessage(channel, threadTs, url)
      }
    }

    const result = await opencode.client.session.prompt({
      path: { id: session.sessionId },
      body: { parts: [{ type: "text", text: message.text }] },
    })

    if (result.error) {
      await say({
        text: "Sorry, I had trouble processing your message. Please try again.",
        thread_ts: threadTs,
      })
      return
    }

    const response = result.data
    const responseText = buildResponseText(response)

    await say({ text: responseText, thread_ts: threadTs })
  })
})

app.command("/test", async ({ ack, say }) => {
  await ack()
  await say("Bot is working.")
})

await app.start()
console.log("⚡️ Slack bot is running!")
