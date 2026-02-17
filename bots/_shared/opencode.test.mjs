import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import path from "node:path"
import { test } from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const opencodeModuleUrl = pathToFileURL(path.join(here, "opencode.mjs")).href

function runNodeScript(script) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
      stdio: ["ignore", "pipe", "pipe"],
    })

    let stdout = ""
    let stderr = ""
    child.stdout.on("data", (chunk) => {
      stdout += String(chunk)
    })
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk)
    })
    child.on("error", reject)
    child.on("close", (code) => {
      resolve({ code, stdout, stderr })
    })
  })
}

test("runEventSubscriptionLoop retries when handler fails before concurrency cap", async () => {
  const script = `
import { runEventSubscriptionLoop } from ${JSON.stringify(opencodeModuleUrl)}

let subscribeCalls = 0
let first = true
console.error = () => {}

setTimeout(() => {
  console.log("SUBSCRIBE_CALLS=" + String(subscribeCalls))
  process.exit(0)
}, 400)

void runEventSubscriptionLoop({
  label: "test-loop",
  minBackoffMs: 10,
  maxBackoffMs: 10,
  jitterMs: 0,
  maxConcurrentOnEvent: 4,
  subscribe: async () => {
    subscribeCalls += 1
    return {
      stream: (async function* () {
        yield { id: 1 }
        if (first) {
          first = false
          await new Promise(() => {})
        }
      })(),
    }
  },
  onEvent: async () => {
    throw new Error("boom")
  },
})
`

  const { code, stdout, stderr } = await runNodeScript(script)
  assert.equal(code, 0, `child exited with non-zero code, stderr=${stderr}`)

  const match = stdout.match(/SUBSCRIBE_CALLS=(\d+)/)
  assert.ok(match, `missing subscribe count output, stdout=${stdout}`)
  const subscribeCalls = Number.parseInt(match[1], 10)
  assert.ok(Number.isFinite(subscribeCalls), `invalid subscribe count, stdout=${stdout}`)
  assert.ok(subscribeCalls >= 2, `expected loop retry, got subscribeCalls=${subscribeCalls}`)
})
