import type { Register } from 'claude-code'

// One `listen` helper per session: the island writes one JSON-string line per phone prompt.
export const register: Register = (on) => {
  let stop: (() => void) | undefined

  on('classic.SessionStart', async ($, e, next) => {
    stop?.() // startup, resume and /clear each change the session: end the old loop (kills its child)
    const child = $.process.spawn({ argv: [`${$.plugin.root}/bin/agent-islands.exe`, 'listen', e.session_id, e.cwd] })
    let dead = false // return() may queue behind a parked next(): drop a replaced child's lines meanwhile
    const mine = () => { dead = true; void child.return({ code: null, signal: null }) } // return() kills the child
    stop = mine
    void (async () => {
      let buf = ''
      for await (const piece of child) {
        if (!('text' in piece) || piece.stream !== 'stdout') continue
        const lines = (buf + piece.text).split('\n')
        buf = lines.pop() ?? ''
        for (const line of lines) {
          if (dead) break
          try {
            void $.prompt.submit({ text: JSON.parse(line), asUser: true })
          } catch {} // not a JSON line: ignore
        }
      }
      if (stop === mine) stop = undefined
    })()
    return next(e)
  })
}
