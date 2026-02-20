import assert from "node:assert/strict"
import fs from "node:fs/promises"
import { existsSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { test } from "node:test"

import { createSessionStore } from "./session_store.mjs"

async function makeTempStorePath() {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "notify-kit-session-store-"))
  return {
    dir,
    file: path.join(dir, "sessions.json"),
  }
}

test("delete missing key does not create persistence file", async () => {
  const { dir, file } = await makeTempStorePath()
  const store = createSessionStore(file, { flushDebounceMs: 5 })
  await store.load()

  const deleted = store.delete("missing")
  assert.equal(deleted, false)

  await store.flush()
  await store.close()

  assert.equal(existsSync(file), false)
  await fs.rm(dir, { recursive: true, force: true })
})

test("delete existing key persists removal", async () => {
  const { dir, file } = await makeTempStorePath()
  const store = createSessionStore(file, { flushDebounceMs: 5 })
  await store.load()

  store.set("k", { sessionId: "s1" })
  await store.flush()
  assert.equal(existsSync(file), true)

  const deleted = store.delete("k")
  assert.equal(deleted, true)
  await store.flush()
  await store.close()

  const persisted = JSON.parse(await fs.readFile(file, "utf-8"))
  assert.deepEqual(persisted.entries, [])
  await fs.rm(dir, { recursive: true, force: true })
})
