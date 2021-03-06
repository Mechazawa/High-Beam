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

export function highlightFuseMatches (matches) {
  const output = {};
  const insert = (strA, idx, strB) => strA.slice(0, idx) + strB + strA.slice(idx);

  for (const match of matches) {
    let str = match.value;

    for (let i = match.indices.length - 1; i >= 0; i--) {
      str = insert(str, match.indices[i][1] + 1, '</b>');
      str = insert(str, match.indices[i][0], '<b style="font-weight: bolder;">');
    }

    output[match.key] = str;
  }

  return output;
}
