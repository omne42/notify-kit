import fsSync from "node:fs"
import fs from "node:fs/promises"
import path from "node:path"
import process from "node:process"

import { ignoreError, isVerbose, logError } from "./log.mjs"

async function safeReadJson(filePath) {
  try {
    const raw = await fs.readFile(filePath, "utf-8")
    return JSON.parse(raw)
  } catch (err) {
    if (err && err.code === "ENOENT") return null
    throw err
  }
}

async function atomicWriteUtf8(filePath, content, { root = null, rootReal = null } = {}) {
  const dir = path.dirname(filePath)
  await fs.mkdir(dir, { recursive: true })
  if (root && rootReal) {
    assertDirRealWithinRoot(root, rootReal, dir)
  }

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

export function createSessionStore(filePath, { flushDebounceMs = 250, rootDir = null } = {}) {
  const map = new Map()
  const root = rootDir && String(rootDir).trim() !== "" ? path.resolve(String(rootDir)) : null
  const rootReal = root ? safeRealpathSync(root) || root : null
  const storePath = resolveStorePath(filePath, root)

  let flushTimer = null
  let pending = Promise.resolve()
  let flushErrorReported = false

  function reportFlushError(err) {
    logError("session store flush failed", err)
    if (isVerbose()) return
    if (flushErrorReported) return
    flushErrorReported = true
    const msg = err?.message || String(err)
    console.error("session store flush failed:", msg)
    console.error("set OPENCODE_BOT_VERBOSE=1 for stack traces")
  }

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
    await atomicWriteUtf8(storePath, `${JSON.stringify(obj, null, 2)}\n`, {
      root,
      rootReal,
    })
  }

  function scheduleFlush() {
    if (!storePath) return
    if (flushTimer) return
    flushTimer = setTimeout(() => {
      flushTimer = null
      pending = pending.then(flushNow).catch((err) => {
        reportFlushError(err)
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

    const flush = () => flushNow().catch((err) => reportFlushError(err))
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
