import type { Register } from 'claude-code'

export const register: Register = (on) => {
  on('classic.SessionStart', ($, e, next) => {
    $.ui.log(JSON.stringify({ sid: e.session_id, cwd: e.cwd }), { to: 'debug' })
    return next(e)
  })
}
