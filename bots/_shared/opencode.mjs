import process from "node:process"

export function assertEnv(name, { optional = false } = {}) {
  const value = process.env[name]
  if ((value === undefined || String(value).trim() === "") && !optional) {
    throw new Error(`missing required env: ${name}`)
  }
  return value
}

export function buildResponseText(response) {
  return (
    response?.info?.content ||
    response?.parts
      ?.filter((p) => p.type === "text")
      .map((p) => p.text)
      .join("\n") ||
    "I received your message but didn't have a response."
  )
}

export function getCompletedToolUpdate(part) {
  if (!part || part.type !== "tool") return null
  if (!part.state || part.state.status !== "completed") return null
  return {
    sessionId: part.sessionID,
    title: part.state.title || "completed",
    tool: part.tool || "tool",
  }
}

