export function debounce (...args) {
  if (typeof args[0] === 'number' && args.length <= 2) {
    const { wait, immediate = false } = args;
    return function ({ descriptor }) {
      descriptor.value = _debounce(descriptor.value, wait, immediate);

      return descriptor;
    }
  } else if (args.length === 2 || args.length === 3) {
    const { func, wait, immediate = false } = args;

    return _debounce(func, wait, immediate);
  }

  throw new Error(`Invalid argument count for debounce expected 1, 2 or 3 arguments got ${args.length}`);
}

function _debounce (func, wait, immediate = false) {
  let timeout;

  const out = (...args) => {
    const later = () => {
      timeout = undefined;

      if (!immediate) func(...args);
    };

    const callNow = immediate && typeof timeout === 'undefined';

    if (typeof timeout !== 'undefined') {
      clearTimeout(timeout);
    }

    timeout = setTimeout(later, wait);

    if (callNow) func(...args);
  };

  out.cancel = () => {
    clearTimeout(timeout);

    timeout = undefined;
  };

  return out;
}
