// Coerce `query()`'s return value into an async iterator.
(o) => (typeof o?.next === 'function' ? o : o[Symbol.asyncIterator]())
