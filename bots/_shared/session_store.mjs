import fsSync from "node:fs"
import fs from "node:fs/promises"
import path from "node:path"
import process from "node:process"

import { ignoreError, logError } from "./log.mjs"

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
      await ignoreError(fs.unlink(filePath), "session store unlink failed")
      await fs.rename(tmp, filePath)
      return
    }
    await ignoreError(fs.unlink(tmp), "session store unlink failed")
    throw err
  }
}

let exitHooksInstalled = false

function safeRealpathSync(p) {
  try {
    return fsSync.realpathSync(p)
  } catch {
    return null
  }
}

function resolveStorePath(filePath, rootDir) {
  const raw = String(filePath || "").trim()
  if (!raw) return null

  const root = rootDir && String(rootDir).trim() !== "" ? path.resolve(String(rootDir)) : null
  const resolved = path.isAbsolute(raw) ? path.resolve(raw) : path.resolve(root || process.cwd(), raw)

  if (!root) return resolved

  const rootReal = safeRealpathSync(root) || root
  const storeDir = path.dirname(resolved)
  const storeDirReal = safeRealpathSync(storeDir) || storeDir

  const rel = path.relative(rootReal, storeDirReal)
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new Error(`session store path must be within rootDir: ${root}`)
  }

  return resolved
}

export function createSessionStore(filePath, { flushDebounceMs = 250, rootDir = null } = {}) {
  const map = new Map()
  const storePath = resolveStorePath(filePath, rootDir)

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
      pending = pending.then(flushNow).catch((err) => {
        logError("session store flush failed", err)
      })
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

    const flush = () => flushNow().catch((err) => logError("session store flush failed", err))
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
