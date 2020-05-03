export function incrementing () {
  if (incrementing.current === Number.MAX_SAFE_INTEGER) {
    incrementing.current = 0;
  }

  return incrementing.current++;
}

incrementing.current = 0;

export function randomString (length = 11) {
  let output = '';

  while (output.length < length) {
    output += Math.random().toString(36).substr(2);
  }

  return output.slice(0, length);
}
