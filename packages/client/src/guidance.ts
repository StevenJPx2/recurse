import { existsSync, readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import type { SkillInfo } from "./protocol.ts"

export const GUIDANCE_TAG = "recurse-guidance"

const FALLBACK_GUIDANCE = `You are working inside recurse: a persistent Python kernel reached through the \`ipython\` tool.
- State (variables, imports, background handles) persists across calls and turns.
- Use plain Python for files and data, \`await bash("cmd")\` for shell commands, and \`h = bash("cmd")\` (no await) for long-running ones; end your turn instead of blocking and you will be notified when it finishes.
- Spawn child agents with \`await rlm.spawn(task, name="...")\`; their results arrive as messages. Children reply with \`await agent_message.send(msg, receiver_role="parent")\`.
- Cells support top-level \`await\`; the last expression's repr is returned.`

export const IPYTHON_DESCRIPTION = `Run Python in this session's persistent IPython-style kernel. This is your primary tool.

- State persists across calls, turns, and compaction: variables, imports, open handles.
- Top-level \`await\` works. The repr of the last expression is returned as \`Out:\`.
- Preloaded: \`bash\` (\`await bash("npm test")\` returns output + exit_code; \`h = bash("cmd")\` without await starts a background handle with poll()/output()/tail()), \`rlm\` (\`await rlm.spawn(task, name=...)\` starts a child agent, \`rlm.list_subagents()\`, \`rlm.delete_subagent()\`), and \`agent_message\` (\`await agent_message.send(msg, receiver_role="parent"|"child", receiver_name=...)\`).
- Use plain Python (pathlib, json, re, subprocess via bash) to read, search, and edit files.
- Do not block waiting on background work or children: end your turn. Finished background commands and child messages arrive as new turns.
- One cell runs at a time per session; timeout_sec (default 600, max 3600) interrupts a runaway cell without losing state.`

/** GUIDANCE.md shipped next to the bundle, else the repository copy, else a short built-in text. */
export function loadGuidance(moduleUrl: string): string {
  for (const relative of ["./GUIDANCE.md", "../GUIDANCE.md", "../../../GUIDANCE.md"]) {
    const file = fileURLToPath(new URL(relative, moduleUrl))

    try {
      if (existsSync(file)) return readFileSync(file, "utf8").trim()
    } catch {
      // Unreadable copies fall through to the next candidate.
    }
  }

  return FALLBACK_GUIDANCE
}

export function renderSkills(skills: readonly SkillInfo[]): string {
  const visible = skills.filter((skill) => !skill.hidden)

  if (visible.length === 0) return ""

  const lines = visible.map((skill) => `- ${skill.name}: ${skill.description} (${skill.path})`)

  return ["Skills (read the SKILL.md from Python when a task matches):", ...lines].join("\n")
}

export function guidanceBody(guidance: string, skills: readonly SkillInfo[]): string {
  const listing = renderSkills(skills)

  return listing ? `${guidance}\n\n${listing}` : guidance
}

/** Guidance wrapped in a tag so hooks can tell it is already present. */
export function renderGuidance(guidance: string, skills: readonly SkillInfo[]): string {
  return `<${GUIDANCE_TAG}>\n${guidanceBody(guidance, skills)}\n</${GUIDANCE_TAG}>`
}

export interface StrictConfig {
  strict: boolean
  allow: string[]
}

export function strictConfig(env: NodeJS.ProcessEnv = process.env): StrictConfig {
  const allow = (env.RECURSE_ALLOW_TOOLS ?? "")
    .split(",")
    .map((name) => name.trim())
    .filter(Boolean)

  return { strict: !["0", "false", "off", "no"].includes((env.RECURSE_STRICT ?? "1").trim().toLowerCase()), allow }
}
