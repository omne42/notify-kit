import process from "node:process"

import { createLimiter } from "./limiter.mjs"
import { createSessionStore } from "./session_store.mjs"

export function createBotLimiter() {
  return createLimiter({ maxInflight: process.env.OPENCODE_BOT_MAX_INFLIGHT || "4" })
}

export async function createBotSessionStore() {
  const store = createSessionStore(process.env.OPENCODE_SESSION_STORE_PATH, {
    rootDir: process.env.OPENCODE_SESSION_STORE_ROOT || process.cwd(),
  })
  await store.load()
  store.installExitHooks()
  if (store.enabled) {
    console.log(`🗄️ Session store enabled: ${store.path}`)
  }
  return store
}

