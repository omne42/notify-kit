import crypto from "node:crypto"
import fsSync from "node:fs"
import fs from "node:fs/promises"
import path from "node:path"
import process from "node:process"

import { ignoreError, isVerbose, logError } from "./log.mjs"

async function safeReadJson(filePath, { maxBytes = 0 } = {}) {
  try {
    if (Number.isFinite(maxBytes) && maxBytes > 0) {
      const st = await fs.stat(filePath)
      if (st.size > maxBytes) {
        console.error(
          `session store file too large (${st.size} bytes > ${maxBytes} bytes), skipping load`,
        )
        return null
      }
    }
    const raw = await fs.readFile(filePath, "utf-8")
    return JSON.parse(raw)
  } catch (err) {
    if (err && err.code === "ENOENT") return null
    if (err instanceof SyntaxError) {
      logError("session store parse failed", err)
      const msg = err?.message || String(err)
      console.error(`session store parse failed: ${msg}`)
      return null
    }
    throw err
  }
}

async function atomicWriteUtf8(filePath, content, { root = null, rootReal = null } = {}) {
  const dir = path.dirname(filePath)
  await fs.mkdir(dir, { recursive: true })
  if (root && rootReal) {
    assertDirRealWithinRoot(root, rootReal, dir)
  }

  const entropy =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`
  const tmp = `${filePath}.${process.pid}.${entropy}.tmp`
  await fs.writeFile(tmp, content, "utf-8")
  try {
    await fs.rename(tmp, filePath)
  } catch (err) {
    // Windows may fail to replace an existing file.
    if (err && (err.code === "EEXIST" || err.code === "EPERM")) {
      await ignoreError(fs.unlink(filePath), "session store unlink failed")
      try {
        await fs.rename(tmp, filePath)
        return
      } catch (renameErr) {
        await ignoreError(fs.unlink(tmp), "session store unlink failed")
        throw renameErr
      }
    }
    await ignoreError(fs.unlink(tmp), "session store unlink failed")
    throw err
  }
}

let exitHooksInstalled = false
const exitHookFlushers = new Set()

function installGlobalExitHooks() {
  if (exitHooksInstalled) return
  exitHooksInstalled = true

  const flushAll = async () => {
    const tasks = [...exitHookFlushers].map((fn) => fn())
    await Promise.allSettled(tasks)
  }

  process.on("beforeExit", () => {
    void flushAll()
  })
  process.on("SIGINT", async () => {
    await flushAll()
    process.exit(0)
  })
  process.on("SIGTERM", async () => {
    await flushAll()
    process.exit(0)
  })
}

function safeRealpathSync(p) {
  try {
    return fsSync.realpathSync(p)
  } catch {
    return null
  }
}

function isPathWithinRoot(rootReal, targetReal) {
  if (!rootReal || !targetReal) return false
  if (rootReal === targetReal) return true
  const rel = path.relative(rootReal, targetReal)
  return rel !== "" && !rel.startsWith("..") && !path.isAbsolute(rel)
}

function assertNoSymlinkEscape(rootAbs, rootReal, targetAbs) {
  const rel = path.relative(rootAbs, targetAbs)
  if (rel.startsWith("..") || path.isAbsolute(rel)) {
    throw new Error(`session store path must be within rootDir: ${rootAbs}`)
  }

  let cur = rootAbs
  for (const seg of rel.split(path.sep)) {
    if (!seg || seg === ".") continue
    cur = path.join(cur, seg)
    if (!fsSync.existsSync(cur)) continue

    let st
    try {
      st = fsSync.lstatSync(cur)
    } catch {
      continue
    }
    if (!st.isSymbolicLink()) continue

    const curReal = safeRealpathSync(cur)
    if (!curReal) {
      throw new Error(`session store path contains unresolved symlink: ${cur}`)
    }
    if (!isPathWithinRoot(rootReal, curReal)) {
      throw new Error(`session store path must be within rootDir: ${rootAbs}`)
    }
  }
}

function assertDirRealWithinRoot(rootAbs, rootReal, dirAbs) {
  const dirReal = safeRealpathSync(dirAbs)
  if (!dirReal) {
    throw new Error(`session store realpath failed: ${dirAbs}`)
  }
  if (!isPathWithinRoot(rootReal, dirReal)) {
    throw new Error(`session store path must be within rootDir: ${rootAbs}`)
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

  assertNoSymlinkEscape(root, rootReal, storeDir)

  return resolved
}

export function createSessionStore(
  filePath,
  { flushDebounceMs = 250, rootDir = null, maxEntries = 0, maxFileBytes = 20 * 1024 * 1024 } = {},
) {
  const map = new Map()
  const root = rootDir && String(rootDir).trim() !== "" ? path.resolve(String(rootDir)) : null
  const rootReal = root ? safeRealpathSync(root) || root : null
  const storePath = resolveStorePath(filePath, root)
  const maxEntriesValue = Number.parseInt(String(maxEntries), 10)
  const entryLimit = Number.isFinite(maxEntriesValue) && maxEntriesValue > 0 ? maxEntriesValue : 0
  const maxFileBytesValue = Number.parseInt(String(maxFileBytes), 10)
  const fileBytesLimit =
    Number.isFinite(maxFileBytesValue) && maxFileBytesValue > 0 ? maxFileBytesValue : 20 * 1024 * 1024

  let flushTimer = null
  let pending = Promise.resolve()
  let dirty = false
  let flushErrorReported = false
  let exitHooksRegistered = false
  let exitHookFlusher = null

  function reportFlushError(err) {
    logError("session store flush failed", err)
    if (isVerbose()) return
    if (flushErrorReported) return
    flushErrorReported = true
    const msg = err?.message || String(err)
    console.error("session store flush failed:", msg)
    console.error("set OPENCODE_BOT_VERBOSE=1 for stack traces")
  }

  function parsePersistedEntries(data) {
    if (!data || typeof data !== "object") {
      return { entries: [], isCurrentFormat: true }
    }
    if (Array.isArray(data.entries)) {
      return { entries: data.entries, isCurrentFormat: true }
    }
    if (Array.isArray(data)) {
      return { entries: data, isCurrentFormat: true }
    }
    return { entries: Object.entries(data), isCurrentFormat: false }
  }

  async function load() {
    if (!storePath) return
    const data = await safeReadJson(storePath, { maxBytes: fileBytesLimit })
    const { entries, isCurrentFormat } = parsePersistedEntries(data)
    if (!Array.isArray(entries) || entries.length === 0) return

    let evictedOnLoad = false
    for (const item of entries) {
      if (!Array.isArray(item) || item.length !== 2) continue
      const [k, v] = item
      if (typeof k !== "string") continue
      const evicted = setMapValue(k, v)
      if (evicted.length > 0) {
        evictedOnLoad = true
      }
    }

    // Compact oversized/legacy persisted data once during startup to avoid repeated heavy loads.
    if (evictedOnLoad || !isCurrentFormat) {
      dirty = true
      await enqueueFlush()
    }
  }

  async function flushNow() {
    if (!storePath || !dirty) return
    dirty = false
    const persisted = {
      version: 1,
      entries: [...map.entries()],
    }
    try {
      await atomicWriteUtf8(storePath, `${JSON.stringify(persisted)}\n`, {
        root,
        rootReal,
      })
    } catch (err) {
      dirty = true
      throw err
    }
  }

  function enqueueFlush() {
    const run = pending.then(flushNow)
    pending = run.catch((err) => {
      reportFlushError(err)
    })
    return run
  }

  function enforceEntryLimit(evicted) {
    if (entryLimit <= 0) return
    while (map.size > entryLimit) {
      const oldest = map.keys().next().value
      if (oldest === undefined) break
      const oldestValue = map.get(oldest)
      map.delete(oldest)
      evicted.push([oldest, oldestValue])
    }
  }

  function setMapValue(key, value) {
    const evicted = []
    if (map.has(key)) {
      // Maintain insertion order so eviction drops least-recently-updated keys first.
      map.delete(key)
    }
    map.set(key, value)
    enforceEntryLimit(evicted)
    return evicted
  }

  function scheduleFlush() {
    if (!storePath) return
    if (flushTimer) return
    flushTimer = setTimeout(() => {
      flushTimer = null
      void enqueueFlush()
    }, flushDebounceMs)
    if (typeof flushTimer?.unref === "function") {
      flushTimer.unref()
    }
  }

  function set(key, value) {
    const evicted = setMapValue(key, value)
    dirty = true
    scheduleFlush()
    return evicted
  }

  function del(key) {
    map.delete(key)
    dirty = true
    scheduleFlush()
  }

  function installExitHooks() {
    if (!storePath) return

    if (!exitHooksRegistered) {
      exitHooksRegistered = true
      exitHookFlusher = () => enqueueFlush()
      exitHookFlushers.add(exitHookFlusher)
    }
    installGlobalExitHooks()
  }

  function close() {
    if (flushTimer) {
      clearTimeout(flushTimer)
      flushTimer = null
    }
    if (dirty) {
      void enqueueFlush()
    }
    if (exitHookFlusher) {
      exitHookFlushers.delete(exitHookFlusher)
      exitHookFlusher = null
      exitHooksRegistered = false
    }
    return pending
  }

  return {
    enabled: Boolean(storePath),
    path: storePath,
    map,
    load,
    flush: enqueueFlush,
    set,
    delete: del,
    installExitHooks,
    close,
  }
}
