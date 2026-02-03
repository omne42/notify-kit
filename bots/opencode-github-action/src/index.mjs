import fs from "node:fs/promises"
import process from "node:process"

import * as core from "@actions/core"
import * as github from "@actions/github"
import { createOpencode } from "@opencode-ai/sdk"

import { assertEnv, buildResponseText } from "../../_shared/opencode.mjs"

function shouldRun(body) {
  const text = String(body || "")
  return text.includes("/oc") || text.includes("/opencode")
}

function extractPrompt(body) {
  const text = String(body || "").trim()
  if (text.startsWith("/opencode")) {
    return text.replace(/^\/opencode\s*/u, "").trim()
  }
  return text.replace(/\s*\/oc\b/gu, "").trim()
}

function truncateForGitHub(body, max = 60000) {
  const text = String(body || "")
  if (text.length <= max) return text
  return `${text.slice(0, max - 20)}\n\n[truncated]\n`
}

async function safeReadJson(path) {
  const raw = await fs.readFile(path, "utf-8")
  return JSON.parse(raw)
}

async function formatIssueThread(octokit, { owner, repo, issueNumber }) {
  const issue = await octokit.rest.issues.get({ owner, repo, issue_number: issueNumber })
  const comments = await octokit.rest.issues.listComments({
    owner,
    repo,
    issue_number: issueNumber,
    per_page: 30,
  })

  const parts = []
  parts.push(`# ${issue.data.title}\n`)
  if (issue.data.body) {
    parts.push(`## Body\n${issue.data.body}\n`)
  }
  if (comments.data.length > 0) {
    parts.push("## Comments")
    for (const c of comments.data) {
      const who = c.user?.login || "unknown"
      const when = c.created_at || ""
      const body = c.body || ""
      parts.push(`- ${who} ${when}\n${body}\n`)
    }
    parts.push("")
  }

  return parts.join("\n")
}

async function formatPullRequest(octokit, { owner, repo, pullNumber }) {
  const pr = await octokit.rest.pulls.get({ owner, repo, pull_number: pullNumber })
  const parts = []
  parts.push(`# ${pr.data.title}\n`)
  if (pr.data.body) {
    parts.push(`## Body\n${pr.data.body}\n`)
  }
  return parts.join("\n")
}

async function run() {
  const githubToken = assertEnv("GITHUB_TOKEN")
  const eventName = assertEnv("GITHUB_EVENT_NAME")
  const eventPath = assertEnv("GITHUB_EVENT_PATH")

  const payload = await safeReadJson(eventPath)

  const octokit = github.getOctokit(githubToken)
  const owner = payload?.repository?.owner?.login || process.env.GITHUB_REPOSITORY_OWNER
  const repo = payload?.repository?.name || (process.env.GITHUB_REPOSITORY || "").split("/")[1]

  if (!owner || !repo) {
    throw new Error("unable to determine repo owner/name from payload/env")
  }

  if (eventName === "issue_comment") {
    const issueNumber = payload?.issue?.number
    const comment = payload?.comment
    const commentBody = comment?.body || ""
    const commenter = comment?.user?.login || ""
    const commenterType = comment?.user?.type || ""

    if (!issueNumber || !comment) return
    if (commenterType === "Bot" || commenter.endsWith("[bot]")) return
    if (!shouldRun(commentBody)) return

    const prompt = extractPrompt(commentBody)
    if (!prompt) return

    console.log("🚀 Starting opencode server...")
    const opencode = await createOpencode({ port: 0 })
    console.log("✅ Opencode server ready")

    const contextText = await formatIssueThread(octokit, { owner, repo, issueNumber })
    const created = await opencode.client.session.create({
      body: { title: `GitHub ${owner}/${repo}#${issueNumber}` },
    })
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    const shared = await opencode.client.session.share({ path: { id: sessionId } })
    const url = shared?.data?.share?.url

    const result = await opencode.client.session.prompt({
      path: { id: sessionId },
      body: {
        parts: [
          {
            type: "text",
            text: `You are responding to a GitHub issue comment.\n\n${contextText}\n\n## Request\n${prompt}\n`,
          },
        ],
      },
    })

    const responseText = buildResponseText(result.data)

    const body = truncateForGitHub([url ? `OpenCode session: ${url}` : null, responseText].filter(Boolean).join("\n\n"))
    await octokit.rest.issues.createComment({
      owner,
      repo,
      issue_number: issueNumber,
      body,
    })

    return
  }

  if (eventName === "pull_request_review_comment") {
    const pullNumber = payload?.pull_request?.number
    const comment = payload?.comment
    const commentBody = comment?.body || ""
    const commenter = comment?.user?.login || ""
    const commenterType = comment?.user?.type || ""

    if (!pullNumber || !comment) return
    if (commenterType === "Bot" || commenter.endsWith("[bot]")) return
    if (!shouldRun(commentBody)) return

    const prompt = extractPrompt(commentBody)
    if (!prompt) return

    console.log("🚀 Starting opencode server...")
    const opencode = await createOpencode({ port: 0 })
    console.log("✅ Opencode server ready")

    const prText = await formatPullRequest(octokit, { owner, repo, pullNumber })
    const codeContext = [
      "## Code context",
      `path: ${comment?.path || ""}`,
      `line: ${comment?.line ?? ""}`,
      "",
      "```diff",
      String(comment?.diff_hunk || "").trim(),
      "```",
      "",
    ].join("\n")

    const created = await opencode.client.session.create({
      body: { title: `GitHub ${owner}/${repo}#${pullNumber}` },
    })
    if (created.error) {
      throw new Error(created.error.message || "failed to create session")
    }

    const sessionId = created.data.id
    const shared = await opencode.client.session.share({ path: { id: sessionId } })
    const url = shared?.data?.share?.url

    const result = await opencode.client.session.prompt({
      path: { id: sessionId },
      body: {
        parts: [
          {
            type: "text",
            text: `You are responding to a GitHub PR review comment.\n\n${prText}\n\n${codeContext}\n## Request\n${prompt}\n`,
          },
        ],
      },
    })

    const responseText = buildResponseText(result.data)

    const body = truncateForGitHub([url ? `OpenCode session: ${url}` : null, responseText].filter(Boolean).join("\n\n"))

    await octokit.rest.pulls.createReplyForReviewComment({
      owner,
      repo,
      pull_number: pullNumber,
      comment_id: comment.id,
      body,
    })

    return
  }

  console.log(`unsupported event: ${eventName}; skipping`)
}

run().catch((err) => {
  core.setFailed(err?.message || String(err))
})
