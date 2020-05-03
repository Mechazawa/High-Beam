export function debounce (...args) {
  if (typeof args[0] === 'number' && args.length <= 2) {
    throw new Error('debounce decorators are broken (context missing)');

    // const { wait, immediate = false } = args;
    //
    // return function (target, key, descriptor) {
    //   console.log('debounce', { target, key, descriptor });
    //   descriptor.value = _debounce((...args) => descriptor.value.apply(target, args), wait, immediate);
    //
    //   return descriptor;
    // };
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

    timeout = setTimeout(later, out.wait);

    if (callNow) func(...args);
  };

  out.cancel = () => {
    clearTimeout(timeout);

    timeout = undefined;
  };

  out.wait = wait;

  return out;
}

export function asyncDebounce (fn, wait, resolveAll = true, defaultValue = undefined) {
  let timeout;
  let promise, resolve, reject;

  const out = (...args) => {
    if (!resolveAll && promise) {
      resolve(defaultValue);
      promise = undefined;
    }

    if (!promise) {
      promise = new Promise((_resolve, _reject) => {
        resolve = _resolve;
        reject = _reject;
      });
    }

    const later = async () => {
      timeout = undefined;

      try {
        resolve(await fn(...args));
      } catch (err) {
        reject(err);
      } finally {
        promise = undefined;
      }
    };

    if (typeof timeout !== 'undefined') {
      clearTimeout(timeout);
    }

    timeout = setTimeout(later, out.wait);

    return promise;
  };

  out.cancel = () => {
    clearTimeout(timeout);

    timeout = undefined;
  };

  out.wait = wait;

  return out;
}
