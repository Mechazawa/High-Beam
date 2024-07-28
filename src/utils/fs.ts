import { resolve } from "path";
import fs from "fs";

const { readdir } = fs.promises;

/**
 * Retrieves a list of files
 * @param dir Target directory
 * @param skipHidden Skip hidden files/directories
 */
export async function* getFiles (dir: string, skipHidden = true): AsyncGenerator<string> {
  const entries = await readdir(dir, { withFileTypes: true });

  for (const entry of entries) {
    const skip = skipHidden && /^\.[^\\/]/.test(entry.name); entry.name.startsWith('.')
    const res = resolve(dir, entry.name);

    if (!skip && entry.isDirectory()) {
      yield* getFiles(res);
    } else {
      yield res;
    }
  }
}