import { iconsFromSvg } from "/vendor/standalone.js"

const key = new URLSearchParams(location.hash.slice(1)).get("key")
const json = (url) => fetch(url).then((r) => r.json())
const [icons, shellMessages] = await Promise.all([json("/vendor/icons.json"), json("/shell.json")])
const plugin = { icons: iconsFromSvg((name) => icons[name]) }

function surface(parent) {
  const el = document.createElement("exponential-surface")
  el.setAttribute("theme", "exponential")
  el.setAttribute("mode", "dark")
  el.setAttribute("width", "100%")
  el.plugin = plugin
  parent.replaceChildren(el)
  return el
}

function apply(el, message) {
  if (message.createSurface) el.surfaceId = message.createSurface.surfaceId
  el.apply(message)
}

const shell = surface(document.getElementById("shell"))
for (const m of shellMessages) apply(shell, m)
const show = (values) => Object.entries(values).forEach(([name, value]) => shell.setData("/" + name, value))

const seconds = (ms) => `${(ms / 1000).toFixed(2)} s`
const painted = () => new Promise((done) => requestAnimationFrame(() => setTimeout(done)))
let express = ""
let busy = false
let lastGood = [] // the messages of the last finished screen

async function* events(url, body) {
  const headers = { "content-type": "application/json", ...(key && { "x-demo-key": key }) }
  const r = await fetch(url, { method: "POST", headers, body: JSON.stringify(body) })
  if (!r.ok) throw new Error((await r.json().catch(() => ({}))).message ?? `HTTP ${r.status}`)
  const reader = r.body.pipeThrough(new TextDecoderStream()).getReader()
  let rest = ""
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    const lines = (rest + value).split("\n")
    rest = lines.pop()
    for (const line of lines) if (line) yield JSON.parse(line)
  }
}

// what the renderer rejected, as one line
function problems(el) {
  const issues = (el?.issues ?? []).map((i) => `#${i.id}: ${i.message}`)
  if (!issues.length) return ""
  const more = issues.length > 3 ? ` (and ${issues.length - 3} more)` : ""
  return `The renderer rejected part of this screen: ${issues.slice(0, 3).join("; ")}${more}`
}

async function run(url, body) {
  if (busy) return
  busy = true
  const started = performance.now()
  const since = () => seconds(performance.now() - started)
  document.body.classList.add("streaming")
  show({ busy: true, error: "", note: "", status: "Writing…", first: "-", total: "-" })
  const parent = document.getElementById("result")
  let el = null
  let source = []
  let messages = []
  let restarts = 0
  let finished = false
  try {
    for await (const event of events(url, body)) {
      if (event.type === "reset") {
        if (el) el = surface(parent)
        if (source.length) show({ status: `Attempt ${++restarts + 1}…` })
        source = []
        messages = []
      } else if (event.type === "statement") {
        source.push(event.source)
        show({ source: source.join("\n"), has: true })
      } else if (event.type === "messages") {
        const first = !el
        el ??= surface(parent)
        for (const m of event.messages) apply(el, m)
        messages.push(...event.messages)
        if (first) {
          await painted()
          show({ first: since() })
        }
      } else if (event.type === "error") {
        throw new Error(event.message)
      } else if (event.type === "done") {
        finished = true
        express = event.express
        lastGood = messages
        await painted()
        const facts = [event.attempts && `${event.attempts} attempt${event.attempts > 1 ? "s" : ""}`, event.tokens && `${event.tokens} tokens`]
        show({
          total: since(), source: express, has: true, status: facts.filter(Boolean).join(", "),
          error: problems(el),
          note: event.streamed ? "" : "This server does not stream: the screen arrived in one piece, so the first block is the whole reply.",
        })
      }
    }
    if (!finished) throw new Error("the connection ended early")
  } catch (e) {
    // a preview that never became valid is not a result: back to the last one
    show({ error: e.message, status: "", total: since(), source: express, has: !!express })
    if (el) restore(parent)
  } finally {
    busy = false
    document.body.classList.remove("streaming")
    show({ busy: false })
  }
}

function restore(parent) {
  parent.replaceChildren()
  if (lastGood.length) {
    const el = surface(parent)
    for (const m of lastGood) apply(el, m)
  }
}

shell.addEventListener("xui-action", (e) => {
  const { name, context = {} } = e.detail
  if (name === "generate") {
    const request = context.request?.trim()
    if (!request) return show({ error: "Describe the screen first." })
    let data
    if (context.data?.trim()) {
      try {
        data = JSON.parse(context.data)
      } catch (err) {
        return show({ error: `The data is not JSON: ${err.message}` })
      }
    }
    e.detail.waitUntil(run("/api/generate", { request, data }))
  } else if (name === "refine") {
    const change = context.change?.trim()
    if (!change) return show({ error: "Describe the change first." })
    e.detail.waitUntil(run("/api/refine", { current: express, change }))
  }
})
