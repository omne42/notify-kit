import fs from "node:fs/promises"
import path from "node:path"
import process from "node:process"

async function safeReadJson(filePath) {
  try {
    const raw = await fs.readFile(filePath, "utf-8")
    return JSON.parse(raw)
  } catch (err) {
    if (err && err.code === "ENOENT") return null
    throw err
  }
}

async function atomicWriteUtf8(filePath, content) {
  await fs.mkdir(path.dirname(filePath), { recursive: true })

  const tmp = `${filePath}.${process.pid}.${Date.now()}.tmp`
  await fs.writeFile(tmp, content, "utf-8")
  try {
    await fs.rename(tmp, filePath)
  } catch (err) {
    // Windows may fail to replace an existing file.
    if (err && (err.code === "EEXIST" || err.code === "EPERM")) {
      await fs.unlink(filePath).catch(() => {})
      await fs.rename(tmp, filePath)
      return
    }
    await fs.unlink(tmp).catch(() => {})
    throw err
  }
}

let exitHooksInstalled = false

export function createSessionStore(filePath, { flushDebounceMs = 250 } = {}) {
  const map = new Map()
  const enabled = Boolean(filePath && String(filePath).trim() !== "")
  const storePath = enabled ? String(filePath) : null

  let flushTimer = null
  let pending = Promise.resolve()

  async function load() {
    if (!storePath) return
    const data = await safeReadJson(storePath)
    if (!data || typeof data !== "object") return
    for (const [k, v] of Object.entries(data)) {
      map.set(k, v)
    }
  }

  async function flushNow() {
    if (!storePath) return
    const obj = Object.fromEntries(map.entries())
    await atomicWriteUtf8(storePath, `${JSON.stringify(obj, null, 2)}\n`)
  }

  function scheduleFlush() {
    if (!storePath) return
    if (flushTimer) return
    flushTimer = setTimeout(() => {
      flushTimer = null
      pending = pending.then(flushNow).catch(() => {})
    }, flushDebounceMs)
  }

  function set(key, value) {
    map.set(key, value)
    scheduleFlush()
  }

  function del(key) {
    map.delete(key)
    scheduleFlush()
  }

  function installExitHooks() {
    if (!storePath) return
    if (exitHooksInstalled) return
    exitHooksInstalled = true

    const flush = () => flushNow().catch(() => {})
    process.on("beforeExit", flush)
    process.on("SIGINT", async () => {
      await flush()
      process.exit(0)
    })
    process.on("SIGTERM", async () => {
      await flush()
      process.exit(0)
    })
  }

  return {
    enabled: Boolean(storePath),
    path: storePath,
    map,
    load,
    flush: flushNow,
    set,
    delete: del,
    installExitHooks,
  }
}

