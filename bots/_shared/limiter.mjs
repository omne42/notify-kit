export function createLimiter({ maxInflight = 4 } = {}) {
  const limit = Number.parseInt(String(maxInflight), 10)
  const concurrency = Number.isFinite(limit) && limit > 0 ? limit : 4

  let inflight = 0
  const queue = []

  const pump = () => {
    while (inflight < concurrency && queue.length > 0) {
      const item = queue.shift()
      if (!item) return
      inflight += 1
      Promise.resolve()
        .then(item.fn)
        .then(item.resolve, item.reject)
        .finally(() => {
          inflight -= 1
          pump()
        })
    }
  }

  const run = (fn) =>
    new Promise((resolve, reject) => {
      queue.push({ fn, resolve, reject })
      pump()
    })

  return { run, maxInflight: concurrency }
}

